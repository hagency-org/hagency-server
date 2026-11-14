//! Field rules for the five v1 event types (Annex 3 §6.1, §7). Unknown schema versions, missing or
//! mistyped required fields and undeclared fields are refused rather than downgraded.

use serde_json::{Map, Value};

use crate::{Error, canonical, invalid};

pub const TASK: &str = "gitfab.task.v1";
pub const IDENTITY: &str = "gitfab.identity.v1";
pub const MEMORY: &str = "gitfab.memory.v1";
pub const TOKEN: &str = "gitfab.token.v1";
pub const RELEASE: &str = "gitfab.release.v1";

const PHASES: &[&str] = &[
    "intake", "spec", "plan", "execute", "test", "review", "release", "deploy",
];
const STATUSES: &[&str] = &[
    "queued",
    "running",
    "succeeded",
    "failed",
    "blocked",
    "manual_intervention",
];
const BACKENDS: &[&str] = &["gitcode", "github"];
pub const KEY_EVENT_TYPES: &[&str] = &[
    "task_created",
    "execution_result",
    "reviewer_verdict",
    "release_issued",
];
const ACTOR_TYPES: &[&str] = &["human", "agent", "system"];

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Digest,
    DigestOrNull,
    Sha,
    Time,
    Int,
    Array,
    Object,
    Signature,
    SignatureOrNull,
    OneOf(&'static [&'static str]),
}
use Kind::*;

type Fields = &'static [(&'static str, Kind)];

const TASK_FIELDS: Fields = &[
    ("schema_version", Text),
    ("event_id", Text),
    ("timestamp", Time),
    ("project_id", Text),
    ("task_id", Text),
    ("run_id", Text),
    ("matrix_room_id", Text),
    ("matrix_thread_event_id", Text),
    ("git_backend", OneOf(BACKENDS)),
    ("repo_ref", Text),
    ("baseline_sha", Sha),
    ("spec_ref", Text),
    ("phase", OneOf(PHASES)),
    ("status", OneOf(STATUSES)),
    ("actor_id", Text),
    ("input_digest", Digest),
    ("output_digest", DigestOrNull),
    ("artifact_refs", Array),
    ("request_id", Text),
    ("signature", SignatureOrNull),
];
// Annex 3 §7.1: a Reviewer rejection that returns to execution carries a round and keeps the findings.
// A stage that drove the Git platform names the commit it acted on, or why the platform refused.
const TASK_OPTIONAL: Fields = &[
    ("round", Int),
    ("findings", Array),
    ("commit_sha", Sha),
    ("platform_error", Text),
    // The Plan's Module breakdown: id, title, spec, tests and capabilities of each Module.
    ("modules", Array),
    // The Spec version the stage worked from.
    ("spec_revision", Int),
    // Git blob SHA of the Spec file at the stage's commit.
    ("spec_blob", Sha),
];

const IDENTITY_FIELDS: Fields = &[
    ("schema_version", Text),
    ("event_id", Text),
    ("timestamp", Time),
    ("event_type", OneOf(KEY_EVENT_TYPES)),
    ("project_id", Text),
    ("task_id", Text),
    ("actor_id", Text),
    ("actor_type", OneOf(ACTOR_TYPES)),
    ("key_id", Text),
    ("algorithm", Text),
    ("payload_digest", Digest),
    ("signature_base64", Text),
];

const MEMORY_FIELDS: Fields = &[
    ("schema_version", Text),
    ("event_id", Text),
    ("timestamp", Time),
    ("project", Text),
    ("module", Text),
    ("spec", Text),
    ("contributor", Text),
    ("action", Text),
    ("input_digest", Digest),
    ("output_digest", Digest),
    ("token_delta", Int),
    ("task_id", Text),
    ("signature", Signature),
];
// Annex 3 §7.4: a correction is a new event that names the event it reverses.
const MEMORY_OPTIONAL: Fields = &[("corrects", Text)];

