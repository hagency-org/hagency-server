//! Reads current Matrix state from the host's existing privileged state API.
//! Human membership and Matrix power levels remain the domain authority.
use crate::{
    Error, Result,
    api::now_ms,
    domain::{AdminFacts, RoomFacts},
};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

pub type ReadState =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Vec<Value>>> + Send>> + Send + Sync>;
#[derive(Clone)]
pub struct Gateway {
    read: ReadState,
    service_mxid: String,
}
impl Gateway {
    pub fn new(read: ReadState, service_mxid: String) -> Self {
        Self { read, service_mxid }
    }
    pub async fn state(&self, room: &str) -> Result<Vec<Value>> {
        (self.read)(room.into()).await
    }
    pub async fn admin(&self, room: &str, actor: &str, space: Option<&str>) -> Result<AdminFacts> {
        let state = self.state(room).await?;
        let is_space = content(&state, "m.room.create", "").is_some_and(|c| c["type"] == "m.space");
        let powers = content(&state, "m.room.power_levels", "");
        let minimum = state_threshold(powers, "im.hagency.agent.policy")?
            .max(state_threshold(powers, "m.space.child")?);
        let linked = if let Some(space) = space {
            let parent = self.state(space).await?;
            content(&parent, "m.space.child", room)
                .is_some_and(|c| c["via"].as_array().is_some_and(|via| !via.is_empty()))
        } else {
            false
        };
        Ok(AdminFacts {
            actor_mxid: actor.into(),
            room_id: room.into(),
            observed_at_ms: now_ms(),
            joined: joined(&state, actor),
            can_manage_policy: level(powers, actor)? >= minimum,
            is_space,
            linked_space_id: if linked {
                space.map(str::to_owned)
            } else {
                None
            },
        })
    }
    pub async fn room(
        &self,
        room: &str,
        space: &str,
        owner: &str,
        puppet: Option<&str>,
    ) -> Result<RoomFacts> {
        if space.is_empty() {
            let state = self.state(room).await?;
            let puppet = puppet.ok_or(Error::Invalid("direct_puppet_required"))?;
            let valid = direct_private(&state, owner, puppet);
            return Ok(RoomFacts {
                owner_direct_valid: valid,
                owner_mxid: owner.into(),
                room_id: room.into(),
                space_id: String::new(),
                observed_at_ms: now_ms(),
                owner_in_room: joined(&state, owner),
                owner_in_space: false,
                room_in_space: false,
                service_can_invite: false,
                puppet_mxid: Some(puppet.into()),
                puppet_in_room: joined(&state, puppet),
                encrypted: content(&state, "m.room.encryption", "").is_some(),
            });
        }
        let (room_state, space_state) = tokio::try_join!(self.state(room), self.state(space))?;
        let powers = content(&room_state, "m.room.power_levels", "");
        Ok(RoomFacts {
            owner_direct_valid: false,
            owner_mxid: owner.into(),
            room_id: room.into(),
            space_id: space.into(),
            observed_at_ms: now_ms(),
            room_in_space: content(&space_state, "m.space.child", room)
                .is_some_and(|c| c["via"].as_array().is_some_and(|via| !via.is_empty())),
            owner_in_space: joined(&space_state, owner),
            owner_in_room: joined(&room_state, owner),
            service_can_invite: joined(&room_state, &self.service_mxid)
                && level(powers, &self.service_mxid)?
                    >= power_integer(power_object(powers)?.and_then(|p| p.get("invite")), 0)?,
            puppet_mxid: puppet.map(str::to_owned),
            puppet_in_room: puppet.is_some_and(|p| joined(&room_state, p)),
            encrypted: content(&room_state, "m.room.encryption", "").is_some(),
        })
    }
    /// Agent conversation created by the applicant for the service and one puppet.
    /// The puppet may be absent before native approval. The applicant may invite
    /// other people to use the Agent; public join rules, guest access and
    /// encryption never qualify this binding.
    pub(crate) async fn pool_room(&self, room: &str, owner: &str) -> Result<(i64, Vec<Value>)> {
        let state = self.pool_room_state(room, owner).await?;
        if !joined(&state, &self.service_mxid) {
            return Err(Error::Unavailable("pool_agent_service_join_pending"));
        }
        Ok((now_ms(), state))
    }
    pub(crate) async fn pool_service_invited(&self, room: &str, owner: &str) -> Result<bool> {
        let state = self.pool_room_state(room, owner).await?;
        Ok(content(&state, "m.room.member", &self.service_mxid)
            .is_some_and(|c| c["membership"] == "invite"))
    }
    async fn pool_room_state(&self, room: &str, owner: &str) -> Result<Vec<Value>> {
        let state = self.state(room).await?;
        if !pool_conversation(&state, owner)
            || content(&state, "m.room.encryption", "").is_some()
            || level(content(&state, "m.room.power_levels", ""), &self.service_mxid)?
                < power_integer(power_object(content(&state, "m.room.power_levels", ""))?.and_then(|p| p.get("invite")), 0)?
        {
            return Err(Error::Unauthorized("pool_agent_private_room_required"));
        }
        Ok(state)
    }
    /// One state response per room: all setup authority and metadata facts use
    /// those same observations, never independently fetched permission snapshots.
    pub(crate) async fn project_setup(
        &self,
        space: &str,
        room: &str,
        actor: &str,
    ) -> Result<(AdminFacts, AdminFacts, RoomFacts, Vec<Value>, Vec<Value>)> {
        let (ss, rs) = tokio::try_join!(self.state(space), self.state(room))?;
        let observed_at_ms = now_ms();
        let linked = content(&ss, "m.space.child", room)
            .is_some_and(|c| c["via"].as_array().is_some_and(|v| !v.is_empty()));
        let admin = |events: &[Value], id: &str, parent: Option<String>| -> Result<AdminFacts> {
            let powers = content(events, "m.room.power_levels", "");
            Ok(AdminFacts {
                actor_mxid: actor.into(),
                room_id: id.into(),
                observed_at_ms,
                joined: joined(events, actor),
                can_manage_policy: level(powers, actor)?
                    >= state_threshold(powers, "im.hagency.agent.policy")?
                        .max(state_threshold(powers, "m.space.child")?),
                is_space: content(events, "m.room.create", "")
                    .is_some_and(|c| c["type"] == "m.space"),
                linked_space_id: parent,
            })
        };
        let sf = admin(&ss, space, None)?;
        let rf = admin(&rs, room, linked.then(|| space.into()))?;
        let powers = content(&rs, "m.room.power_levels", "");
        let facts = RoomFacts {
            owner_direct_valid: false,
            owner_mxid: actor.into(),
            room_id: room.into(),
            space_id: space.into(),
            observed_at_ms,
            room_in_space: linked,
            owner_in_space: joined(&ss, actor),
            owner_in_room: joined(&rs, actor),
            service_can_invite: joined(&rs, &self.service_mxid)
                && level(powers, &self.service_mxid)?
                    >= power_integer(power_object(powers)?.and_then(|p| p.get("invite")), 0)?,
            puppet_mxid: None,
            puppet_in_room: false,
            encrypted: content(&rs, "m.room.encryption", "").is_some(),
        };
        Ok((sf, rf, facts, ss, rs))
    }
    pub async fn service_invited(&self, room: &str) -> Result<bool> {
        Ok(content(
            &self.state(room).await?,
            "m.room.member",
            &self.service_mxid,
        )
        .is_some_and(|c| c["membership"] == "invite"))
    }
    pub(crate) fn service_mxid(&self) -> &str {
        &self.service_mxid
    }
    pub async fn delivery(
        &self,
        room: &str,
        space: &str,
        owner: &str,
        requester: &str,
        puppet: &str,
    ) -> Result<crate::transport::DeliveryFacts> {
        self.delivery_for(room, space, owner, requester, puppet, false).await
    }
    pub(crate) async fn shared_delivery(&self, room: &str, space: &str, owner: &str, requester: &str, puppet: &str) -> Result<crate::transport::DeliveryFacts> {
        self.delivery_for(room, space, owner, requester, puppet, true).await
    }
    async fn delivery_for(&self, room: &str, space: &str, owner: &str, requester: &str, puppet: &str, pool: bool) -> Result<crate::transport::DeliveryFacts> {
        if space.is_empty() {
            let state = self.state(room).await?;
            return Ok(crate::transport::DeliveryFacts {
                owner_direct_valid: direct_private(&state, owner, puppet)
                    || pool && pool_conversation(&state, owner),
                owner_mxid: owner.into(),
                requester_mxid: requester.into(),
                puppet_mxid: puppet.into(),
                room_id: room.into(),
                space_id: String::new(),
                observed_at_ms: now_ms(),
                owner_in_room: joined(&state, owner),
                owner_in_space: false,
                room_in_space: false,
                requester_in_room: joined(&state, requester),
                puppet_in_room: joined(&state, puppet),
                puppet_can_send_message: message_permission(
                    content(&state, "m.room.power_levels", ""),
                    puppet,
                )?,
                encrypted: content(&state, "m.room.encryption", "").is_some(),
            });
        }
        let (room_state, space_state) = tokio::try_join!(self.state(room), self.state(space))?;
        Ok(crate::transport::DeliveryFacts {
            owner_direct_valid: false,
            owner_mxid: owner.into(),
            requester_mxid: requester.into(),
            puppet_mxid: puppet.into(),
            room_id: room.into(),
            space_id: space.into(),
            observed_at_ms: now_ms(),
            owner_in_space: joined(&space_state, owner),
            owner_in_room: joined(&room_state, owner),
            requester_in_room: joined(&room_state, requester),
            puppet_in_room: joined(&room_state, puppet),
            puppet_can_send_message: message_permission(
                content(&room_state, "m.room.power_levels", ""),
                puppet,
            )?,
            room_in_space: content(&space_state, "m.space.child", room)
                .is_some_and(|c| c["via"].as_array().is_some_and(|via| !via.is_empty())),
            encrypted: content(&room_state, "m.room.encryption", "").is_some(),
        })
    }
    /// Metadata is returned only from a snapshot in which the actor is joined.
    pub async fn member_metadata(&self, room: &str, actor: &str) -> Result<Option<Value>> {
        let state = self.state(room).await?;
        if !joined(&state, actor) {
            return Ok(None);
        }
        let name = content(&state, "m.room.name", "")
            .and_then(|c| c.get("name"))
            .and_then(Value::as_str);
        let topic = content(&state, "m.room.topic", "")
            .and_then(|c| c.get("topic"))
            .and_then(Value::as_str);
        Ok(Some(serde_json::json!({"name":name,"topic":topic})))
    }
    pub async fn visible(&self, space: &str, owner: &str) -> Result<bool> {
        Ok(joined(&self.state(space).await?, owner))
    }
    pub async fn puppet_invited(&self, room: &str, puppet: &str) -> Result<bool> {
        Ok(content(&self.state(room).await?, "m.room.member", puppet)
            .is_some_and(|c| c["membership"] == "invite"))
    }
}
/// Invite-only, non-guest, non-public conversation joined by its applicant.
/// Other members are allowed; each sender's membership is checked per message.
fn pool_conversation(state: &[Value], owner: &str) -> bool {
    content(state, "m.room.create", "").is_some_and(|c| c["type"] != "m.space")
        && content(state, "m.room.join_rules", "").is_some_and(|c| c["join_rule"] == "invite")
        && !content(state, "m.room.guest_access", "").is_some_and(|c| c["guest_access"] == "can_join")
        && content(state, "m.room.history_visibility", "").is_some_and(|c| matches!(c["history_visibility"].as_str(), Some("joined" | "invited" | "shared")))
        && joined(state, owner)
}
fn direct_private(state: &[Value], owner: &str, puppet: &str) -> bool {
    content(state, "m.room.create", "").is_some_and(|c| c["type"] != "m.space")
        && content(state, "m.room.join_rules", "").is_some_and(|c| c["join_rule"] == "invite")
        && !content(state, "m.room.guest_access", "")
            .is_some_and(|c| c["guest_access"] == "can_join")
        && content(state, "m.room.history_visibility", "").is_some_and(|c| {
            matches!(
                c["history_visibility"].as_str(),
                Some("joined" | "invited" | "shared")
            )
        })
        && joined(state, owner)
        && content(state, "m.room.member", puppet)
            .is_some_and(|c| c["membership"] == "join" || c["membership"] == "invite")
        && state
            .iter()
            .filter(|e| {
                e["type"] == "m.room.member"
                    && (e["content"]["membership"] == "join"
                        || e["content"]["membership"] == "invite")
            })
            .all(|e| e["state_key"] == owner || e["state_key"] == puppet)
}
fn content<'a>(state: &'a [Value], kind: &str, key: &str) -> Option<&'a Value> {
    state
        .iter()
        .find(|event| event["type"] == kind && event["state_key"] == key)
        .and_then(|e| e.get("content"))
}
fn joined(state: &[Value], user: &str) -> bool {
    content(state, "m.room.member", user).is_some_and(|c| c["membership"] == "join")
}
fn power_object(powers: Option<&Value>) -> Result<Option<&serde_json::Map<String, Value>>> {
    powers
        .map(|v| {
            v.as_object()
                .ok_or(Error::Unavailable("invalid_matrix_power_levels"))
        })
        .transpose()
}
fn power_map<'a>(
    powers: Option<&'a serde_json::Map<String, Value>>,
    name: &str,
) -> Result<Option<&'a serde_json::Map<String, Value>>> {
    powers
        .and_then(|p| p.get(name))
        .map(|v| {
            v.as_object()
                .ok_or(Error::Unavailable("invalid_matrix_power_levels"))
        })
        .transpose()
}
fn power_integer(value: Option<&Value>, default: i64) -> Result<i64> {
    match value {
        None => Ok(default),
        Some(v) => v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
            .ok_or(Error::Unavailable("invalid_matrix_power_levels")),
    }
}
fn level(powers: Option<&Value>, user: &str) -> Result<i64> {
    let powers = power_object(powers)?;
    let default = power_integer(powers.and_then(|p| p.get("users_default")), 0)?;
    power_integer(
        power_map(powers, "users")?.and_then(|p| p.get(user)),
        default,
    )
}
fn state_threshold(powers: Option<&Value>, event: &str) -> Result<i64> {
    let powers = power_object(powers)?;
    let default = power_integer(powers.and_then(|p| p.get("state_default")), 50)?;
    power_integer(
        power_map(powers, "events")?.and_then(|p| p.get(event)),
        default,
    )
}
// Missing fields use Matrix defaults. Present but invalid fields never silently
// fall back; valid legacy decimal strings match Palpo's v1 power-level parser.
fn message_permission(powers: Option<&Value>, puppet: &str) -> Result<bool> {
    let object = power_object(powers)?;
    let default = power_integer(object.and_then(|p| p.get("events_default")), 0)?;
    let required = power_integer(
        power_map(object, "events")?.and_then(|p| p.get("m.room.message")),
        default,
    )?;
    Ok(level(powers, puppet)? >= required)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pool_service_invitation_requires_invite_only_authorized_room() {
        let read: ReadState = Arc::new(|room| Box::pin(async move {
            let mut events = vec![
                serde_json::json!({"type":"m.room.create","state_key":"","content":{}}),
                serde_json::json!({"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}}),
                serde_json::json!({"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}}),
                serde_json::json!({"type":"m.room.member","state_key":"@owner:test","content":{"membership":"join"}}),
                serde_json::json!({"type":"m.room.member","state_key":"@service:test","content":{"membership":if room=="joined" {"join"} else {"invite"}}}),
            ];
            match room.as_str() {
                "public" => events[1]["content"]["join_rule"] = serde_json::json!("public"),
                "member" => events.push(serde_json::json!({"type":"m.room.member","state_key":"@member:test","content":{"membership":"invite"}})),
                "guest" => events.push(serde_json::json!({"type":"m.room.guest_access","state_key":"","content":{"guest_access":"can_join"}})),
                "absent" => events[3]["content"]["membership"] = serde_json::json!("leave"),
                "encrypted" => events.push(serde_json::json!({"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}})),
                "power" => events.push(serde_json::json!({"type":"m.room.power_levels","state_key":"","content":{"invite":50}})),
                _ => {},
            }
            Ok(events)
        }));
        let gateway = Gateway::new(read, "@service:test".into());
        assert!(gateway.pool_service_invited("valid", "@owner:test").await.unwrap());
        assert!(matches!(gateway.pool_room("valid", "@owner:test").await,
            Err(Error::Unavailable("pool_agent_service_join_pending"))));
        assert!(!gateway.pool_service_invited("joined", "@owner:test").await.unwrap());
        assert!(gateway.pool_room("joined", "@owner:test").await.is_ok());
        // Other people the applicant invites may share the conversation.
        assert!(gateway.pool_service_invited("member", "@owner:test").await.unwrap());
        for room in ["public", "guest", "absent", "encrypted", "power"] {
            assert!(gateway.pool_service_invited(room, "@owner:test").await.is_err(), "{room}");
        }
    }
    #[tokio::test]
    async fn membership_and_explicit_power_thresholds_are_independent() {
        let read: ReadState = Arc::new(|room| {
            Box::pin(async move {
                Ok(vec![
                    serde_json::json!({"type":"m.room.create","state_key":"","content":{"type":if room=="!space:test"{"m.space"}else{"room"}}}),
                    serde_json::json!({"type":"m.room.member","state_key":"@alice:test","content":{"membership":"join"}}),
                    serde_json::json!({"type":"m.room.power_levels","state_key":"","content":{"users":{"@alice:test":50},"events":{"im.hagency.agent.policy":60}}}),
                ])
            })
        });
        let facts = Gateway::new(read, "@_hagency_service:test".into())
            .admin("!space:test", "@alice:test", None)
            .await
            .unwrap();
        assert!(facts.joined && facts.is_space);
        assert!(!facts.can_manage_policy);
    }
    #[test]
    fn message_power_uses_non_state_threshold_and_fails_closed_on_bad_state() {
        let puppet = "@agent:test";
        assert!(message_permission(None, puppet).unwrap());
        assert!(
            message_permission(Some(&serde_json::json!({"state_default":100})), puppet).unwrap()
        );
        assert!(
            !message_permission(Some(&serde_json::json!({"events_default":10})), puppet).unwrap()
        );
        assert!(
            message_permission(
                Some(&serde_json::json!({"users_default":20,"events_default":10})),
                puppet
            )
            .unwrap()
        );
        assert!(!message_permission(Some(&serde_json::json!({"users_default":20,"users":{"@agent:test":0},"events":{"m.room.message":1}})), puppet).unwrap());
        assert!(message_permission(Some(&serde_json::json!({"users":{"@agent:test":30},"events":{"m.room.message":30},"events_default":100})), puppet).unwrap());
        // Legacy Matrix state can contain decimal integer strings; malformed
        // current state is unavailable rather than silently defaulting to 0.
        assert!(
            message_permission(
                Some(&serde_json::json!({"users_default":"10","events_default":"10"})),
                puppet
            )
            .unwrap()
        );
        for malformed in [
            serde_json::json!({"events_default":"bad"}),
            serde_json::json!({"events":[]}),
            serde_json::json!({"users_default":true}),
            serde_json::json!({"users":{"@agent:test":null}}),
        ] {
            assert!(message_permission(Some(&malformed), puppet).is_err());
        }
    }
    #[tokio::test]
    async fn joined_puppet_can_still_be_denied_message_sending() {
        let read: ReadState = Arc::new(|room| {
            Box::pin(async move {
                let mut state = vec![
                    serde_json::json!({"type":"m.room.member","state_key":"@owner:test","content":{"membership":"join"}}),
                    serde_json::json!({"type":"m.room.member","state_key":"@requester:test","content":{"membership":"join"}}),
                    serde_json::json!({"type":"m.room.member","state_key":"@agent:test","content":{"membership":"join"}}),
                    serde_json::json!({"type":"m.room.power_levels","state_key":"","content":{"events":{"m.room.message":50}}}),
                ];
                if room == "!space:test" {
                    state.push(serde_json::json!({"type":"m.space.child","state_key":"!room:test","content":{"via":["test"]}}));
                }
                Ok(state)
            })
        });
        let facts = Gateway::new(read, "@service:test".into())
            .delivery(
                "!room:test",
                "!space:test",
                "@owner:test",
                "@requester:test",
                "@agent:test",
            )
            .await
            .unwrap();
        assert!(
            facts.puppet_in_room
                && facts.owner_in_room
                && facts.owner_in_space
                && facts.requester_in_room
                && facts.room_in_space
        );
        assert!(!facts.puppet_can_send_message);
    }
}

#[cfg(test)]
mod power_regressions {
    use super::*;
    #[tokio::test]
    async fn legacy_power_strings_never_lower_admin_or_invite_thresholds() {
        let read: ReadState = Arc::new(|room| {
            Box::pin(async move {
                Ok(vec![
            serde_json::json!({"type":"m.room.create","state_key":"","content":{"type":"m.space"}}),
            serde_json::json!({"type":"m.room.member","state_key":"@actor:test","content":{"membership":"join"}}),
            serde_json::json!({"type":"m.room.member","state_key":"@service:test","content":{"membership":"join"}}),
            serde_json::json!({"type":"m.space.child","state_key":"!room:test","content":{"via":["test"]}}),
            serde_json::json!({"type":"m.room.power_levels","state_key":"","content":{"users":{"@actor:test":50,"@service:test":"0"},"users_default":100,"state_default":"100","invite":"100"}}),
        ].into_iter().filter(|e|room=="!space:test" || e["type"]!="m.space.child").collect())
            })
        });
        let gateway = Gateway::new(read, "@service:test".into());
        assert!(
            !gateway
                .admin("!space:test", "@actor:test", None)
                .await
                .unwrap()
                .can_manage_policy
        );
        assert!(
            !gateway
                .room("!room:test", "!space:test", "@actor:test", None)
                .await
                .unwrap()
                .service_can_invite
        );
        assert_eq!(
            level(
                Some(&serde_json::json!({"users":{"@actor:test":"0"},"users_default":100})),
                "@actor:test"
            )
            .unwrap(),
            0
        );
        for invalid in [
            serde_json::json!({"state_default":"oops"}),
            serde_json::json!({"events":[]}),
            serde_json::json!({"users":{"@actor:test":null}}),
        ] {
            assert!(
                state_threshold(Some(&invalid), "m.space.child").is_err()
                    || level(Some(&invalid), "@actor:test").is_err()
            );
        }
    }
}
