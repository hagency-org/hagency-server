//! Agent-owned stable execution device; never a transferable execution identity.
use super::*;
use crate::management_types::ManagementPolicy;
use diesel::sql_types::Nullable;
use diesel::{BoolExpressionMethods, ExpressionMethods, IntoSql, QueryDsl};
use diesel::{OptionalExtension, QueryableByName};
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
    #[diesel(sql_type=Nullable<Text>)]
    pub isolation_kind: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    pub isolation_host: Option<String>,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IsolationKind {
    Kata,
    Firecracker,
}
impl IsolationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Kata => "kata",
            Self::Firecracker => "firecracker",
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttestExecutorIsolation {
    pub kind: IsolationKind,
    pub host_label: String,
    pub expected_generation: i64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeExecutorIsolation {}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorIsolationQuery {
    pub after: Option<String>,
}
#[derive(Serialize, QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorIsolationDevice {
    #[diesel(sql_type=Text)]
    device_id: String,
    #[diesel(sql_type=Text)]
    owner_mxid: String,
    #[diesel(sql_type=Text)]
    name: String,
    #[diesel(sql_type=BigInt)]
    generation: i64,
    #[diesel(sql_type=Bool)]
    revoked: bool,
    #[diesel(sql_type=Nullable<Text>)]
    isolation_kind: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    isolation_host: Option<String>,
    #[diesel(sql_type=Nullable<BigInt>)]
    attested_at_ms: Option<i64>,
    #[diesel(sql_type=Nullable<Text>)]
    attested_by_mxid: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorIsolationPage {
    devices: Vec<ExecutorIsolationDevice>,
    next_after: Option<String>,
}
// Attestations of an older device generation are invisible and never satisfy the gate.
const ISOLATION_VIEW: &str = "SELECT d.id AS device_id, u.mxid AS owner_mxid, d.name, d.generation, d.revoked,
 a.kind AS isolation_kind, a.host_label AS isolation_host, a.attested_at_ms, au.mxid AS attested_by_mxid
FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.users u ON u.id=d.user_id
LEFT JOIN hagency_agent_v1.executor_isolation_attestations a
 ON a.device_id=d.id AND a.owner_user_id=d.user_id AND a.generation=d.generation
LEFT JOIN hagency_agent_v1.users au ON au.id=a.attested_by_user_id";
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
async fn isolation_device(
    db: &mut AsyncPgConnection,
    device_id: &str,
) -> Result<ExecutorIsolationDevice> {
    sql_query(format!("{ISOLATION_VIEW} WHERE d.id=$1"))
        .bind::<Text, _>(device_id)
        .get_result::<ExecutorIsolationDevice>(db)
        .await
        .optional()?
        .ok_or(Error::NotFound("execution_device_not_found"))
}
impl DomainStore {
    pub async fn owner_devices(&self, p: &Principal, now: i64) -> Result<Vec<OwnerDevice>> {
        self.db
            .write_transaction(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, now).await?;
                Ok(sql_query("SELECT d.id,d.name,d.generation,d.revoked,a.kind AS isolation_kind,a.host_label AS isolation_host FROM hagency_agent_v1.devices d LEFT JOIN hagency_agent_v1.executor_isolation_attestations a ON a.device_id=d.id AND a.owner_user_id=d.user_id AND a.generation=d.generation WHERE d.user_id=$1 ORDER BY d.id")
                    .bind::<Text,_>(&p.user_id)
                    .load::<OwnerDevice>(db)
                    .await?)
            })
            .await
    }
    pub async fn executor_isolation_devices(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        q: ExecutorIsolationQuery,
    ) -> Result<ExecutorIsolationPage> {
        if !policy.can_administer(p) {
            return Err(Error::Unauthorized("management_administrator_required"));
        }
        if q.after.as_deref() == Some("") {
            return Err(Error::Invalid("invalid_management_payload"));
        }
        self.db
            .write_transaction(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let mut devices = sql_query(format!(
                    "{ISOLATION_VIEW} WHERE ($1::text IS NULL OR d.id>$1) ORDER BY d.id LIMIT {}",
                    crate::management_types::MAX_PAGE + 1
                ))
                .bind::<Nullable<Text>, _>(&q.after)
                .load::<ExecutorIsolationDevice>(db)
                .await?;
                let more = devices.len() as i64 > crate::management_types::MAX_PAGE;
                devices.truncate(crate::management_types::MAX_PAGE as usize);
                let next_after = more
                    .then(|| devices.last().map(|d| d.device_id.clone()))
                    .flatten();
                Ok(ExecutorIsolationPage {
                    devices,
                    next_after,
                })
            })
            .await
    }
    pub async fn attest_executor_isolation(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        device_id: &str,
        input: AttestExecutorIsolation,
    ) -> Result<ExecutorIsolationDevice> {
        if !policy.can_administer(p) {
            return Err(Error::Unauthorized("management_administrator_required"));
        }
        let label = &input.host_label;
        if input.expected_generation < 1
            || label.is_empty()
            || label.len() > 64
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(Error::Invalid("invalid_executor_isolation"));
        }
        self.db
            .write_transaction(async |db: &mut AsyncPgConnection| {
                let now = crate::api::now_ms();
                Self::authorize(db, p, now).await?;
                #[derive(QueryableByName)]
                struct Row {
                    #[diesel(sql_type=Text)]
                    user_id: String,
                    #[diesel(sql_type=BigInt)]
                    generation: i64,
                    #[diesel(sql_type=Bool)]
                    revoked: bool,
                }
                let row = sql_query("SELECT user_id, generation, revoked FROM hagency_agent_v1.devices WHERE id=$1 FOR UPDATE")
                    .bind::<Text,_>(device_id)
                    .get_result::<Row>(db)
                    .await
                    .optional()?
                    .filter(|r| !r.revoked)
                    .ok_or(Error::NotFound("execution_device_not_found"))?;
                if row.generation != input.expected_generation {
                    return Err(Error::Conflict("execution_device_generation_changed"));
                }
                sql_query("INSERT INTO hagency_agent_v1.executor_isolation_attestations(device_id,owner_user_id,generation,kind,host_label,attested_by_user_id,attested_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(device_id) DO UPDATE SET owner_user_id=excluded.owner_user_id, generation=excluded.generation, kind=excluded.kind, host_label=excluded.host_label, attested_by_user_id=excluded.attested_by_user_id, attested_at_ms=excluded.attested_at_ms")
                    .bind::<Text,_>(device_id)
                    .bind::<Text,_>(&row.user_id)
                    .bind::<BigInt,_>(row.generation)
                    .bind::<Text,_>(input.kind.as_str())
                    .bind::<Text,_>(label)
                    .bind::<Text,_>(&p.user_id)
                    .bind::<BigInt,_>(now)
                    .execute(db)
                    .await?;
                Self::audit(db, p, "executor_isolation.attest", device_id, now).await?;
                isolation_device(db, device_id).await
            })
            .await
    }
    pub async fn revoke_executor_isolation(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        device_id: &str,
        _input: RevokeExecutorIsolation,
    ) -> Result<ExecutorIsolationDevice> {
        if !policy.can_administer(p) {
            return Err(Error::Unauthorized("management_administrator_required"));
        }
        self.db
            .write_transaction(async |db: &mut AsyncPgConnection| {
                let now = crate::api::now_ms();
                Self::authorize(db, p, now).await?;
                let device = isolation_device(db, device_id).await?;
                sql_query("DELETE FROM hagency_agent_v1.executor_isolation_attestations WHERE device_id=$1")
                    .bind::<Text,_>(device_id)
                    .execute(db)
                    .await?;
                Self::audit(db, p, "executor_isolation.revoke", device_id, now).await?;
                Ok(ExecutorIsolationDevice { isolation_kind: None, isolation_host: None, attested_at_ms: None, attested_by_mxid: None, ..device })
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
        crate::database::retry_crypto_write(|| self.db.write_transaction(async |db:&mut AsyncPgConnection| {
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
 })).await
    }
}
