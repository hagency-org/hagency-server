//! Token ledger (Annex 3 §7.4): idempotent by event key, a conflicting duplicate fails the whole ledger, and
//! totals are exact integer sums that can be recomputed per project, module, Spec and contributor.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{Error, canonical, schema};

#[derive(Debug, PartialEq, Eq)]
pub enum LedgerError {
    /// A line is not a valid event.
    Invalid {
        line: usize,
        error: Error,
    },
    /// The same key was recorded with different content.
    Conflict {
        key: String,
        first_line: usize,
        line: usize,
    },
    /// A correction names an event that is not a token event in this ledger.
    DanglingCorrection {
        line: usize,
        corrects: String,
    },
    Overflow {
        line: usize,
    },
}

impl LedgerError {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Invalid { line, error } => {
                json!({"line": line, "code": error.code(), "message": error.to_string()})
            }
            Self::Conflict {
                key,
                first_line,
                line,
            } => {
                json!({"line": line, "code": "conflict", "key": key, "first_line": first_line,
                       "message": format!("{key} was recorded on line {first_line} with different content")})
            }
            Self::DanglingCorrection { line, corrects } => {
                json!({"line": line, "code": "dangling_correction", "corrects": corrects,
                       "message": format!("corrects {corrects:?}, which is not a token event in this ledger")})
            }
            Self::Overflow { line } => {
                json!({"line": line, "code": "overflow", "message": "token sum exceeds i64"})
            }
        }
    }
}

#[derive(Default)]
pub struct Ledger {
    /// Unique events by key, with the line each was first seen on.
    events: BTreeMap<String, (usize, String, Value)>,
    pub duplicates: usize,
    pub lines: usize,
}

