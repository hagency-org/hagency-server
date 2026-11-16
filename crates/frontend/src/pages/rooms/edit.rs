use crate::types::RoomRecord;
use serde_json::{Value, json};

/// State PUT replaces the whole content. Only write dirty state types, preserving
/// the complete join-rules object and its extensions when that policy is edited.
pub(super) fn changes(
    room: &RoomRecord,
    original_join: &Value,
    name: &str,
    topic: &str,
    join_rule: &str,
    history: &str,
    allow_json: &str,
) -> Result<Vec<(&'static str, Value)>, String> {
    let mut changes = Vec::new();
    if room.room.name.as_deref().unwrap_or_default() != name {
        changes.push(("m.room.name", json!({"name":name})));
    }
    if room.room.topic.as_deref().unwrap_or_default() != topic {
        changes.push(("m.room.topic", json!({"topic":topic})));
    }
    if room.room.history_visibility.as_deref().unwrap_or("shared") != history {
        changes.push((
            "m.room.history_visibility",
            json!({"history_visibility":history}),
        ));
    }
    let original_rule = original_join
        .get("join_rule")
        .and_then(Value::as_str)
        .unwrap_or("invite");
    let original_allow = original_join
        .get("allow")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let allow: Value = serde_json::from_str(allow_json)
        .map_err(|_| "Allowed spaces must be a JSON array.".to_string())?;
    if !allow.is_array() {
        return Err("Allowed spaces must be a JSON array.".into());
    }
    if original_rule != join_rule || original_allow != allow {
        let version = room
            .room
            .version
            .as_deref()
            .unwrap_or("1")
            .parse::<u64>()
            .ok();
        if matches!(join_rule, "restricted" | "knock_restricted") {
            let minimum = if join_rule == "restricted" { 8 } else { 10 };
            if version.is_none_or(|v| v < minimum) {
                return Err(format!(
                    "{join_rule} requires room version {minimum} or later."
                ));
            }
            if allow.as_array().unwrap().is_empty() {
                return Err("Restricted joining requires at least one allowed space.".into());
            }
            for condition in allow.as_array().unwrap() {
                if condition.get("type").and_then(Value::as_str) != Some("m.room_membership")
                    || condition
                        .get("room_id")
                        .and_then(Value::as_str)
                        .is_none_or(|id| !id.starts_with('!'))
                {
                    return Err(
                        "Each allowed space needs type m.room_membership and a room_id.".into(),
                    );
                }
            }
        }
        let mut content = original_join
            .as_object()
            .cloned()
            .ok_or("The original join policy is unavailable. Reload before editing.")?;
        content.insert("join_rule".into(), json!(join_rule));
        if matches!(join_rule, "restricted" | "knock_restricted") {
            content.insert("allow".into(), allow);
        } else {
            content.remove("allow");
        }
        changes.push(("m.room.join_rules", Value::Object(content)));
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn room() -> RoomRecord {
        let mut room = RoomRecord::default();
        room.room.name = Some("Old".into());
        room.room.topic = Some("-".into());
        room.room.version = Some("12".into());
        room
    }
    #[test]
    fn changing_a_name_preserves_literal_topic_and_restricted_policy() {
        let content = json!({"join_rule":"restricted","allow":[{"type":"m.room_membership","room_id":"!space:server"}],"custom":true});
        let patch = changes(
            &room(),
            &content,
            "New",
            "-",
            "restricted",
            "shared",
            &content["allow"].to_string(),
        )
        .unwrap();
        assert_eq!(patch, vec![("m.room.name", json!({"name":"New"}))]);
    }
    #[test]
    fn policy_changes_preserve_extensions_and_validate_the_room_version() {
        let content = json!({"join_rule":"invite","custom":true});
        let allow = r#"[{"type":"m.room_membership","room_id":"!space:server"}]"#;
        let patch = changes(
            &room(),
            &content,
            "Old",
            "-",
            "knock_restricted",
            "shared",
            allow,
        )
        .unwrap();
        assert_eq!(patch[0].1["custom"], true);
        assert_eq!(patch[0].1["allow"][0]["room_id"], "!space:server");
        let mut old = room();
        old.room.version = Some("7".into());
        assert!(changes(&old, &content, "Old", "-", "restricted", "shared", allow).is_err());
        assert!(changes(&room(), &content, "Old", "-", "restricted", "shared", "[]").is_err());
    }
}
