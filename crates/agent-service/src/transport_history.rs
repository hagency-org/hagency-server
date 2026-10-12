//! Permanent execution metadata witnesses. No message bodies, model budgets or
//! charges leave the server; the authenticated owner client checks its ledger.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionHistorySnapshot {
    pub count: u64,
    pub digest: String,
}
impl ExecutionHistorySnapshot {
    pub(super) fn validate(&self) -> Result<()> {
        if self.count > 9_007_199_254_740_991
            || self.digest.len() != 64
            || !self
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid("invalid_execution_history_snapshot"));
        }
        Ok(())
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryExecution {
    pub dispatch_id: String,
    pub execution_id: String,
    pub binding_id: String,
    pub agent_id: String,
    pub event_id: String,
    pub room_id: String,
    pub requester_mxid: String,
    pub thread_root: String,
    pub binding_generation: i64,
    pub dispatch_epoch: i64,
    pub dispatch_device_id: String,
    pub immutable_digest: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionHistoryPage {
    pub agent_id: String,
    pub snapshot: ExecutionHistorySnapshot,
    pub executions: Vec<HistoryExecution>,
    pub next_cursor: Option<String>,
}
#[derive(diesel::QueryableByName)]
struct IdentityRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    execution_id: String,
}
#[derive(diesel::QueryableByName)]
struct HistoryRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    execution_id: String,
    #[diesel(sql_type=Text)]
    binding_id: String,
    #[diesel(sql_type=Text)]
    agent_id: String,
    #[diesel(sql_type=Text)]
    event_id: String,
    #[diesel(sql_type=Text)]
    room_id: String,
    #[diesel(sql_type=Text)]
    requester_mxid: String,
    #[diesel(sql_type=Text)]
    thread_root: String,
    #[diesel(sql_type=BigInt)]
    binding_generation: i64,
    #[diesel(sql_type=BigInt)]
    dispatch_epoch: i64,
    #[diesel(sql_type=Text)]
    dispatch_device_id: String,
    #[diesel(sql_type=Text)]
    body: String,
}
impl HistoryRow {
    fn metadata(self) -> Result<HistoryExecution> {
        // Exact serde tuple order/encoding used by local inbox::Dispatch::digest.
        let immutable_digest = hash(
            &serde_json::to_string(&(
                &self.id,
                &self.binding_id,
                &self.agent_id,
                &self.event_id,
                &self.room_id,
                &self.requester_mxid,
                &self.thread_root,
                &self.body,
                self.binding_generation,
            ))
            .map_err(|_| Error::Invalid("invalid_execution_history"))?,
        );
        Ok(HistoryExecution {
            dispatch_id: self.id,
            execution_id: self.execution_id,
            binding_id: self.binding_id,
            agent_id: self.agent_id,
            event_id: self.event_id,
            room_id: self.room_id,
            requester_mxid: self.requester_mxid,
            thread_root: self.thread_root,
            binding_generation: self.binding_generation,
            dispatch_epoch: self.dispatch_epoch,
            dispatch_device_id: self.dispatch_device_id,
            immutable_digest,
        })
    }
}
pub(super) async fn snapshot(
    db: &mut AsyncPgConnection,
    p: &Principal,
    agent: &str,
) -> Result<ExecutionHistorySnapshot> {
    // Callers hold the same authentication advisory lock as execution start.
    // Retired Agents/bindings and old lease epochs remain permanent witnesses.
    sql_query(
        "SELECT true AS matched FROM hagency_agent_v1.agents WHERE id=$1 AND owner_user_id=$2",
    )
    .bind::<Text, _>(agent)
    .bind::<Text, _>(&p.user_id)
    .get_result::<Flag>(db)
    .await
    .map_err(unauthorized)?;
    let mut digest = Sha256::new();
    digest.update(b"hagency-started-executions-v1\n");
    let mut cursor = String::new();
    let mut count = 0u64;
    loop {
        let rows=sql_query("SELECT id,execution_id FROM hagency_agent_v1.owner_events WHERE agent_id=$1 AND owner_user_id=$2 AND execution_id IS NOT NULL AND id COLLATE \"C\">$3 COLLATE \"C\" ORDER BY id COLLATE \"C\" LIMIT 512")
            .bind::<Text,_>(agent).bind::<Text,_>(&p.user_id).bind::<Text,_>(&cursor).load::<IdentityRow>(db).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            digest.update(
                serde_json::to_vec(&(&row.id, &row.execution_id))
                    .map_err(|_| Error::Invalid("invalid_execution_history"))?,
            );
            digest.update(b"\n");
            cursor = row.id;
            count = count
                .checked_add(1)
                .filter(|v| *v <= 9_007_199_254_740_991)
                .ok_or(Error::Conflict("execution_history_exhausted"))?;
        }
    }
    Ok(ExecutionHistorySnapshot {
        count,
        digest: hex::encode(digest.finalize()),
    })
}
impl TransportStore {
    /// Owner/device-authenticated history across ALL bindings, including retired
    /// Rooms. Cursor pages require the original immutable snapshot witness.
    pub async fn execution_history(
        &self,
        p: &Principal,
        agent: &str,
        cursor: Option<&str>,
        expected: Option<&ExecutionHistorySnapshot>,
        now: i64,
    ) -> Result<ExecutionHistoryPage> {
        key(agent)?;
        if let Some(cursor) = cursor {
            key(cursor)?;
            if expected.is_none() {
                return Err(Error::Invalid("execution_history_snapshot_required"));
            }
        }
        if let Some(expected) = expected {
            expected.validate()?;
        }
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authenticate(db,p,now).await?;
            let current=snapshot(db,p,agent).await?;
            if expected.is_some_and(|value|value!=&current){return Err(Error::Conflict("execution_history_changed"));}
            let mut rows=sql_query("SELECT id,execution_id,binding_id,agent_id,event_id,room_id,requester_mxid,thread_root,binding_generation,dispatch_epoch,dispatch_device_id,body FROM hagency_agent_v1.owner_events WHERE agent_id=$1 AND owner_user_id=$2 AND execution_id IS NOT NULL AND id COLLATE \"C\">$3 COLLATE \"C\" ORDER BY id COLLATE \"C\" LIMIT 129")
                .bind::<Text,_>(agent).bind::<Text,_>(&p.user_id).bind::<Text,_>(cursor.unwrap_or("")).load::<HistoryRow>(db).await?;
            let more=rows.len()>128;if more {rows.pop();}
            let next_cursor=if more {rows.last().map(|row|row.id.clone())}else{None};
            let executions=rows.into_iter().map(HistoryRow::metadata).collect::<Result<Vec<_>>>()?;
            Self::authenticate(db,p,now).await?;
            Ok(ExecutionHistoryPage{agent_id:agent.into(),snapshot:current,executions,next_cursor})
        }).await
    }
    #[cfg(test)]
    pub(crate) async fn acquire_for_test(
        &self,
        p: &Principal,
        agent: &str,
        ttl: i64,
        takeover: bool,
        now: i64,
    ) -> Result<Lease> {
        let current = self
            .execution_history(p, agent, None, None, now)
            .await?
            .snapshot;
        self.acquire_lease(p, agent, ttl, takeover, &current, now)
            .await
    }
}
