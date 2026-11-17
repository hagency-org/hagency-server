//! Append-only JSONL journal (Annex 3 §6, §7). Events are signed when they must be, key events are
//! followed by their identity event, and an event key is written once: the same content again is a no-op,
//! different content is refused. Recorded lines are never rewritten.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::sign::Key;
use crate::{Error, canonical, identity, schema};

pub struct Signer<'a> {
    pub key: &'a Key,
    pub public_key_ref: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Appended {
    /// Written, with the identity event when it is a key event.
    Written { lines: usize },
    /// Already recorded with identical content; nothing written.
    Duplicate,
}

pub struct Journal {
    path: PathBuf,
    /// Canonical content by event key.
    recorded: HashMap<String, String>,
}

impl Journal {
    /// Opens or creates the journal and indexes it. An invalid line or a conflicting duplicate in the file
    /// stops here rather than being appended to.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let mut recorded = HashMap::new();
        if path.exists() {
            let text = std::fs::read_to_string(path).map_err(io)?;
            for (n, raw) in text
                .lines()
                .enumerate()
                .filter(|(_, l)| !l.trim().is_empty())
            {
                let event: Value = serde_json::from_str(raw)
                    .map_err(|e| Error::Json(format!("line {}: {e}", n + 1)))?;
                schema::validate(&event)?;
                let key = schema::event_key(&event).expect("validated").to_owned();
                let content = canonical::canonical(&event)?;
                if recorded.get(&key).is_some_and(|seen| *seen != content) {
                    return Err(Error::Conflict(key));
                }
                recorded.insert(key, content);
            }
        }
        Ok(Self {
            path: path.to_owned(),
            recorded,
        })
    }

    /// Appends one event. Memory, token and release events and key task events are signed with `signer`;
    /// `actor_type` (human, agent or system) goes into the identity event of a key event.
    pub fn append(
        &mut self,
        event: &Value,
        signer: Option<&Signer>,
        actor_type: &str,
    ) -> Result<Appended, Error> {
        let event = if needs_signature(event) {
            let signer = signer.ok_or_else(|| Error::MissingSignature(kind(event).to_owned()))?;
            signer.key.sign(event, signer.public_key_ref)?
        } else {
            event.clone()
        };
        schema::validate(&event)?;
        let mut lines = vec![event.clone()];
        if let Some(kind) = identity::attested_type(&event) {
            let id = format!("identity:{}", schema::event_key(&event).expect("validated"));
            let at = event
                .get("timestamp")
                .or_else(|| event.get("deployed_at"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let attest = identity::for_event(&event, &id, at, actor_type)
                .ok_or_else(|| Error::MissingIdentity(kind.to_owned()))?;
            schema::validate(&attest)?;
            lines.push(attest);
        }
        let mut fresh = Vec::new();
        for line in &lines {
            let key = schema::event_key(line).expect("validated").to_owned();
            let content = canonical::canonical(line)?;
            match self.recorded.get(&key) {
                Some(seen) if *seen == content => {}
                Some(_) => return Err(Error::Conflict(key)),
                None => fresh.push((key, content)),
            }
        }
        if fresh.is_empty() {
            return Ok(Appended::Duplicate);
        }
        // One write and one sync, so an event and its identity event land together.
        let text: String = fresh.iter().map(|(_, c)| format!("{c}\n")).collect();
        let mut file: File = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(io)?;
        file.write_all(text.as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
        let written = fresh.len();
        self.recorded.extend(fresh);
        Ok(Appended::Written { lines: written })
    }
}

fn needs_signature(event: &Value) -> bool {
    match event["schema_version"].as_str() {
        Some(schema::MEMORY | schema::TOKEN | schema::RELEASE) => true,
        Some(schema::TASK) => schema::key_event_type(event).is_some(),
        _ => false,
    }
}

fn kind(event: &Value) -> &str {
    schema::key_event_type(event)
        .or(event["schema_version"].as_str())
        .unwrap_or("event")
}

fn io(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::schema::tests::{memory, task};

    fn unsigned_memory(id: &str, delta: i64) -> Value {
        let mut e = memory(id, delta);
        e.as_object_mut().unwrap().remove("signature");
        e["signature"] = Value::Null;
        e
    }

    fn setup() -> (tempfile::TempDir, PathBuf, Key) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        (dir, path, Key::generate("gitfab-1").unwrap())
    }

    #[test]
    fn key_events_are_signed_and_followed_by_their_identity_event() {
        let (_dir, path, key) = setup();
        let signer = Signer {
            key: &key,
            public_key_ref: "keys/gitfab-1.pub",
        };
        let mut journal = Journal::open(&path).unwrap();
        let created = task("intake", "succeeded");
        assert_eq!(
            journal.append(&created, Some(&signer), "agent").unwrap(),
            Appended::Written { lines: 2 }
        );
        assert_eq!(
            journal
                .append(&task("plan", "running"), Some(&signer), "agent")
                .unwrap(),
            Appended::Written { lines: 1 }
        );

        let events: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(events.len(), 3);
        let mut ring = crate::sign::Keyring::default();
        ring.insert("gitfab-1", &key.public_base64()).unwrap();
        ring.verify(&events[0]).unwrap();
        assert_eq!(events[1]["event_type"], "task_created");
        assert!(identity::check(&events).is_empty());
    }

    #[test]
    fn appending_again_is_a_no_op_and_changed_content_is_refused() {
        let (_dir, path, key) = setup();
        let signer = Signer {
            key: &key,
            public_key_ref: "keys/gitfab-1.pub",
        };
        let mut journal = Journal::open(&path).unwrap();
        journal
            .append(&unsigned_memory("m1", 5), Some(&signer), "agent")
            .unwrap();
        assert_eq!(
            journal
                .append(&unsigned_memory("m1", 5), Some(&signer), "agent")
                .unwrap(),
            Appended::Duplicate
        );
        assert_eq!(
            journal
                .append(&unsigned_memory("m1", 6), Some(&signer), "agent")
                .unwrap_err(),
            Error::Conflict("m1".into())
        );

        // A reopened journal remembers what it holds.
        let mut reopened = Journal::open(&path).unwrap();
        assert_eq!(
            reopened
                .append(&unsigned_memory("m1", 5), Some(&signer), "agent")
                .unwrap(),
            Appended::Duplicate
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
    }

    #[test]
    fn a_signed_type_without_a_signer_or_an_invalid_event_writes_nothing() {
        let (_dir, path, _key) = setup();
        let mut journal = Journal::open(&path).unwrap();
        assert_eq!(
            journal
                .append(&unsigned_memory("m1", 5), None, "agent")
                .unwrap_err()
                .code(),
            "missing_signature"
        );
        let mut bad = task("plan", "running");
        bad["phase"] = json!("deploying");
        assert_eq!(
            journal.append(&bad, None, "agent").unwrap_err().code(),
            "invalid_field"
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_tampered_file_is_refused_on_open() {
        let (_dir, path, key) = setup();
        let signer = Signer {
            key: &key,
            public_key_ref: "keys/gitfab-1.pub",
        };
        let mut journal = Journal::open(&path).unwrap();
        journal
            .append(&unsigned_memory("m1", 5), Some(&signer), "agent")
            .unwrap();
        let line = std::fs::read_to_string(&path).unwrap();
        let mut changed: Value = serde_json::from_str(line.trim()).unwrap();
        changed["token_delta"] = json!(50);
        std::fs::write(&path, format!("{line}{changed}\n")).unwrap();
        assert_eq!(
            Journal::open(&path).err().unwrap(),
            Error::Conflict("m1".into())
        );
    }
}
