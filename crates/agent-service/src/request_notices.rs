//! Durable per-request feedback. Does not consume model quota, start/replay work,
//! or require the executing device to be online. Fresh Matrix authority still applies.
use super::*;

const SELECT: &str = "SELECT n.id,n.owner_event_id,n.kind,n.matrix_txn_id,n.state,n.worker_token,e.binding_id,e.requester_mxid,e.event_id,e.thread_root,e.binding_generation,coalesce(d.name,'') AS device_name,CASE WHEN a.execution_device_id IS NULL THEN 'unassigned' WHEN d.id IS NULL OR d.revoked OR s.revoked OR s.valid_until_ms<=(extract(epoch from clock_timestamp())*1000)::bigint THEN 'offline' WHEN l.expires_at_ms>(extract(epoch from clock_timestamp())*1000)::bigint AND l.device_generation=d.generation THEN 'ready' WHEN p.device_generation=d.generation AND p.last_seen_at_ms BETWEEN (extract(epoch from clock_timestamp())*1000)::bigint-15000 AND (extract(epoch from clock_timestamp())*1000)::bigint THEN 'not_running' WHEN p.device_generation=d.generation AND p.last_seen_at_ms IS NOT NULL THEN 'offline' ELSE 'unobserved' END AS device_state FROM hagency_agent_v1.request_notice_outbox n JOIN hagency_agent_v1.owner_events e ON e.id=n.owner_event_id JOIN hagency_agent_v1.agents a ON a.id=e.agent_id LEFT JOIN hagency_agent_v1.devices d ON d.id=a.execution_device_id AND d.user_id=e.owner_user_id LEFT JOIN hagency_agent_v1.sessions s ON s.id=d.session_id AND s.user_id=e.owner_user_id LEFT JOIN hagency_agent_v1.configuration_presence p ON p.device_id=d.id LEFT JOIN hagency_agent_v1.execution_leases l ON l.agent_id=a.id AND l.device_id=d.id";

#[cfg(test)]
pub(crate) fn body(kind: &str) -> &'static str {
    body_for(kind, "unobserved")
}
pub(crate) fn body_for(kind: &str, device: &str) -> &'static str {
    match kind {
        "waiting" => match device {
            "offline" => {
                "⏳ Request queued. No recent heartbeat from the execution device. Processing has not started. Check the device and Agent service; this request may expire while waiting."
            }
            "not_running" => {
                "⏳ Request queued. The execution device is connected, but no active Agent runtime was observed. Check Service and its logs. This request may expire while waiting."
            }
            "unassigned" => {
                "⏳ Request queued. No execution device is assigned to this Agent. Assign a device and configure its service. This request may expire while waiting."
            }
            "ready" => {
                "⏳ Request queued. The Agent runtime is connected, but has not started this request. This request may expire while waiting."
            }
            _ => {
                "⏳ Request queued. Processing has not started. Check the execution device and Service logs. This request may expire while waiting."
            }
        },
        "processing" => "⏳ The Agent is still processing this request. No need to send it again.",
        "delivery" => {
            "⏳ Processing finished, but the reply has not been delivered. Delivery is being retried."
        }
        "failed" => {
            "⚠️ This request failed without a reply. Check the Agent's Service logs for the cause."
        }
        "rejected" => "⏸️ This request was not authorized for processing.",
        "expired" => {
            "⚠️ This request expired before processing started. You can send a new request."
        }
        _ => {
            "⚠️ Processing was interrupted and its result is unconfirmed. This request will not be run again automatically."
        }
    }
}

