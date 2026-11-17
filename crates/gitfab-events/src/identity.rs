//! Identity events (Annex 3 §7.2) against the key events they attest. Each signed key event needs one
//! identity event naming the same task, event type, payload digest, key and signature; an identity event
//! that attests nothing in the set is an orphan.

use serde_json::Value;

use crate::{Error, invalid, schema};

/// Which key event a signed event is: task events by phase and status, release records as releases.
pub fn attested_type(event: &Value) -> Option<&'static str> {
    match event["schema_version"].as_str()? {
        schema::TASK => schema::key_event_type(event),
        schema::RELEASE => Some("release_issued"),
        _ => None,
    }
}

/// Checks a whole set of already validated and verified events. Returns failures with the index of the
/// event they concern.
pub fn check(events: &[Value]) -> Vec<(usize, Error)> {
    let identities: Vec<(usize, &Value)> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e["schema_version"] == schema::IDENTITY)
        .collect();
    let mut used = vec![false; identities.len()];
    let mut failures = Vec::new();
    for (i, event) in events.iter().enumerate() {
        let Some(kind) = attested_type(event) else {
            continue;
        };
        let sig = &event["signature"];
        if sig.is_null() {
            continue;
        }
        let digest = &sig["payload_digest"];
        let candidates: Vec<usize> = identities
            .iter()
            .enumerate()
            .filter(|(_, (_, id))| id["payload_digest"] == *digest)
            .map(|(n, _)| n)
            .collect();
        let Some(&n) = candidates.first() else {
            failures.push((i, Error::MissingIdentity(kind.to_owned())));
            continue;
        };
        for &n in &candidates {
            used[n] = true;
        }
        let id = identities[n].1;
        let mismatch = [
            ("event_type", id["event_type"].as_str() == Some(kind)),
            ("task_id", id["task_id"] == event["task_id"]),
            ("project_id", id["project_id"] == event["project_id"]),
            ("key_id", id["key_id"] == sig["key_id"]),
            (
                "signature_base64",
                id["signature_base64"] == sig["signature_base64"],
            ),
        ]
        .into_iter()
        .find(|(_, ok)| !ok);
        if let Some((field, _)) = mismatch {
            failures.push((
                identities[n].0,
                invalid(field, "does not match the attested event"),
            ));
        }
    }
    for (n, (index, _)) in identities.iter().enumerate() {
        if !used[n] {
            failures.push((*index, Error::OrphanIdentity));
        }
    }
    failures.sort_by_key(|(i, _)| *i);
    failures
}

/// Builds the identity event for a signed key event.
pub fn for_event(
    event: &Value,
    event_id: &str,
    timestamp: &str,
    actor_type: &str,
) -> Option<Value> {
    let kind = attested_type(event)?;
    let sig = event.get("signature").filter(|s| !s.is_null())?;
    let actor = event.get("actor_id").or_else(|| event.get("runner_id"))?;
    Some(serde_json::json!({
        "schema_version": schema::IDENTITY, "event_id": event_id, "timestamp": timestamp, "event_type": kind,
        "project_id": event["project_id"], "task_id": event["task_id"], "actor_id": actor, "actor_type": actor_type,
        "key_id": sig["key_id"], "algorithm": sig["algorithm"], "payload_digest": sig["payload_digest"],
        "signature_base64": sig["signature_base64"],
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::schema::tests::task;
    use crate::sign::Key;

    fn signed_created() -> Value {
        Key::generate("k")
            .unwrap()
            .sign(&task("intake", "succeeded"), "keys/k.pub")
            .unwrap()
    }

    #[test]
    fn a_matching_identity_event_passes_the_schema_and_the_check() {
        let event = signed_created();
        let id = for_event(&event, "id-1", "2026-11-12T03:00:01Z", "agent").unwrap();
        schema::validate(&id).unwrap();
        assert!(check(&[event, id]).is_empty());
    }

    #[test]
    fn a_signed_key_event_without_identity_fails() {
        assert_eq!(
            check(&[signed_created()]),
            vec![(0, Error::MissingIdentity("task_created".into()))]
        );
        assert!(check(&[task("plan", "running")]).is_empty());
    }

    #[test]
    fn mismatches_and_orphans_are_reported_on_the_identity_event() {
        let event = signed_created();
        let mut id = for_event(&event, "id-1", "2026-11-12T03:00:01Z", "agent").unwrap();
        id["event_type"] = json!("release_issued");
        assert_eq!(check(&[event.clone(), id])[0].0, 1);

        let mut orphan = for_event(&event, "id-2", "2026-11-12T03:00:01Z", "agent").unwrap();
        orphan["payload_digest"] = json!(format!("sha256:{}", "0".repeat(64)));
        let failures = check(&[event, orphan]);
        assert!(failures.contains(&(1, Error::OrphanIdentity)));
        assert!(failures.contains(&(0, Error::MissingIdentity("task_created".into()))));
    }
}
