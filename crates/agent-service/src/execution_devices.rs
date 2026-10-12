//! Agent-owned stable execution device; never a transferable execution identity.
use super::*;
use diesel::QueryableByName;
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetExecutionDevice {
    pub expected_generation: i64,
}
#[derive(Serialize, QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct OwnerDevice {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub name: String,
    #[diesel(sql_type=BigInt)]
    pub generation: i64,
    #[diesel(sql_type=Bool)]
    pub revoked: bool,
}
pub(crate) async fn require_assigned(
    db: &mut AsyncPgConnection,
    p: &Principal,
    agent: &str,
) -> Result<()> {
    let device = p
        .device_id
        .as_deref()
        .ok_or(Error::Unauthorized("device_authorization_required"))?;
    sql_query("SELECT true AS matched FROM hagency_agent_v1.agents WHERE id=$1 AND owner_user_id=$2 AND execution_device_id=$3 FOR SHARE")
 .bind::<Text,_>(agent).bind::<Text,_>(&p.user_id).bind::<Text,_>(device).get_result::<Flag>(db).await.map_err(|_|Error::Unauthorized("execution_device_required"))?;
    Ok(())
}
impl DomainStore {
    pub async fn owner_devices(&self, p: &Principal, now: i64) -> Result<Vec<OwnerDevice>> {
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {Self::authorize(db,p,now).await?;
 Ok(sql_query("SELECT id,name,generation,revoked FROM hagency_agent_v1.devices WHERE user_id=$1 ORDER BY id").bind::<Text,_>(&p.user_id).load(db).await?) }).await
    }
    pub async fn set_execution_device(
        &self,
        p: &Principal,
        agent: &str,
        input: SetExecutionDevice,
        now: i64,
    ) -> Result<Agent> {
        if input.expected_generation < 1 {
            return Err(Error::Invalid("invalid_execution_device"));
        }
        let device = p
            .device_id
            .as_deref()
            .ok_or(Error::Unauthorized("device_authorization_required"))?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
 Self::authorize(db,p,now).await?;let current=Self::agent_db(db,p,agent).await?;
 if !matches!(current.state.as_str(),"creating"|"active"|"suspended") {return Err(Error::Conflict("agent_retired"));}
 if current.generation!=input.expected_generation {return Err(Error::Conflict("execution_device_changed"));}
 if current.execution_device_id.as_deref()==Some(device) {return Ok(current);}
 sql_query("UPDATE hagency_agent_v1.agents SET execution_device_id=$1,generation=generation+1 WHERE id=$2").bind::<Text,_>(device).bind::<Text,_>(agent).execute(db).await?;
 // Already issued tools cannot be physically undone; retain uncertain execution.
 sql_query("UPDATE hagency_agent_v1.owner_events SET state=CASE WHEN state='running' THEN 'unknown' ELSE 'pending' END WHERE agent_id=$1 AND state IN ('offered','acknowledged','running')").bind::<Text,_>(agent).execute(db).await?;
 sql_query("UPDATE hagency_agent_v1.reply_outbox SET state='cancelled' WHERE agent_id=$1 AND state='pending'").bind::<Text,_>(agent).execute(db).await?;
 sql_query("UPDATE hagency_agent_v1.execution_leases SET expires_at_ms=0 WHERE agent_id=$1").bind::<Text,_>(agent).execute(db).await?;
 Self::audit(db,p,"agent.execution_device",agent,now).await?;
 Self::agent_db(db,p,agent).await
 }).await
    }
}
