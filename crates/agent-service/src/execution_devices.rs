//! Agent-owned stable execution device; never a transferable execution identity.
use super::*;
use diesel::QueryableByName;
use diesel::{BoolExpressionMethods, ExpressionMethods, IntoSql, QueryDsl};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetExecutionDevice {
    pub expected_generation: i64,
}
#[derive(Serialize, diesel::Queryable, QueryableByName)]
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
    {
        use crate::schema::agent_tables::agents as q;
        let params = (agent, &p.user_id, device);
        q::table
            .select((true.into_sql::<diesel::sql_types::Bool>(),))
            .filter(
                (((q::id).eq(&params.0)).and((q::owner_user_id).eq(&params.1)))
                    .and((q::execution_device_id).eq(&params.2)),
            )
            .for_share()
            .get_result::<Flag>(db)
            .await
    }
    .map_err(|_| Error::Unauthorized("execution_device_required"))?;
    Ok(())
}
impl DomainStore {
    pub async fn owner_devices(&self, p: &Principal, now: i64) -> Result<Vec<OwnerDevice>> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    Ok({
                        use crate::schema::agent_tables::devices as q;
                        let params = (&p.user_id,);
                        q::table
                            .select((q::id, q::name, q::generation, q::revoked))
                            .filter((q::user_id).eq(&params.0))
                            .order((q::id.asc(),))
                            .load(db)
                            .await
                    }?)
                })
                .await
            })
            .await
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
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
 Self::authorize(db,p,now).await?;let current=Self::agent_db(db,p,agent).await?;
 if !matches!(current.state.as_str(),"creating"|"active"|"suspended") {return Err(Error::Conflict("agent_retired"));}
 if current.generation!=input.expected_generation {return Err(Error::Conflict("execution_device_changed"));}
 if current.execution_device_id.as_deref()==Some(device) {return Ok(current);}
 {
     use crate::schema::agent_tables::agents as q;
     let params = (device, agent);
     diesel::update(q::table)
         .filter((q::id).eq(&params.1))
         .set((
             q::execution_device_id.eq(&params.0),
             q::generation.eq(q::generation + 1),
         ))
         .execute(db)
         .await
 }?;
 // Already issued tools cannot be physically undone; retain uncertain execution.
 sql_query("UPDATE hagency_agent_v1.owner_events SET state=CASE WHEN state='running' THEN 'unknown' ELSE 'pending' END WHERE agent_id=$1 AND state IN ('offered','acknowledged','running')").bind::<Text,_>(agent).execute(db).await?;
 {
     use crate::schema::agent_tables::reply_outbox as q;
     let params = (agent,);
     diesel::update(q::table)
         .filter(((q::agent_id).eq(&params.0)).and((q::state).eq("pending")))
         .set((q::state.eq("cancelled"),))
         .execute(db)
         .await
 }?;
 {
     use crate::schema::agent_tables::execution_leases as q;
     let params = (agent,);
     diesel::update(q::table)
         .filter((q::agent_id).eq(&params.0))
         .set((q::expires_at_ms.eq(0),))
         .execute(db)
         .await
 }?;
 Self::audit(db,p,"agent.execution_device",agent,now).await?;
 Self::agent_db(db,p,agent).await
 }).await
    }).await
    }
}
