//! Owner-scoped crypto receive operations. No room, message or media authority.
use super::*;
pub(crate) const CAPABILITY: &str = "agent-crypto-owner-receive-v1";
pub(crate) const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub(crate) const FILTER: &str =
    r#"{"room":{"rooms":[]},"presence":{"types":[]},"account_data":{"types":[]}}"#;
pub(crate) fn route(method: &str, path: &str) -> Result<Operation> {
    if (method, path) == ("GET", "/_matrix/client/v3/sync") {
        return Ok(Operation::ReceiveSync);
    }
    if (method, path) == ("POST", "/_matrix/client/v3/keys/claim") {
        return Ok(Operation::OwnerClaim);
    }
    Operation::route(method, path).map(|op| match op {
        Operation::Query => Operation::OwnerQuery,
        Operation::Signatures => Operation::OwnerSignatures,
        other => other,
    })
}
pub(crate) fn sync_query(query: Option<&str>) -> Result<Value> {
    let mut seen = std::collections::BTreeSet::new();
    let mut since = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if !seen.insert(key.to_string()) {
            return Err(invalid());
        }
        match key.as_ref() {
            "since"
                if !value.is_empty()
                    && value.len() <= 4096
                    && !value.chars().any(char::is_control) =>
            {
                since = Some(value.to_string())
            }
            "timeout" if value.parse::<u64>().is_ok_and(|n| n <= 5000) => {}
            "set_presence" if matches!(value.as_ref(), "online" | "offline" | "unavailable") => {}
            "full_state" if value == "false" => {}
            "use_state_after" if value == "true" => {}
            _ => return Err(invalid()),
        }
    }
    Ok(json!({"since":since}))
}
pub(crate) fn fingerprints(
    op: Operation,
    body: &Value,
    user: &str,
    device: &str,
    owner: &str,
) -> Result<Vec<(&'static str, String)>> {
    match op {
        Operation::OwnerClaim => {
            object(body, &["one_time_keys", "timeout"])?;
            let users = body["one_time_keys"]
                .as_object()
                .filter(|o| o.len() == 1 && o.contains_key(owner))
                .ok_or_else(invalid)?;
            let devices = users[owner]
                .as_object()
                .filter(|o| !o.is_empty() && o.len() <= 64)
                .ok_or_else(invalid)?;
            if devices.iter().any(|(id, v)| {
                id.is_empty()
                    || id.len() > 255
                    || id.chars().any(char::is_control)
                    || v != "signed_curve25519"
            }) {
                return Err(invalid());
            }
            if body
                .get("timeout")
                .is_some_and(|v| v.as_u64().is_none_or(|n| n > 10000))
            {
                return Err(invalid());
            }
        }
        Operation::ReceiveSync => {
            let o = object(body, &["since"])?;
            if let Some(v) = o.get("since").filter(|v| !v.is_null()) {
                let value = v.as_str().ok_or_else(invalid)?;
                if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
                    return Err(invalid());
                }
            }
        }
        Operation::OwnerQuery => {
            let o = object(body, &["device_keys", "timeout"])?;
            if o.get("timeout")
                .is_some_and(|v| v.as_u64().is_none_or(|n| n > 5000))
            {
                return Err(invalid());
            }
            let users = body["device_keys"]
                .as_object()
                .filter(|o| !o.is_empty() && o.len() <= 2)
                .ok_or_else(invalid)?;
            for (u, ids) in users {
                let ids = ids.as_array().ok_or_else(invalid)?;
                if u == user {
                    if ids.len() > 1 || ids.iter().any(|id| id.as_str() != Some(device)) {
                        return Err(invalid());
                    }
                } else if u == owner {
                    if ids.len() > 64
                        || ids.iter().any(|id| {
                            id.as_str().is_none_or(|id| {
                                id.is_empty() || id.len() > 255 || id.chars().any(char::is_control)
                            })
                        })
                    {
                        return Err(invalid());
                    }
                } else {
                    return Err(invalid());
                }
            }
        }
        Operation::OwnerSignatures => {
            let users = body
                .as_object()
                .filter(|o| o.len() == 1)
                .ok_or_else(invalid)?;
            if users.contains_key(user) {
                return super::fingerprints(Operation::Signatures, body, user, device);
            }
            let keys = users
                .get(owner)
                .and_then(Value::as_object)
                .filter(|o| o.len() == 1)
                .ok_or_else(invalid)?;
            for (target, value) in keys {
                object(value, &["user_id", "usage", "keys", "signatures"])?;
                if value["user_id"] != owner || value["usage"] != json!(["master"]) {
                    return Err(invalid());
                }
                let keys = value["keys"]
                    .as_object()
                    .filter(|o| o.len() == 1)
                    .ok_or_else(invalid)?;
                let (id, key) = keys.iter().next().ok_or_else(invalid)?;
                let key = public_key(key)?;
                if target != key || id != &format!("ed25519:{key}") {
                    return Err(invalid());
                }
                let sigs = value["signatures"][user]
                    .as_object()
                    .filter(|o| o.len() == 1)
                    .ok_or_else(invalid)?;
                for (key, sig) in sigs {
                    public_key(&Value::String(
                        key.strip_prefix("ed25519:").ok_or_else(invalid)?.into(),
                    ))?;
                    let sig = sig.as_str().ok_or_else(invalid)?;
                    if STANDARD_NO_PAD.decode(sig).map_err(|_| invalid())?.len() != 64 {
                        return Err(invalid());
                    }
                }
            }
        }
        _ => return super::fingerprints(op, body, user, device),
    }
    Ok(Vec::new())
}
pub(crate) fn signing_digest(
    op: Operation,
    body: &Value,
    user: &str,
    owner: &str,
) -> Result<Option<String>> {
    if !matches!(op, Operation::OwnerSignatures) || body.get(owner).is_none() {
        return Ok(None);
    }
    let value = body[owner]
        .as_object()
        .and_then(|o| o.values().next())
        .ok_or_else(invalid)?;
    let key = value["signatures"][user]
        .as_object()
        .and_then(|o| o.keys().next())
        .and_then(|s| s.strip_prefix("ed25519:"))
        .ok_or_else(invalid)?;
    Ok(Some(digest(&(user, "user_signing", key))?))
}
pub(crate) fn sync_response(value: &Value) -> Result<()> {
    let bad = || Error::Unavailable("invalid_agent_crypto_receive_response");
    let allowed = [
        "next_batch",
        "to_device",
        "device_lists",
        "device_one_time_keys_count",
        "device_unused_fallback_key_types",
        "org.matrix.msc2732.device_unused_fallback_key_types",
        "rooms",
        "presence",
        "account_data",
    ];
    let o = value
        .as_object()
        .filter(|o| o.keys().all(|k| allowed.contains(&k.as_str())))
        .ok_or_else(bad)?;
    if value["next_batch"]
        .as_str()
        .is_none_or(|s| s.is_empty() || s.len() > 4096 || s.chars().any(char::is_control))
    {
        return Err(bad());
    }
    if let Some(rooms) = o.get("rooms") {
        let rooms = rooms.as_object().ok_or_else(bad)?;
        if rooms.iter().any(|(name, v)| {
            !matches!(name.as_str(), "join" | "invite" | "leave" | "knock")
                || v.as_object().is_none_or(|o| !o.is_empty())
        }) {
            return Err(bad());
        }
    }
    for name in ["presence", "account_data"] {
        if let Some(value) = o.get(name)
            && (value.as_object().is_none_or(|o| o.len() != 1)
                || value["events"].as_array().is_none_or(|e| !e.is_empty()))
        {
            return Err(bad());
        }
    }
    if let Some(device) = o.get("to_device") {
        if device.as_object().is_none_or(|o| o.len() != 1) {
            return Err(bad());
        }
        let events = device["events"]
            .as_array()
            .filter(|e| e.len() <= 256)
            .ok_or_else(bad)?;
        for event in events {
            if event.as_object().is_none_or(|o| {
                o.keys()
                    .any(|k| !matches!(k.as_str(), "type" | "sender" | "content"))
            }) || event["type"]
                .as_str()
                .is_none_or(|s| s.is_empty() || s.len() > 255)
                || event["sender"]
                    .as_str()
                    .is_none_or(|s| !s.starts_with('@') || !s.contains(':') || s.len() > 255)
                || !event["content"].is_object()
            {
                return Err(bad());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receiver_is_owner_scoped_and_never_adds_room_send_login_or_peer_claim_authority() {
        for (method, path) in [
            (
                "PUT",
                "/_matrix/client/v3/sendToDevice/m.room.encrypted/txn",
            ),
            ("POST", "/_matrix/client/v3/login"),
            ("GET", "/_matrix/client/v3/rooms/r/messages"),
            ("GET", "/_matrix/client/v3/room_keys/keys"),
            ("PUT", "/_matrix/client/v3/rooms/r/send/m.room.encrypted/t"),
        ] {
            assert!(route(method, path).is_err());
        }
        assert!(matches!(
            route("GET", "/_matrix/client/v3/sync").unwrap(),
            Operation::ReceiveSync
        ));
        let user = "@_hagency_agent:test";
        let owner = "@owner:test";
        assert!(matches!(
            route("POST", "/_matrix/client/v3/keys/claim").unwrap(),
            Operation::OwnerClaim
        ));
        assert!(
            fingerprints(
                Operation::OwnerClaim,
                &json!({"one_time_keys":{owner:{"SIGNED":"signed_curve25519"}},"timeout":10000}),
                user,
                "DEVICE",
                owner
            )
            .is_ok()
        );
        for value in [
            json!({"one_time_keys":{"@foreign:test":{"DEVICE":"signed_curve25519"}}}),
            json!({"one_time_keys":{owner:{"DEVICE":"curve25519"}}}),
            json!({"one_time_keys":{user:{"DEVICE":"signed_curve25519"}}}),
        ] {
            assert!(fingerprints(Operation::OwnerClaim, &value, user, "DEVICE", owner).is_err());
        }

        assert!(
            fingerprints(
                Operation::OwnerQuery,
                &json!({"device_keys":{user:["DEVICE"],owner:[]}}),
                user,
                "DEVICE",
                owner
            )
            .is_ok()
        );
        for body in [
            json!({"device_keys":{"@other:test":[]}}),
            json!({"device_keys":{user:["OTHER"]}}),
            json!({"device_keys":{owner:[]},"user_id":owner}),
            json!({"device_keys":{owner:[]},"timeout":5001}),
        ] {
            assert!(fingerprints(Operation::OwnerQuery, &body, user, "DEVICE", owner).is_err());
        }
    }
    #[test]
    fn receiver_sync_query_has_no_caller_filter_identity_presence_or_wait_control() {
        for query in [
            "filter=arbitrary",
            "user_id=%40owner%3Atest",
            "device_id=OTHER",
            "since=a&since=b",
            "timeout=5001",
            "full_state=true",
            "use_state_after=false",
            "use_state_after=arbitrary",
            "since=%00",
            "access_token=token",
            "since=",
        ] {
            assert!(sync_query(Some(query)).is_err(), "accepted {query}");
        }
        assert_eq!(
            sync_query(Some(
                "since=opaque%26user_id%3Dother&timeout=10&set_presence=online&full_state=false&use_state_after=true"
            ))
            .unwrap(),
            json!({"since":"opaque&user_id=other"})
        );
        assert_eq!(sync_query(None).unwrap(), json!({"since":null}));
        assert_eq!(
            serde_json::from_str::<Value>(FILTER).unwrap()["room"]["rooms"],
            json!([])
        );
    }
    #[test]
    fn receiver_rejects_room_presence_account_history_and_malformed_sync_responses() {
        let valid = json!({"next_batch":"s_opaque","to_device":{"events":[{"sender":"@owner:test","type":"m.room.encrypted","content":{"algorithm":"m.olm.v1.curve25519-aes-sha2"}}]},"rooms":{"join":{},"invite":{},"leave":{}},"presence":{"events":[]},"account_data":{"events":[]}});
        sync_response(&valid).unwrap();
        for body in [
            json!({}),
            json!({"next_batch":""}),
            json!({"next_batch":"s","rooms":{"join":{"!private:test":{}}}}),
            json!({"next_batch":"s","presence":{"events":[{}]}}),
            json!({"next_batch":"s","account_data":{"events":[{}]}}),
            json!({"next_batch":"s","unexpected":["secret"]}),
            json!({"next_batch":"s","to_device":{"events":[{"sender":"owner","type":"m.room.encrypted","content":{}}]}}),
        ] {
            assert!(sync_response(&body).is_err());
        }
    }
    #[test]
    fn receiver_signs_only_owner_master_with_the_original_agent_user_signer() {
        let user = "@_hagency_agent:test";
        let owner = "@owner:test";
        let public = STANDARD_NO_PAD.encode([4; 32]);
        let signer = STANDARD_NO_PAD.encode([5; 32]);
        let signature = STANDARD_NO_PAD.encode([6; 64]);
        let body = json!({owner:{&public:{"user_id":owner,"usage":["master"],"keys":{format!("ed25519:{public}"):public},"signatures":{user:{format!("ed25519:{signer}"):signature},owner:{"ed25519:existing":"existing"}}}}});
        fingerprints(Operation::OwnerSignatures, &body, user, "DEVICE", owner).unwrap();
        assert_eq!(
            signing_digest(Operation::OwnerSignatures, &body, user, owner).unwrap(),
            Some(digest(&(user, "user_signing", &signer)).unwrap())
        );
        for change in ["self_signing", "user_signing"] {
            let mut changed = body.clone();
            changed[owner][&public]["usage"] = json!([change]);
            assert!(
                fingerprints(Operation::OwnerSignatures, &changed, user, "DEVICE", owner).is_err()
            );
        }
        let mut wrong = body.clone();
        wrong[owner][&public]["user_id"] = json!("@other:test");
        assert!(fingerprints(Operation::OwnerSignatures, &wrong, user, "DEVICE", owner).is_err());
        let mut fake = body;
        fake[owner][&public]["signatures"][user] = json!({format!("ed25519:{signer}"):"invalid"});
        assert!(fingerprints(Operation::OwnerSignatures, &fake, user, "DEVICE", owner).is_err());
    }
}
