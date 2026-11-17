//! Puppet invites for hosted Agents. The homeserver delivers an invite of an
//! Agent's puppet only to this appservice, so the Agent's owner (a concierge
//! acting for a browser user) never sees it. An invite is kept only when the
//! inviter is the Agent's recorded Room human (its requester); the owner lists
//! it and binds through the normal bind path, which still enforces visibility.
use super::*;
use serde_json::Value;

/// Invites older than this are not offered: the requester can invite again.
pub const PUPPET_INVITE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// At most this many open invites are kept per Agent.
const MAX_PUPPET_INVITES: i64 = 64;

/// An `m.room.member` invite as delivered in an appservice transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PuppetInviteEvent {
    pub room_id: String,
    pub inviter_mxid: String,
    pub puppet_mxid: String,
}

pub(crate) fn parse_puppet_invite(raw: &Value) -> Option<PuppetInviteEvent> {
    if raw["type"] != "m.room.member" || raw["content"]["membership"] != "invite" {
        return None;
    }
    let room = raw["room_id"].as_str()?;
    let sender = raw["sender"].as_str()?;
    let target = raw["state_key"].as_str()?;
    let valid = |id: &str, sigil: char| id.starts_with(sigil) && id.len() <= 255;
    if !valid(room, '!') || !valid(sender, '@') || !valid(target, '@') || sender == target {
        return None;
    }
    Some(PuppetInviteEvent { room_id: room.into(), inviter_mxid: sender.into(), puppet_mxid: target.into() })
}

#[derive(Debug, Serialize, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct PuppetInvite {
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub inviter_mxid: String,
    #[diesel(sql_type=BigInt)]
    pub received_at_ms: i64,
}

impl TransportStore {
    /// Trusted routing worker only. Returns whether the invite was kept.
    pub async fn record_puppet_invite(&self, invite: &PuppetInviteEvent, received_at_ms: i64) -> Result<bool> {
        let mut guard = self.db.lock().await?;
        let db = &mut *guard;
        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            // The inviter must be a Room human the owner recorded for this
            // Agent on a live binding; anyone else's invite is ignored.
            let agent = sql_query("SELECT a.id FROM hagency_agent_v1.agents a JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id WHERE a.puppet_mxid=$1 AND a.state IN ('active','suspended') AND u.active AND EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.agent_id=a.id AND b.state<>'left' AND b.room_human_mxid=$2)")
                .bind::<Text, _>(&invite.puppet_mxid).bind::<Text, _>(&invite.inviter_mxid)
                .get_result::<AgentId>(db).await.optional()?;
            let Some(agent) = agent else { return Ok(false) };
            sql_query("DELETE FROM hagency_agent_v1.puppet_invites i WHERE agent_id=$1 AND (received_at_ms<$2 OR EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.agent_id=i.agent_id AND b.room_id=i.room_id AND b.state<>'left'))")
                .bind::<Text, _>(&agent.id).bind::<BigInt, _>(received_at_ms - PUPPET_INVITE_TTL_MS).execute(db).await?;
            let open = sql_query("SELECT count(*) AS total FROM hagency_agent_v1.puppet_invites WHERE agent_id=$1 AND room_id<>$2")
                .bind::<Text, _>(&agent.id).bind::<Text, _>(&invite.room_id).get_result::<Count>(db).await?;
            if open.total >= MAX_PUPPET_INVITES {
                return Ok(false);
            }
            sql_query("INSERT INTO hagency_agent_v1.puppet_invites(agent_id,room_id,inviter_mxid,received_at_ms) VALUES($1,$2,$3,$4) ON CONFLICT(agent_id,room_id) DO UPDATE SET inviter_mxid=EXCLUDED.inviter_mxid,received_at_ms=EXCLUDED.received_at_ms")
                .bind::<Text, _>(&agent.id).bind::<Text, _>(&invite.room_id).bind::<Text, _>(&invite.inviter_mxid).bind::<BigInt, _>(received_at_ms)
                .execute(db).await?;
            Ok(true)
        }).await
    }
    /// Owner only: open invites of the owner's Agent into Rooms it is not bound to.
    pub async fn puppet_invites(&self, p: &Principal, agent_id: &str, now: i64) -> Result<Vec<PuppetInvite>> {
        key(agent_id)?;
        let mut guard = self.db.lock().await?;
        let db = &mut *guard;
        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authenticate(db, p, now).await?;
            sql_query("SELECT true AS matched FROM hagency_agent_v1.agents WHERE id=$1 AND owner_user_id=$2")
                .bind::<Text, _>(agent_id).bind::<Text, _>(&p.user_id).get_result::<Flag>(db).await.map_err(unauthorized)?;
            Ok(sql_query("SELECT i.room_id,i.inviter_mxid,i.received_at_ms FROM hagency_agent_v1.puppet_invites i WHERE i.agent_id=$1 AND i.received_at_ms>=$2 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.agent_id=i.agent_id AND b.room_id=i.room_id AND b.state<>'left') AND EXISTS(SELECT 1 FROM hagency_agent_v1.bindings h WHERE h.agent_id=i.agent_id AND h.state<>'left' AND h.room_human_mxid=i.inviter_mxid) ORDER BY i.received_at_ms,i.room_id")
                .bind::<Text, _>(agent_id).bind::<BigInt, _>(now - PUPPET_INVITE_TTL_MS).load(db).await?)
        }).await
    }
}

#[derive(diesel::QueryableByName)]
struct AgentId {
    #[diesel(sql_type=Text)]
    id: String,
}