const RELEASE_FIELDS: Fields = &[
    ("schema_version", Text),
    ("release_id", Text),
    ("project_id", Text),
    ("task_id", Text),
    ("git_backend", OneOf(BACKENDS)),
    ("repo_ref", Text),
    ("commit_sha", Sha),
    ("tag", Text),
    ("artifact_digest", Digest),
    ("image_ref", Text),
    ("image_digest", Digest),
    ("sbom_ref", Text),
    ("sbom_digest", Digest),
    ("deployed_at", Time),
    ("runner_id", Text),
    ("healthcheck", Object),
    ("rollback", Object),
    ("signature", Signature),
];

const SIGNATURE_FIELDS: Fields = &[
    ("algorithm", OneOf(&["ed25519"])),
    ("key_id", Text),
    ("public_key_ref", Text),
    ("payload_digest", Digest),
    ("signature_base64", Text),
];

/// Checks one event and returns its schema version.
pub fn validate(event: &Value) -> Result<&'static str, Error> {
    // Refuses floats anywhere before any field check.
    canonical::canonical(event)?;
    let map = event
        .as_object()
        .ok_or_else(|| Error::Json("an event is a JSON object".into()))?;
    let version = match map.get("schema_version") {
        Some(Value::String(v)) => v.as_str(),
        Some(_) => return Err(invalid("schema_version", "must be a string")),
        None => return Err(Error::Missing("schema_version".into())),
    };
    let (version, fields, optional): (&'static str, Fields, Fields) = match version {
        TASK => (TASK, TASK_FIELDS, TASK_OPTIONAL),
        IDENTITY => (IDENTITY, IDENTITY_FIELDS, &[]),
        // Annex 3 §7.4 names gitfab.token.v1 as the ledger rule set; its records carry the memory fields.
        MEMORY | TOKEN => (
            if version == MEMORY { MEMORY } else { TOKEN },
            MEMORY_FIELDS,
            MEMORY_OPTIONAL,
        ),
        RELEASE => (RELEASE, RELEASE_FIELDS, &[]),
        other => return Err(Error::UnknownSchema(other.to_owned())),
    };
    check_object(map, "", fields, optional)?;
    if version == TASK && key_event_type(event).is_some() && map["signature"].is_null() {
        return Err(Error::MissingSignature(
            key_event_type(event).unwrap().to_owned(),
        ));
    }
    if matches!(version, IDENTITY)
        && !map["algorithm"]
            .as_str()
            .is_some_and(|a| a.eq_ignore_ascii_case("ed25519"))
    {
        return Err(invalid("algorithm", "must be ed25519"));
    }
    forbid_secrets(event, "$")?;
    Ok(version)
}

/// Which of the four signed key events (Annex 1 §1.2.1(5)) a task event is, if any.
pub fn key_event_type(event: &Value) -> Option<&'static str> {
    let terminal = |s: &str| {
        matches!(
            s,
            "succeeded" | "failed" | "blocked" | "manual_intervention"
        )
    };
    let (phase, status) = (
        event.get("phase")?.as_str()?,
        event.get("status")?.as_str()?,
    );
    match phase {
        "intake" if status == "succeeded" => Some("task_created"),
        "execute" if terminal(status) => Some("execution_result"),
        "review" if terminal(status) => Some("reviewer_verdict"),
        "release" if status == "succeeded" => Some("release_issued"),
        _ => None,
    }
}

/// The idempotency key: `release_id` for releases, `event_id` otherwise.
pub fn event_key(event: &Value) -> Option<&str> {
    event
        .get("event_id")
        .or_else(|| event.get("release_id"))
        .and_then(Value::as_str)
}

fn check_object(
    map: &Map<String, Value>,
    prefix: &str,
    fields: Fields,
    optional: Fields,
) -> Result<(), Error> {
    for (name, kind) in fields {
        let path = format!("{prefix}{name}");
        let value = map.get(*name).ok_or_else(|| Error::Missing(path.clone()))?;
        check(value, &path, *kind)?;
    }
    for (name, kind) in optional {
        if let Some(value) = map.get(*name) {
            check(value, &format!("{prefix}{name}"), *kind)?;
        }
    }
    let known = |k: &str| fields.iter().chain(optional).any(|(name, _)| *name == k);
    if let Some(extra) = map.keys().find(|k| !known(k)) {
        return Err(Error::UnknownField(format!("{prefix}{extra}")));
    }
    Ok(())
}

