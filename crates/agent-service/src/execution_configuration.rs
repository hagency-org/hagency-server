//! Device-scoped configuration delivery. Claims are never redelivered or used as model leases.
use super::*;
use crate::configuration_types::*;
use diesel::QueryableByName;
use serde_json::Value;

const TTL: i64 = 60_000;
const ONLINE: i64 = 15_000;
const RETENTION: i64 = 7 * 24 * 60 * 60 * 1000;
const COLUMNS: &str = "id,owner_user_id,agent_id,agent_generation,execution_device_id,device_generation,requester_device_id,requester_generation,idempotency_key,digest,operation,input,state,created_at_ms,expires_at_ms,claimed_at_ms,completed_at_ms,result,error_code";
// Current ownership, assignment, lifecycle and both authorization epochs are required.
const VALID_PINS: &str = "EXISTS(SELECT 1 FROM hagency_agent_v1.agents a JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id JOIN hagency_agent_v1.devices t ON t.id=q.execution_device_id AND t.user_id=u.id JOIN hagency_agent_v1.devices r ON r.id=q.requester_device_id AND r.user_id=u.id JOIN hagency_agent_v1.sessions ts ON ts.id=t.session_id AND ts.user_id=u.id JOIN hagency_agent_v1.sessions rs ON rs.id=r.session_id AND rs.user_id=u.id WHERE a.id=q.agent_id AND u.id=q.owner_user_id AND u.active AND a.state IN ('creating','active','suspended') AND a.generation=q.agent_generation AND a.execution_device_id=q.execution_device_id AND t.generation=q.device_generation AND r.generation=q.requester_generation AND NOT t.revoked AND NOT r.revoked AND NOT ts.revoked AND NOT rs.revoked AND ts.valid_until_ms>greatest($1,(extract(epoch from clock_timestamp())*1000)::bigint) AND rs.valid_until_ms>greatest($1,(extract(epoch from clock_timestamp())*1000)::bigint))";
#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    agent_id: String,
    #[diesel(sql_type=BigInt)]
    agent_generation: i64,
    #[diesel(sql_type=Text)]
    execution_device_id: String,
    #[diesel(sql_type=BigInt)]
    device_generation: i64,
    #[diesel(sql_type=Text)]
    requester_device_id: String,
    #[diesel(sql_type=BigInt)]
    requester_generation: i64,
    #[diesel(sql_type=Text)]
    idempotency_key: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    operation: String,
    #[diesel(sql_type=Text)]
    input: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type=BigInt)]
    expires_at_ms: i64,
    #[diesel(sql_type=Nullable<BigInt>)]
    claimed_at_ms: Option<i64>,
    #[diesel(sql_type=Nullable<BigInt>)]
    completed_at_ms: Option<i64>,
    #[diesel(sql_type=Nullable<Text>)]
    result: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    error_code: Option<String>,
}
impl Row {
    fn public(self) -> Result<ConfigurationRequest> {
        // Internal authorization pins/digests never enter the public result.
        let _ = (
            self.owner_user_id,
            self.requester_device_id,
            self.requester_generation,
            self.idempotency_key,
        );
        Ok(ConfigurationRequest {
            id: self.id,
            agent_id: self.agent_id,
            agent_generation: self.agent_generation,
            execution_device_id: self.execution_device_id,
            device_generation: self.device_generation,
            operation: serde_json::from_str(&self.operation)
                .map_err(|_| Error::Unavailable("configuration_storage_invalid"))?,
            input: serde_json::from_str(&self.input)
                .map_err(|_| Error::Unavailable("configuration_storage_invalid"))?,
            state: self.state,
            created_at_ms: self.created_at_ms,
            expires_at_ms: self.expires_at_ms,
            claimed_at_ms: self.claimed_at_ms,
            completed_at_ms: self.completed_at_ms,
            result: self
                .result
                .map(|v| serde_json::from_str(&v))
                .transpose()
                .map_err(|_| Error::Unavailable("configuration_storage_invalid"))?,
            error_code: self.error_code,
        })
    }
}
#[derive(QueryableByName)]
struct Presence {
    #[diesel(sql_type=BigInt)]
    generation: i64,
    #[diesel(sql_type=Nullable<BigInt>)]
    last_seen: Option<i64>,
}
#[derive(QueryableByName)]
struct Observation {
    #[diesel(sql_type=BigInt)]
    observed_at_ms: i64,
    #[diesel(sql_type=Text)]
    environment: String,
}
fn device(p: &Principal) -> Result<(&str, i64)> {
    match (p.device_id.as_deref(), p.device_generation) {
        (Some(id), Some(generation)) => Ok((id, generation)),
        _ => Err(Error::Unauthorized("device_authorization_required")),
    }
}
async fn maintain(db: &mut AsyncPgConnection, owner: &str, now: i64) -> Result<()> {
    // Sign-out updates a session directly. Keep both remote authorization epochs
    // locked through publication, including when the two devices have different sessions.
    sql_query("SELECT true AS matched FROM hagency_agent_v1.sessions WHERE user_id=$1 ORDER BY id FOR SHARE")
        .bind::<Text,_>(owner).load::<Flag>(db).await?;
    // Evaluated under the same domain advisory lock as assignment/lifecycle changes.
    sql_query(format!("UPDATE hagency_agent_v1.configuration_requests q SET state=CASE WHEN state='pending' THEN 'expired' ELSE 'unknown' END,result=NULL,error_code='configuration_target_changed' WHERE owner_user_id=$2 AND state NOT IN ('unknown','expired') AND NOT {VALID_PINS}"))
        .bind::<BigInt,_>(now).bind::<Text,_>(owner).execute(db).await?;
    sql_query("UPDATE hagency_agent_v1.configuration_requests SET state=CASE WHEN state='pending' THEN 'expired' ELSE 'unknown' END,result=NULL,error_code='configuration_request_expired' WHERE owner_user_id=$1 AND state IN ('pending','claimed') AND expires_at_ms<=greatest($2,(extract(epoch from clock_timestamp())*1000)::bigint)")
        .bind::<Text,_>(owner).bind::<BigInt,_>(now).execute(db).await?;
    sql_query("DELETE FROM hagency_agent_v1.configuration_requests WHERE owner_user_id=$1 AND state NOT IN ('pending','claimed') AND created_at_ms<$2")
        .bind::<Text,_>(owner).bind::<BigInt,_>(now.saturating_sub(RETENTION)).execute(db).await?;
    Ok(())
}
async fn row(db: &mut AsyncPgConnection, owner: &str, id: &str) -> Result<Row> {
    sql_query(format!("SELECT {COLUMNS} FROM hagency_agent_v1.configuration_requests WHERE id=$1 AND owner_user_id=$2"))
        .bind::<Text,_>(id).bind::<Text,_>(owner).get_result(db).await.map_err(hidden)
}
async fn presence(
    db: &mut AsyncPgConnection,
    owner: &str,
    id: &str,
    now: i64,
) -> Result<Option<Presence>> {
    Ok(sql_query("SELECT d.generation,CASE WHEN p.device_generation=d.generation THEN p.last_seen_at_ms ELSE NULL END AS last_seen FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.sessions s ON s.id=d.session_id LEFT JOIN hagency_agent_v1.configuration_presence p ON p.device_id=d.id WHERE d.id=$1 AND d.user_id=$2 AND NOT d.revoked AND NOT s.revoked AND s.user_id=$2 AND s.valid_until_ms>$3")
        .bind::<Text,_>(id).bind::<Text,_>(owner).bind::<BigInt,_>(now).get_result(db).await.optional()?)
}
fn online(p: &Presence, now: i64) -> bool {
    p.last_seen
        .is_some_and(|last| last <= now && last >= now - ONLINE)
}
fn stringify<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|_| Error::Invalid("invalid_configuration_payload"))
}
impl DomainStore {
    pub async fn configuration_status(
        &self,
        p: &Principal,
        id: &str,
        now: i64,
    ) -> Result<ConfigurationStatus> {
        key(id)?;
        device(p)?;
        let now = now.max(crate::api::now_ms());
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,now).await?;maintain(db,&p.user_id,now).await?;
            let a=Self::agent_db(db,p,id).await?;
            let target=match a.execution_device_id.as_deref() {Some(id)=>presence(db,&p.user_id,id,now).await?,None=>None};
            let target=if matches!(a.state.as_str(),"creating"|"active"|"suspended"){target}else{None};
            let mut result=ConfigurationStatus {agent_id:a.id.clone(),agent_generation:a.generation,execution_device_id:a.execution_device_id.clone(),device_generation:target.as_ref().map(|p|p.generation),online:target.as_ref().is_some_and(|p|online(p,now)),last_seen_at_ms:target.as_ref().and_then(|p|p.last_seen),observed_at_ms:None,cached:false,environment:None};
            if let (Some(target_id),Some(t))=(a.execution_device_id.as_deref(),target) {
                let observation=sql_query("SELECT observed_at_ms,environment FROM hagency_agent_v1.configuration_observations WHERE agent_id=$1 AND agent_generation=$2 AND execution_device_id=$3 AND device_generation=$4")
                    .bind::<Text,_>(&a.id).bind::<BigInt,_>(a.generation).bind::<Text,_>(target_id).bind::<BigInt,_>(t.generation).get_result::<Observation>(db).await.optional()?;
                if let Some(o)=observation {
                    result.observed_at_ms=Some(o.observed_at_ms);
                    result.cached=!result.online || o.observed_at_ms<now-ONLINE;
                    result.environment=Some(serde_json::from_str(&o.environment).map_err(|_|Error::Unavailable("configuration_storage_invalid"))?);
                }
            }
            Ok(result)
        }).await
    }
    pub async fn create_configuration_request(
        &self,
        p: &Principal,
        id: &str,
        input: ConfigurationInput,
        now: i64,
    ) -> Result<ConfigurationRequest> {
        key(id)?;
        key(&input.idempotency_key)?;
        let (requester, requester_generation) = device(p)?;
        if input.expected_generation < 1 {
            return Err(Error::Invalid("invalid_configuration_payload"));
        }
        let value = validate_input(&input.operation, input.input)?;
        let input_json = stringify(&value)?;
        let operation = stringify(&input.operation)?;
        let digest = hash(&stringify(&(
            input.expected_generation,
            &input.operation,
            &value,
        ))?);
        let now = now.max(crate::api::now_ms());
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,now).await?;maintain(db,&p.user_id,now).await?;
            let a=Self::agent_db(db,p,id).await?;
            let existing=sql_query(format!("SELECT {COLUMNS} FROM hagency_agent_v1.configuration_requests WHERE owner_user_id=$1 AND requester_device_id=$2 AND agent_id=$3 AND idempotency_key=$4"))
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(requester).bind::<Text,_>(id).bind::<Text,_>(&input.idempotency_key).get_result::<Row>(db).await.optional()?;
            if let Some(existing)=existing {if existing.digest!=digest{return Err(Error::Conflict("configuration_idempotency_conflict"));}return existing.public();}
            if a.generation!=input.expected_generation{return Err(Error::Conflict("configuration_target_changed"));}
            if !matches!(a.state.as_str(),"creating"|"active"|"suspended"){return Err(Error::Conflict("agent_retired"));}
            let target=a.execution_device_id.as_deref().ok_or(Error::Conflict("execution_device_required"))?;
            let t=presence(db,&p.user_id,target,now).await?.ok_or(Error::Conflict("configuration_device_unavailable"))?;
            if input.operation.mutation() && !online(&t,now){return Err(Error::Conflict("configuration_device_offline"));}
            let full=sql_query("SELECT (SELECT count(*) FROM hagency_agent_v1.configuration_requests WHERE owner_user_id=$1 AND state IN ('pending','claimed'))>=16 OR (SELECT count(*) FROM hagency_agent_v1.configuration_requests WHERE execution_device_id=$2 AND state IN ('pending','claimed'))>=16 AS matched")
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(target).get_result::<Flag>(db).await?.matched;
            if full{return Err(Error::Conflict("configuration_queue_full"));}
            if input.operation.mutation() {
                let busy=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.configuration_requests WHERE agent_id=$1 AND mutation AND state IN ('pending','claimed')) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;
                if busy{return Err(Error::Conflict("configuration_request_busy"));}
            }
            let request_id=format!("cfg_{}",entity_id()?);
            sql_query("INSERT INTO hagency_agent_v1.configuration_requests(id,owner_user_id,agent_id,agent_generation,execution_device_id,device_generation,requester_device_id,requester_generation,idempotency_key,digest,operation,input,mutation,state,created_at_ms,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,'pending',$14,$15)")
                .bind::<Text,_>(&request_id).bind::<Text,_>(&p.user_id).bind::<Text,_>(id).bind::<BigInt,_>(a.generation).bind::<Text,_>(target).bind::<BigInt,_>(t.generation).bind::<Text,_>(requester).bind::<BigInt,_>(requester_generation).bind::<Text,_>(&input.idempotency_key).bind::<Text,_>(&digest).bind::<Text,_>(&operation).bind::<Text,_>(&input_json).bind::<Bool,_>(input.operation.mutation()).bind::<BigInt,_>(now).bind::<BigInt,_>(now+TTL).execute(db).await?;
            row(db,&p.user_id,&request_id).await?.public()
        }).await
    }
    pub async fn configuration_request(
        &self,
        p: &Principal,
        agent: &str,
        id: &str,
        now: i64,
    ) -> Result<ConfigurationRequest> {
        key(agent)?;
        key(id)?;
        device(p)?;
        let now = now.max(crate::api::now_ms());
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, now).await?;
                maintain(db, &p.user_id, now).await?;
                Self::agent_db(db, p, agent).await?;
                let r = row(db, &p.user_id, id).await?;
                if r.agent_id != agent {
                    return Err(Error::NotFound("not_found"));
                }
                r.public()
            })
            .await
    }
    pub async fn poll_configuration(
        &self,
        p: &Principal,
        input: ConfigurationPoll,
        now: i64,
    ) -> Result<Option<ConfigurationRequest>> {
        let (id, generation) = device(p)?;
        let now = now.max(crate::api::now_ms());
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,now).await?;maintain(db,&p.user_id,now).await?;
            sql_query("INSERT INTO hagency_agent_v1.configuration_presence(device_id,device_generation,last_seen_at_ms) VALUES($1,$2,$3) ON CONFLICT(device_id) DO UPDATE SET device_generation=EXCLUDED.device_generation,last_seen_at_ms=EXCLUDED.last_seen_at_ms")
                .bind::<Text,_>(id).bind::<BigInt,_>(generation).bind::<BigInt,_>(now).execute(db).await?;
            if !input.accept_work{return Ok(None);}
            let busy=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.configuration_requests WHERE execution_device_id=$1 AND state='claimed') AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;
            if busy{return Ok(None);}
            let r=sql_query(format!("SELECT {COLUMNS} FROM hagency_agent_v1.configuration_requests WHERE owner_user_id=$1 AND execution_device_id=$2 AND device_generation=$3 AND state='pending' ORDER BY created_at_ms,id LIMIT 1"))
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(id).bind::<BigInt,_>(generation).get_result::<Row>(db).await.optional()?;
            let Some(mut r)=r else{return Ok(None);};
            sql_query("UPDATE hagency_agent_v1.configuration_requests SET state='claimed',claimed_at_ms=$2 WHERE id=$1").bind::<Text,_>(&r.id).bind::<BigInt,_>(now).execute(db).await?;
            r.state="claimed".into();r.claimed_at_ms=Some(now);Ok(Some(r.public()?))
        }).await
    }
    pub async fn complete_configuration(
        &self,
        p: &Principal,
        id: &str,
        input: ConfigurationComplete,
        now: i64,
    ) -> Result<ConfigurationRequest> {
        key(id)?;
        let (target, generation) = device(p)?;
        let now = now.max(crate::api::now_ms());
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,now).await?;maintain(db,&p.user_id,now).await?;
            let r=row(db,&p.user_id,id).await?;
            if r.execution_device_id!=target || r.device_generation!=generation {return Err(Error::Unauthorized("execution_device_required"));}
            if matches!(r.state.as_str(),"unknown"|"expired") {return Err(Error::Conflict(if r.error_code.as_deref()==Some("configuration_request_expired"){"configuration_request_expired"}else{"configuration_target_changed"}));}
            if r.claimed_at_ms.is_none() {return Err(Error::Conflict("configuration_not_claimed"));}
            let op:ConfigurationOperation=serde_json::from_str(&r.operation).map_err(|_|Error::Unavailable("configuration_storage_invalid"))?;
            let request_input:Value=serde_json::from_str(&r.input).map_err(|_|Error::Unavailable("configuration_storage_invalid"))?;
            let result=match (input.state.as_str(),input.result,input.error_code.as_deref()) {
                ("succeeded",Some(v),None)=>Some(stringify(&validate_result(&op,&request_input,&r.agent_id,v)?)?),
                ("failed",None,Some(code)) if failure_code(code)=>None,
                _=>return Err(Error::Invalid("invalid_configuration_payload")),
            };
            if matches!(r.state.as_str(),"succeeded"|"failed") {
                if r.state!=input.state || r.result!=result || r.error_code!=input.error_code {return Err(Error::Conflict("configuration_completion_conflict"));}
                return r.public();
            }
            sql_query("UPDATE hagency_agent_v1.configuration_requests SET state=$2,result=$3,error_code=$4,completed_at_ms=$5 WHERE id=$1")
                .bind::<Text,_>(id).bind::<Text,_>(&input.state).bind::<Nullable<Text>,_>(&result).bind::<Nullable<Text>,_>(&input.error_code).bind::<BigInt,_>(now).execute(db).await?;
            if op==ConfigurationOperation::Environment && let Some(environment)=result {
                sql_query("INSERT INTO hagency_agent_v1.configuration_observations(agent_id,agent_generation,execution_device_id,device_generation,observed_at_ms,environment) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(agent_id) DO UPDATE SET agent_generation=EXCLUDED.agent_generation,execution_device_id=EXCLUDED.execution_device_id,device_generation=EXCLUDED.device_generation,observed_at_ms=EXCLUDED.observed_at_ms,environment=EXCLUDED.environment")
                    .bind::<Text,_>(&r.agent_id).bind::<BigInt,_>(r.agent_generation).bind::<Text,_>(target).bind::<BigInt,_>(generation).bind::<BigInt,_>(now).bind::<Text,_>(&environment).execute(db).await?;
            } else if op.mutation() && input.state=="succeeded" {
                // A saved profile/provider/policy invalidates an earlier environment snapshot.
                sql_query("DELETE FROM hagency_agent_v1.configuration_observations WHERE agent_id=$1").bind::<Text,_>(&r.agent_id).execute(db).await?;
            }
            row(db,&p.user_id,id).await?.public()
        }).await
    }
}