#[derive(Debug, diesel::QueryableByName)]
pub(crate) struct RequestNotice {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub owner_event_id: String,
    #[diesel(sql_type=Text)]
    pub kind: String,
    #[diesel(sql_type=Text)]
    pub matrix_txn_id: String,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=Nullable<Text>)]
    pub worker_token: Option<String>,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    pub requester_mxid: String,
    #[diesel(sql_type = Text)]
    pub event_id: String,
    #[diesel(sql_type=Text)]
    pub thread_root: String,
    #[diesel(sql_type=BigInt)]
    pub binding_generation: i64,
    #[diesel(sql_type=Text)]
    pub device_name: String,
    #[diesel(sql_type=Text)]
    pub device_state: String,
}
impl TransportStore {
    pub(crate) async fn request_notice_candidates(
        &self,
        cursor: &str,
        now: i64,
    ) -> Result<Vec<RequestNotice>> {
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;
            let now=Self::clock(db,now).await?;
            // A circular scan bounds maintenance and cannot be pinned by one Room.
            sql_query("INSERT INTO hagency_agent_v1.request_notice_outbox(id,owner_event_id,kind,matrix_txn_id,state) SELECT 'status_'||v.owner_event_id||'_'||v.kind,v.owner_event_id,v.kind,'hg-status-'||v.owner_event_id||'-'||v.kind,'pending' FROM hagency_agent_v1.request_notice_needed v JOIN hagency_agent_v1.bindings b ON b.id=v.binding_id WHERE v.kind IS NOT NULL AND b.state='active' AND b.generation=v.binding_generation AND NOT b.admin_project_paused AND NOT b.admin_room_paused AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.request_notice_outbox n WHERE n.owner_event_id=v.owner_event_id AND n.kind=v.kind) ORDER BY (('status_'||v.owner_event_id||'_'||v.kind)<=$1),v.owner_event_id LIMIT 20 ON CONFLICT DO NOTHING")
                .bind::<Text,_>(cursor).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.request_notice_outbox n SET state='cancelled' WHERE n.state IN ('pending','sending','unknown') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.request_notice_needed v JOIN hagency_agent_v1.bindings b ON b.id=v.binding_id WHERE v.owner_event_id=n.owner_event_id AND v.kind=n.kind AND b.state='active' AND b.generation=v.binding_generation AND NOT b.admin_project_paused AND NOT b.admin_room_paused)").execute(db).await?;
            Ok(sql_query(format!("{SELECT} WHERE n.state IN ('pending','unknown','sending') AND n.worker_until_ms<=$1 ORDER BY (n.id<=$2),n.id LIMIT 20"))
                .bind::<BigInt,_>(now).bind::<Text,_>(cursor).load(db).await?)
        }).await
    }
    pub(crate) async fn claim_request_notice(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<RequestNotice> {
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let now=Self::clock(db,now).await?;
            let n=sql_query(format!("{SELECT} WHERE n.id=$1")).bind::<Text,_>(id).get_result::<RequestNotice>(db).await?;
            let scope=Self::scope(db,&n.binding_id).await?;
            let needed=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.request_notice_needed WHERE owner_event_id=$1 AND kind=$2) AS matched")
                .bind::<Text,_>(&n.owner_event_id).bind::<Text,_>(&n.kind).get_result::<Flag>(db).await?.matched;
            let authorized=if needed && scope.binding_generation==n.binding_generation {
                match facts(&scope,&n.requester_mxid,f,now) {
                    Ok(())=>true,Err(e @ Error::Unavailable(_))=>return Err(e),Err(_)=>false
                }
            } else {false};
            if !authorized {
                sql_query("UPDATE hagency_agent_v1.request_notice_outbox SET state='cancelled' WHERE id=$1 AND state NOT IN ('sent','cancelled')")
                    .bind::<Text,_>(id).execute(db).await?;
                return Ok(None);
            }
            if !matches!(n.state.as_str(),"pending"|"unknown"|"sending") {return Ok(None);}
            let worker=secret_token();
            let claimed=sql_query("UPDATE hagency_agent_v1.request_notice_outbox SET state='sending',worker_token=$2,worker_until_ms=$3 WHERE id=$1 AND worker_until_ms<=$4")
                .bind::<Text,_>(id).bind::<Text,_>(worker).bind::<BigInt,_>(now+self.limits.worker_lease_ms).bind::<BigInt,_>(now).execute(db).await?;
            if claimed==0 {return Ok(None);}
            Ok(Some(sql_query(format!("{SELECT} WHERE n.id=$1")).bind::<Text,_>(id).get_result::<RequestNotice>(db).await?))
        }).await?.ok_or(Error::Conflict("request_notice_not_sendable"))
    }
    pub(crate) async fn confirm_request_notice(
        &self,
        id: &str,
        worker: &str,
        outcome: &Result<String>,
        now: i64,
    ) -> Result<()> {
        let mut guard = self.db.lock().await?;
        let state = match outcome {
            Ok(_) => "sent",
            Err(Error::Conflict("matrix_permission_missing")) => "cancelled",
            Err(_) => "unknown",
        };
        sql_query("UPDATE hagency_agent_v1.request_notice_outbox SET state=$3,matrix_event_id=$4,worker_token=NULL,worker_until_ms=$5 WHERE id=$1 AND worker_token=$2 AND state='sending'")
            .bind::<Text,_>(id).bind::<Text,_>(worker).bind::<Text,_>(state)
            .bind::<Nullable<Text>,_>(outcome.as_ref().ok().map(String::as_str))
            .bind::<BigInt,_>(if state=="unknown" {now+5000} else {0}).execute(&mut *guard).await?;
        Ok(())
    }
}
