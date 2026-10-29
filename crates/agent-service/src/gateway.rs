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
            can_manage_policy: at_least(&state, actor, minimum)?,
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
                && at_least(
                    &room_state,
                    &self.service_mxid,
                    power_integer(power_object(powers)?.and_then(|p| p.get("invite")), 0)?,
                )?,
            puppet_mxid: puppet.map(str::to_owned),
            puppet_in_room: puppet.is_some_and(|p| joined(&room_state, p)),
            encrypted: content(&room_state, "m.room.encryption", "").is_some(),
        })
    }
    pub async fn service_invited(&self, room: &str) -> Result<bool> {
        Ok(content(
            &self.state(room).await?,
            "m.room.member",
            &self.service_mxid,
        )
        .is_some_and(|c| c["membership"] == "invite"))
    }
    pub async fn delivery(
        &self,
        room: &str,
        space: &str,
        owner: &str,
        requester: &str,
        puppet: &str,
    ) -> Result<crate::transport::DeliveryFacts> {
        if space.is_empty() {
            let state = self.state(room).await?;
            return Ok(crate::transport::DeliveryFacts {
                owner_direct_valid: direct_private(&state, owner, puppet),
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
                puppet_can_send_message: can_send(&state, puppet)?,
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
            puppet_can_send_message: can_send(&room_state, puppet)?,
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
    /// Navigation may reveal an invited room to its invitee. This is deliberately
    /// separate from execution/board/Agent authority, which still requires join.
    pub async fn room_navigation_metadata(
        &self,
        room: &str,
        space: &str,
        actor: &str,
    ) -> Result<Option<Value>> {
        Ok(self.room_navigation_facts(room, space, actor).await?.1)
    }
    pub(crate) async fn room_navigation_facts(
        &self,
        room: &str,
        space: &str,
        actor: &str,
    ) -> Result<(bool, Option<Value>)> {
        let (room_state, space_state) = tokio::try_join!(self.state(room), self.state(space))?;
        Ok((
            room_navigation_candidate(&room_state, &space_state, room),
            room_navigation_metadata(&room_state, &space_state, room, actor),
        ))
    }
    pub async fn visible(&self, space: &str, owner: &str) -> Result<bool> {
        Ok(joined(&self.state(space).await?, owner))
    }
    pub async fn puppet_invited(&self, room: &str, puppet: &str) -> Result<bool> {
        Ok(content(&self.state(room).await?, "m.room.member", puppet)
            .is_some_and(|c| c["membership"] == "invite"))
    }
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
fn room_navigation_candidate(room_state: &[Value], space_state: &[Value], room: &str) -> bool {
    content(room_state, "m.room.tombstone", "").is_none()
        && content(room_state, "m.room.create", "").is_none_or(|c| c["type"] != "m.space")
        && content(space_state, "m.space.child", room)
            .is_some_and(|c| c["via"].as_array().is_some_and(|v| !v.is_empty()))
}
fn room_navigation_metadata(
    room_state: &[Value],
    space_state: &[Value],
    room: &str,
    actor: &str,
) -> Option<Value> {
    if !joined(space_state, actor) || !room_navigation_candidate(room_state, space_state, room) {
        return None;
    }
    let membership = content(room_state, "m.room.member", actor)?["membership"].as_str()?;
    if !matches!(membership, "join" | "invite") {
        return None;
    }
    let name = content(room_state, "m.room.name", "")
        .and_then(|c| c.get("name"))
        .and_then(Value::as_str);
    let topic = content(room_state, "m.room.topic", "")
        .and_then(|c| c.get("topic"))
        .and_then(Value::as_str);
    Some(serde_json::json!({"name":name,"topic":topic,"membership":membership}))
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
// Matrix v12 creators have unbounded power outside power_levels.users. Read
// only the immutable create event from the trusted current state, never an
// arbitrary message, unsigned field or old-version additional_creators claim.
fn room_creator(state: &[Value], user: &str) -> Result<bool> {
    let Some(create) = state
        .iter()
        .find(|e| e["type"] == "m.room.create" && e["state_key"] == "")
    else {
        return Ok(false);
    };
    if create["content"]["room_version"] != "12" {
        return Ok(false);
    }
    let valid = |id: &str| {
        id.len() <= 255
            && id.starts_with('@')
            && !id.chars().any(|c| c.is_control() || c.is_whitespace())
            && id[1..]
                .split_once(':')
                .is_some_and(|(local, server)| !local.is_empty() && !server.is_empty())
    };
    let sender = create["sender"]
        .as_str()
        .filter(|id| valid(id))
        .ok_or(Error::Unavailable("invalid_matrix_create"))?;
    let additional = create["content"].get("additional_creators");
    let mut is_creator = sender == user;
    if let Some(additional) = additional {
        for creator in additional
            .as_array()
            .ok_or(Error::Unavailable("invalid_matrix_create"))?
        {
            let id = creator
                .as_str()
                .filter(|id| valid(id))
                .ok_or(Error::Unavailable("invalid_matrix_create"))?;
            is_creator |= id == user;
        }
    }
    Ok(is_creator)
}
fn at_least(state: &[Value], user: &str, required: i64) -> Result<bool> {
    let explicit = level(content(state, "m.room.power_levels", ""), user)?;
    Ok(room_creator(state, user)? || explicit >= required)
}
fn can_send(state: &[Value], user: &str) -> Result<bool> {
    let ordinary = message_permission(content(state, "m.room.power_levels", ""), user)?;
    Ok(room_creator(state, user)? || ordinary)
}
/// Linking Rooms and managing Agent policy may have different Matrix thresholds.
pub(crate) fn can_create_room(state: &[Value], actor: &str) -> Result<bool> {
    Ok(joined(state, actor)
        && content(state, "m.room.create", "").is_some_and(|c| c["type"] == "m.space")
        && at_least(
            state,
            actor,
            state_threshold(content(state, "m.room.power_levels", ""), "m.space.child")?,
        )?)
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
    fn room_creation_does_not_inherit_the_agent_policy_threshold() {
        let state = vec![
            serde_json::json!({"type":"m.room.create","state_key":"","content":{"type":"m.space"}}),
            serde_json::json!({"type":"m.room.member","state_key":"@alice:test","content":{"membership":"join"}}),
            serde_json::json!({"type":"m.room.power_levels","state_key":"","content":{"users":{"@alice:test":50},"events":{"m.space.child":50,"im.hagency.agent.policy":100}}}),
        ];
        assert!(can_create_room(&state, "@alice:test").unwrap());
        assert!(!at_least(&state, "@alice:test", 100).unwrap());
        assert!(!can_create_room(&state, "@outside:test").unwrap());
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

#[cfg(test)]
mod version12_tests {
    use super::*;
    #[tokio::test]
    async fn version12_creators_have_current_matrix_authority_without_power_user_entries() {
        let read: ReadState = Arc::new(|_| {
            Box::pin(async {
                Ok(vec![
                    serde_json::json!({"type":"m.room.create","state_key":"","sender":"@creator:test","content":{"type":"m.space","room_version":"12","additional_creators":["@co:test"]}}),
                    serde_json::json!({"type":"m.room.power_levels","state_key":"","content":{"users":{},"state_default":1000}}),
                    serde_json::json!({"type":"m.room.member","state_key":"@creator:test","content":{"membership":"join"}}),
                    serde_json::json!({"type":"m.room.member","state_key":"@co:test","content":{"membership":"join"}}),
                    serde_json::json!({"type":"m.room.member","state_key":"@ordinary:test","content":{"membership":"join"}}),
                ])
            })
        });
        let gateway = Gateway::new(read, "@service:test".into());
        for actor in ["@creator:test", "@co:test"] {
            let facts = gateway.admin("!opaque", actor, None).await.unwrap();
            assert!(facts.joined && facts.can_manage_policy);
        }
        assert!(
            !gateway
                .admin("!opaque", "@ordinary:test", None)
                .await
                .unwrap()
                .can_manage_policy
        );
    }
}

#[cfg(test)]
mod creator_boundary_tests {
    use super::*;
    #[test]
    fn creator_power_requires_v12_valid_create_and_still_parses_actual_power_state() {
        let mut state = vec![
            serde_json::json!({"type":"m.room.create","state_key":"","sender":"@alice:test","content":{"room_version":"12","additional_creators":["@co:test"]}}),
            serde_json::json!({"type":"m.room.power_levels","state_key":"","content":{"users":{},"events_default":900,"invite":900}}),
        ];
        for user in ["@alice:test", "@co:test"] {
            assert!(at_least(&state, user, 900).unwrap());
            assert!(can_send(&state, user).unwrap());
        }
        assert!(!can_send(&state, "@other:test").unwrap());
        state[0]["content"]["room_version"] = serde_json::json!("11");
        assert!(!at_least(&state, "@co:test", 900).unwrap());
        assert!(!at_least(&state, "@alice:test", 900).unwrap());
        state[0]["content"]["room_version"] = serde_json::json!("12");
        state[0]["content"]["additional_creators"] = serde_json::json!([123]);
        assert!(at_least(&state, "@alice:test", 900).is_err());
        state[0]["content"]["additional_creators"] = serde_json::json!([]);
        state[1]["content"]["users"] = serde_json::json!({"@alice:test":"invalid"});
        assert!(at_least(&state, "@alice:test", 900).is_err());
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn project_room_invitation_is_discoverable_without_granting_joined_authority() {
        let space = vec![
            json!({"type":"m.room.member","state_key":"@bob:test","content":{"membership":"join"}}),
            json!({"type":"m.space.child","state_key":"!room:test","content":{"via":["test"]}}),
        ];
        let mut room = vec![
            json!({"type":"m.room.member","state_key":"@bob:test","content":{"membership":"invite"}}),
            json!({"type":"m.room.name","state_key":"","content":{"name":"Planning"}}),
        ];
        let metadata = room_navigation_metadata(&room, &space, "!room:test", "@bob:test").unwrap();
        assert_eq!(metadata["membership"], "invite");
        assert_eq!(metadata["name"], "Planning");
        assert!(!joined(&room, "@bob:test"));
        for state in ["leave", "ban", "knock"] {
            room[0]["content"]["membership"] = json!(state);
            assert!(room_navigation_metadata(&room, &space, "!room:test", "@bob:test").is_none());
        }
        room[0]["content"]["membership"] = json!("join");
        assert!(room_navigation_metadata(&room, &space, "!room:test", "@bob:test").is_some());
        assert!(room_navigation_metadata(&room, &[], "!room:test", "@bob:test").is_none());
        assert!(room_navigation_metadata(&room, &space, "!unlinked:test", "@bob:test").is_none());
        assert!(room_navigation_metadata(&room, &space, "!room:test", "@outsider:test").is_none());
        assert!(room_navigation_candidate(&room, &space, "!room:test"));
        room.push(json!({"type":"m.room.tombstone","state_key":"","content":{"replacement_room":"!replacement:test"}}));
        assert!(!room_navigation_candidate(&room, &space, "!room:test"));
        assert!(room_navigation_metadata(&room, &space, "!room:test", "@bob:test").is_none());
    }
}