impl Ledger {
    /// Reads JSONL. Blank lines are skipped; every other line must be one complete event.
    pub fn read(&mut self, text: &str, first_line: usize) -> Vec<LedgerError> {
        let mut errors = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = first_line + i;
            if raw.trim().is_empty() {
                continue;
            }
            self.lines += 1;
            if let Err(e) = self.add(raw, line) {
                errors.push(e);
            }
        }
        errors
    }

    fn add(&mut self, raw: &str, line: usize) -> Result<(), LedgerError> {
        let event: Value = serde_json::from_str(raw).map_err(|e| LedgerError::Invalid {
            line,
            error: Error::Json(e.to_string()),
        })?;
        schema::validate(&event).map_err(|error| LedgerError::Invalid { line, error })?;
        let canonical =
            canonical::canonical(&event).map_err(|error| LedgerError::Invalid { line, error })?;
        let key = schema::event_key(&event)
            .expect("validated events carry a key")
            .to_owned();
        match self.events.get(&key) {
            Some((_, seen, _)) if *seen == canonical => self.duplicates += 1,
            Some((first_line, ..)) => {
                return Err(LedgerError::Conflict {
                    key,
                    first_line: *first_line,
                    line,
                });
            }
            None => {
                self.events.insert(key, (line, canonical, event));
            }
        }
        Ok(())
    }

    pub fn events(&self) -> impl Iterator<Item = (usize, &Value)> {
        let mut all: Vec<_> = self
            .events
            .values()
            .map(|(line, _, e)| (*line, e))
            .collect();
        all.sort_by_key(|(line, _)| *line);
        all.into_iter()
    }

    /// Totals over unique token-bearing events. Fails on overflow or a correction with no target.
    pub fn summary(&self) -> Result<Value, Vec<LedgerError>> {
        let is_token = |e: &Value| {
            matches!(
                e["schema_version"].as_str(),
                Some(schema::MEMORY | schema::TOKEN)
            )
        };
        let token_keys: BTreeSet<&str> = self
            .events
            .iter()
            .filter(|(_, (.., e))| is_token(e))
            .map(|(k, _)| k.as_str())
            .collect();
        let mut errors = Vec::new();
        let mut total: i64 = 0;
        let mut groups: BTreeMap<&str, BTreeMap<String, i64>> = BTreeMap::new();
        let mut corrections = 0;
        for (line, e) in self.events().filter(|(_, e)| is_token(e)) {
            if let Some(target) = e.get("corrects").and_then(Value::as_str) {
                corrections += 1;
                if !token_keys.contains(target) || Some(target) == schema::event_key(e) {
                    errors.push(LedgerError::DanglingCorrection {
                        line,
                        corrects: target.to_owned(),
                    });
                }
            }
            let delta = e["token_delta"].as_i64().expect("validated");
            let Some(sum) = total.checked_add(delta) else {
                errors.push(LedgerError::Overflow { line });
                continue;
            };
            total = sum;
            for dim in ["project", "module", "spec", "contributor"] {
                let slot = groups
                    .entry(dim)
                    .or_default()
                    .entry(e[dim].as_str().expect("validated").to_owned())
                    .or_default();
                match slot.checked_add(delta) {
                    Some(sum) => *slot = sum,
                    None => errors.push(LedgerError::Overflow { line }),
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut by_schema: BTreeMap<&str, usize> = BTreeMap::new();
        for (.., e) in self.events.values() {
            *by_schema
                .entry(e["schema_version"].as_str().unwrap_or(""))
                .or_default() += 1;
        }
        // Digest over the sorted unique events, so two parties can compare ledgers without exchanging them.
        let joined: Vec<&str> = self.events.values().map(|(_, c, _)| c.as_str()).collect();
        Ok(json!({
            "token_total": total,
            "by_project": groups.remove("project").unwrap_or_default(),
            "by_module": groups.remove("module").unwrap_or_default(),
            "by_spec": groups.remove("spec").unwrap_or_default(),
            "by_contributor": groups.remove("contributor").unwrap_or_default(),
            "unique_events": self.events.len(),
            "events_by_schema": by_schema,
            "duplicates_ignored": self.duplicates,
            "corrections": corrections,
            "ledger_digest": canonical::sha256(joined.join("\n").as_bytes()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::schema::tests::memory;

    fn jsonl(events: &[Value]) -> String {
        events.iter().map(|e| e.to_string() + "\n").collect()
    }

    #[test]
    fn identical_duplicates_count_once_and_totals_are_grouped() {
        let mut other = memory("m2", 7);
        other["contributor"] = json!("@reviewer:example.org");
        let text = jsonl(&[memory("m1", 5), memory("m1", 5), other]);
        let mut ledger = Ledger::default();
        assert!(ledger.read(&text, 1).is_empty());
        let s = ledger.summary().unwrap();
        assert_eq!(s["token_total"], 12);
        assert_eq!(s["duplicates_ignored"], 1);
        assert_eq!(s["by_contributor"]["@impl:example.org"], 5);
        assert_eq!(s["by_contributor"]["@reviewer:example.org"], 7);
        assert_eq!(s["by_project"]["DEMO-05"], 12);
    }

    #[test]
    fn same_key_with_different_content_is_a_conflict() {
        let mut ledger = Ledger::default();
        let errors = ledger.read(&jsonl(&[memory("m1", 5), memory("m1", 6)]), 1);
        assert_eq!(
            errors,
            vec![LedgerError::Conflict {
                key: "m1".into(),
                first_line: 1,
                line: 2
            }]
        );
    }

    #[test]
    fn duplicate_detection_ignores_key_order() {
        let mut ledger = Ledger::default();
        let reordered: Value = serde_json::from_str(&{
            let mut fields: Vec<(String, Value)> = memory("m1", 5)
                .as_object()
                .unwrap()
                .clone()
                .into_iter()
                .collect();
            fields.reverse();
            format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(k, v)| format!("{}:{}", json!(k), v))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .unwrap();
        assert!(
            ledger
                .read(&jsonl(&[memory("m1", 5), reordered]), 1)
                .is_empty()
        );
        assert_eq!(ledger.duplicates, 1);
    }

    #[test]
    fn corrections_must_name_a_token_event() {
        let mut fix = memory("m2", -5);
        fix["corrects"] = json!("m1");
        let mut ledger = Ledger::default();
        assert!(
            ledger
                .read(&jsonl(&[memory("m1", 5), fix.clone()]), 1)
                .is_empty()
        );
        let s = ledger.summary().unwrap();
        assert_eq!(
            (s["token_total"].as_i64(), s["corrections"].as_i64()),
            (Some(0), Some(1))
        );

        fix["corrects"] = json!("missing");
        let mut ledger = Ledger::default();
        assert!(ledger.read(&jsonl(&[memory("m1", 5), fix]), 1).is_empty());
        assert!(matches!(
            ledger.summary().unwrap_err()[0],
            LedgerError::DanglingCorrection { .. }
        ));
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        let mut ledger = Ledger::default();
        assert!(
            ledger
                .read(&jsonl(&[memory("m1", i64::MAX), memory("m2", 1)]), 1)
                .is_empty()
        );
        assert!(matches!(
            ledger.summary().unwrap_err()[0],
            LedgerError::Overflow { line: 2 }
        ));
    }
}
