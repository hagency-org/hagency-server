//! Durable per-request feedback. Does not consume model quota, start/replay work,
//! or require the executing device to be online. Fresh Matrix authority still applies.
use super::*;

const SELECT: &str = "SELECT n.id,n.owner_event_id,n.kind,n.matrix_txn_id,n.state,n.worker_token,e.binding_id,e.requester_mxid,e.event_id,e.thread_root,e.binding_generation FROM hagency_agent_v1.request_notice_outbox n JOIN hagency_agent_v1.owner_events e ON e.id=n.owner_event_id";

pub(crate) fn body(kind: &str) -> &'static str {
    match kind {
        "waiting" => {
            "⏳ 请求已收到，Agent 当前尚未开始处理（可能离线或正在处理其他请求）。请求仍在等待，无需重复发送；恢复后会继续处理。"
        }
        "processing" => "⏳ Agent 仍在处理本次请求，耗时较长，请稍候。无需重复发送。",
        "delivery" => "⏳ 本次处理已结束，但回复暂未送达，系统正在重试发送。无需重复发送请求。",
        "failed" => "⚠️ 本次请求处理失败，未生成可发送的回复。后续请求仍可正常处理。",
        "rejected" => "⏸️ 本次请求未获处理授权。其他符合授权条件的请求仍可正常处理。",
        "expired" => "⚠️ 本次请求等待超时，尚未开始处理，已结束等待。你可以重新发送请求。",
        _ => {
            "⚠️ 本次请求处理已中断，暂时无法确认执行结果。为避免重复执行，系统不会自动重跑这条请求；你可以继续发送新请求。"
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
                match plaintext_sendable(&scope,&n.requester_mxid,f,now) {
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