fn check(value: &Value, path: &str, kind: Kind) -> Result<(), Error> {
    let text = || value.as_str().filter(|s| !s.is_empty());
    let ok = match kind {
        Text => text().is_some(),
        Digest => text().is_some_and(is_digest),
        DigestOrNull => value.is_null() || text().is_some_and(is_digest),
        Sha => text().is_some_and(|s| {
            matches!(s.len(), 40 | 64)
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }),
        Time => text()
            .is_some_and(|s| s.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(s).is_ok()),
        Int => value.as_i64().is_some(),
        Array => value.is_array(),
        Object => value.as_object().is_some_and(|m| !m.is_empty()),
        OneOf(allowed) => text().is_some_and(|s| allowed.contains(&s)),
        Signature | SignatureOrNull => {
            if value.is_null() && matches!(kind, SignatureOrNull) {
                return Ok(());
            }
            let map = value
                .as_object()
                .ok_or_else(|| invalid(path, "must be a signature object"))?;
            return check_object(map, &format!("{path}."), SIGNATURE_FIELDS, &[]);
        }
    };
    if ok {
        Ok(())
    } else {
        Err(invalid(path, describe(kind)))
    }
}

fn describe(kind: Kind) -> String {
    match kind {
        Text => "must be a non-empty string".into(),
        Digest | DigestOrNull => "must be sha256:<64 lowercase hex>".into(),
        Sha => "must be a 40 or 64 character lowercase hex commit SHA".into(),
        Time => "must be UTC RFC 3339 ending in Z".into(),
        Int => "must be a signed 64-bit integer".into(),
        Array => "must be an array".into(),
        Object => "must be a non-empty object".into(),
        OneOf(allowed) => format!("must be one of {}", allowed.join("/")),
        Signature | SignatureOrNull => "must be a signature object".into(),
    }
}

fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Annex 3 §6.1: no credentials in URLs and no local absolute paths anywhere in an event.
fn forbid_secrets(value: &Value, at: &str) -> Result<(), Error> {
    match value {
        Value::String(s) => {
            let credential_url = s.split_once("://").is_some_and(|(_, rest)| {
                rest.split(['/', '?', '#'])
                    .next()
                    .is_some_and(|authority| authority.contains('@'))
            });
            if credential_url {
                return Err(invalid(at, "contains a URL with credentials"));
            }
            let local_path = ["/Users/", "/home/", "/root/", "/private/", "/var/folders/"]
                .iter()
                .any(|p| s.starts_with(p))
                || s.as_bytes().get(1..3) == Some(b":\\");
            if local_path {
                return Err(invalid(at, "contains a local absolute path"));
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                forbid_secrets(item, &format!("{at}[{i}]"))?;
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                forbid_secrets(item, &format!("{at}.{key}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    use super::*;

    pub fn digest(c: char) -> String {
        format!("sha256:{}", c.to_string().repeat(64))
    }

    pub fn memory(event_id: &str, delta: i64) -> Value {
        json!({
            "schema_version": MEMORY, "event_id": event_id, "timestamp": "2026-11-12T03:00:00Z",
            "project": "DEMO-05", "module": "ledger", "spec": "spec-v1@sha256:ab", "contributor": "@impl:example.org",
            "action": "execute", "input_digest": digest('a'), "output_digest": digest('b'), "token_delta": delta,
            "task_id": "task-1",
            "signature": {"algorithm": "ed25519", "key_id": "k", "public_key_ref": "keys/k.pub",
                          "payload_digest": digest('c'), "signature_base64": "AA=="}
        })
    }

    pub fn task(phase: &str, status: &str) -> Value {
        json!({
            "schema_version": TASK, "event_id": format!("evt-{phase}-{status}"), "timestamp": "2026-11-12T03:00:00Z",
            "project_id": "DEMO-01", "task_id": "task-1", "run_id": "run-1", "matrix_room_id": "!room:example.org",
            "matrix_thread_event_id": "$root", "git_backend": "github", "repo_ref": "https://github.com/org/demo-01",
            "baseline_sha": "a".repeat(40), "spec_ref": "spec/DEMO-01.md@v1", "phase": phase, "status": status,
            "actor_id": "@coordinator:example.org", "input_digest": digest('a'), "output_digest": null,
            "artifact_refs": [], "request_id": "req-1", "signature": null
        })
    }

    #[test]
    fn well_formed_events_pass() {
        assert_eq!(validate(&memory("m1", -3)).unwrap(), MEMORY);
        assert_eq!(validate(&task("plan", "running")).unwrap(), TASK);
        let mut e = task("test", "running");
        e["commit_sha"] = json!("c".repeat(40));
        e["platform_error"] = json!("GitHub answered 403 to POST /repos/o/r/pulls");
        assert_eq!(validate(&e).unwrap(), TASK);
        e["commit_sha"] = json!("main");
        assert_eq!(validate(&e).unwrap_err().code(), "invalid_field");
    }

    #[test]
    fn version_fields_and_types_are_checked() {
        let mut e = memory("m1", 1);
        e["schema_version"] = json!("gitfab.memory.v2");
        assert_eq!(validate(&e).unwrap_err().code(), "unknown_schema");
        let mut e = memory("m1", 1);
        e.as_object_mut().unwrap().remove("spec");
        assert_eq!(validate(&e).unwrap_err(), Error::Missing("spec".into()));
        let mut e = memory("m1", 1);
        e["token_delta"] = json!("5");
        assert_eq!(validate(&e).unwrap_err().code(), "invalid_field");
        let mut e = memory("m1", 1);
        e["token_delta"] = json!(1.5);
        assert_eq!(validate(&e).unwrap_err().code(), "non_integer");
        let mut e = memory("m1", 1);
        e["token_delta"] = json!(u64::MAX);
        assert_eq!(validate(&e).unwrap_err().code(), "invalid_field");
        let mut e = memory("m1", 1);
        e["extra"] = json!(1);
        assert_eq!(
            validate(&e).unwrap_err(),
            Error::UnknownField("extra".into())
        );
        let mut e = memory("m1", 1);
        e["timestamp"] = json!("2026-11-12T11:00:00+08:00");
        assert_eq!(validate(&e).unwrap_err().code(), "invalid_field");
        let mut e = memory("m1", 1);
        e["signature"]["note"] = json!("x");
        assert_eq!(
            validate(&e).unwrap_err(),
            Error::UnknownField("signature.note".into())
        );
    }

    #[test]
    fn key_task_events_need_a_signature() {
        assert_eq!(
            validate(&task("intake", "succeeded")).unwrap_err(),
            Error::MissingSignature("task_created".into())
        );
        assert_eq!(
            validate(&task("review", "failed")).unwrap_err().code(),
            "missing_signature"
        );
        assert!(validate(&task("review", "running")).is_ok());
    }

    #[test]
    fn credentials_and_local_paths_are_refused() {
        let mut e = task("plan", "running");
        e["repo_ref"] = json!("https://user:token@github.com/org/demo");
        assert_eq!(validate(&e).unwrap_err().code(), "invalid_field");
        let mut e = task("plan", "running");
        e["artifact_refs"] = json!(["/Users/alice/work/report.json"]);
        assert_eq!(validate(&e).unwrap_err().code(), "invalid_field");
        let mut e = task("plan", "running");
        e["artifact_refs"] = json!([
            "ghcr.io/org/demo@sha256:abc",
            "https://github.com/org/demo/pull/1"
        ]);
        assert!(validate(&e).is_ok());
    }
}
