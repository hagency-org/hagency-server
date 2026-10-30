//! Authenticated, finite permission requests bound to one running work item.
use super::*;
use serde::Deserialize;
use serde_json::{Value,json};
#[path="management_shared_chat_status.rs"]
mod chat_status;
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct RegisterApproval {
    authority: SharedActivityRef, work_id:String, work_digest:String, id:String,
    request:Value, remaining_ms:i64,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct TakeApproval {authority:SharedActivityRef,id:String,digest:String}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct ApprovalQuery {room_id:Option<String>}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct DecideApproval {id:String,digest:String,decision:String}
#[derive(QueryableByName)]
struct Row {
 #[diesel(sql_type=Text)] id:String,
 #[diesel(sql_type=Text)] work_id:String,
 #[diesel(sql_type=Text)] runtime_id:String,
 #[diesel(sql_type=Text)] owner_user_id:String,
 #[diesel(sql_type=Text)] requester_mxid:String,
 #[diesel(sql_type=Text)] payload:String,
 #[diesel(sql_type=Text)] digest:String,
 #[diesel(sql_type=BigInt)] expires_at_ms:i64,
 #[diesel(sql_type=Nullable<Text>)] decision:Option<String>,
 #[diesel(sql_type=Nullable<Bool>)] decision_by_owner:Option<bool>,
 #[diesel(sql_type=Bool)] consumed:bool,
}
async fn row(db:&mut AsyncPgConnection,id:&str)->Result<Row>{sql_query("SELECT * FROM hagency_agent_v1.management_shared_approvals WHERE id=$1 FOR UPDATE").bind::<Text,_>(id).get_result(db).await.map_err(hidden)}
async fn live_request(db:&mut AsyncPgConnection,a:&Row,server:&str,now:i64)->Result<bool>{
 let r=runtime(db,&a.runtime_id).await?;let w=work_row(db,&a.work_id).await?;
 let original=original(db,&r).await?;let c=command(&r,server)?;
 Ok(a.expires_at_ms>now && !a.consumed && w.state=="started" && w.runtime_id==r.id && r.state=="qualified" && r.activity_until_ms>now && current(db,&r,&original,&c,now).await?)
}
impl DomainStore {
 pub async fn register_shared_approval(&self,p:&Principal,input:RegisterApproval)->Result<Value>{
  key(&input.id)?;key(&input.work_id)?;
  if !(1..=120_000).contains(&input.remaining_ms) || serde_json::to_vec(&input.request).map_or(true,|v|v.len()>64*1024) {return Err(Error::Invalid("invalid_permission_request"));}
  let request=&input.request;
  if !matches!(request["method"].as_str(),Some("item/commandExecution/requestApproval"|"item/fileChange/requestApproval"|"item/permissions/requestApproval")) || !request["params"].is_object() || !request["sessionAllowed"].is_boolean() || !request["managerCanDecide"].is_boolean() || ["threadId","turnId","itemId"].iter().any(|k|request["params"][k].as_str().is_none_or(|s|s.is_empty()||s.len()>255)) {return Err(Error::Invalid("invalid_permission_request"));}
  let mut guard=self.db.lock().await?;
  (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
   let now=clock(db).await?;let r=runtime(db,&input.authority.runtime_command_id).await?;
   let c=require_activity(db,p,&r,&input.authority,&self.server,now).await?;
   let raw:Value=decode(&r.payload)?;
   if raw["toolPolicy"]["mode"]!="standard" {return Err(Error::Unauthorized("approval_mode_required"));}
   let w=work_row(db,&input.work_id).await?;let work=item(&w,&r)?;
   if w.state!="started" || w.digest!=input.work_digest {return Err(Error::Conflict("approval_work_changed"));}
   let old=sql_query("SELECT * FROM hagency_agent_v1.management_shared_approvals WHERE id=$1").bind::<Text,_>(&input.id).get_result::<Row>(db).await.optional()?;
   if let Some(old)=old {let payload:Value=decode(&old.payload)?;if old.work_id!=w.id || old.owner_user_id!=p.user_id || payload["request"]!=input.request{return Err(Error::Conflict("approval_request_changed"));}return Ok(json!({"approval":payload,"digest":old.digest}));}
   if sql_query("SELECT count(*)>=16 AS matched FROM hagency_agent_v1.management_shared_approvals WHERE runtime_id=$1 AND NOT consumed AND expires_at_ms>$2").bind::<Text,_>(&r.id).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched {return Err(Error::Unavailable("approval_queue_full"));}
   let expires=(now+input.remaining_ms).min(reserve_lifetime(db,&c.reserve_command,now).await?);
   let payload=json!({"id":input.id,"workId":w.id,"agentId":c.reserve_command.proposed_agent_id,"agentName":c.reserve_command.definition.name,"roomId":work.room_id,"eventId":work.event_id,"threadRoot":work.thread_root,"requesterMxid":work.requester_mxid,"ownerMxid":p.mxid,"request":input.request,"expiresAtMs":expires,"sessionScope":"current_codex_session"});let digest=allocation_digest(&payload)?;
   // Other Room members may send the work, but manager-delegable decisions stay with the manager.
   sql_query("INSERT INTO hagency_agent_v1.management_shared_approvals(id,work_id,runtime_id,owner_user_id,requester_mxid,room_id,payload,digest,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind::<Text,_>(&input.id).bind::<Text,_>(&w.id).bind::<Text,_>(&r.id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&c.reserve_command.manager_mxid).bind::<Text,_>(&work.room_id).bind::<Text,_>(wire(&payload)?).bind::<Text,_>(&digest).bind::<BigInt,_>(expires).execute(db).await?;
   Ok(json!({"approval":payload,"digest":digest}))
  }).await
 }
 pub async fn shared_approvals(&self,p:&Principal,input:ApprovalQuery)->Result<Value>{
  if input.room_id.as_ref().is_some_and(|s|s.len()>255 || !s.starts_with('!')) {return Err(Error::Invalid("invalid_room"));}
  let mut db=self.db.lock().await?;let now=clock(&mut db).await?;
  let rows=sql_query("SELECT * FROM hagency_agent_v1.management_shared_approvals WHERE (owner_user_id=$1 OR requester_mxid=$2) AND ($3::text IS NULL OR room_id=$3) AND NOT consumed AND expires_at_ms>$4 ORDER BY expires_at_ms,id LIMIT 16").bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Nullable<Text>,_>(&input.room_id).bind::<BigInt,_>(now).load::<Row>(&mut *db).await?;
  let mut result=Vec::new();for a in rows {if live_request(&mut db,&a,&self.server,now).await? {let payload:Value=decode(&a.payload)?;let can=a.owner_user_id==p.user_id || (a.requester_mxid==p.mxid && payload["request"]["managerCanDecide"]==true);result.push(json!({"approval":payload,"digest":a.digest,"decision":a.decision,"canDecide":can}));}}
  let executions=chat_status::query(&mut db,p,input.room_id.as_deref(),now).await?;
  Ok(json!({"approvals":result,"executions":executions}))
 }
 pub async fn decide_shared_approval(&self,p:&Principal,input:DecideApproval)->Result<Value>{
  key(&input.id)?;
  if !matches!(input.decision.as_str(),"approve_once"|"approve_session"|"deny") {return Err(Error::Invalid("invalid_permission_decision"));}
  let mut guard=self.db.lock().await?;
  (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
   let a=row(db,&input.id).await?;
   let reviewed:Value=decode(&a.payload)?;
   if a.owner_user_id!=p.user_id && !(a.requester_mxid==p.mxid && reviewed["request"]["managerCanDecide"]==true) {return Err(Error::Unauthorized("resource_owner_required"));}
   if a.digest!=input.digest {return Err(Error::Conflict("approval_request_changed"));}
   let payload:Value=decode(&a.payload)?;
   if input.decision=="approve_session" && payload["request"]["sessionAllowed"]!=true {return Err(Error::Invalid("session_approval_unavailable"));}
   if let Some(decision)=&a.decision {if decision==&input.decision {return Ok(json!({"decision":decision}));}return Err(Error::Conflict("approval_already_decided"));}
   let now=clock(db).await?;if !live_request(db,&a,&self.server,now).await? {return Err(Error::Conflict("approval_expired"));}
   sql_query("UPDATE hagency_agent_v1.management_shared_approvals SET decision=$2,decision_by_owner=$3,decided_by_mxid=$4,decided_at_ms=$5 WHERE id=$1").bind::<Text,_>(&a.id).bind::<Text,_>(&input.decision).bind::<Bool,_>(a.owner_user_id==p.user_id).bind::<Text,_>(&p.mxid).bind::<BigInt,_>(now).execute(db).await?;
   Ok(json!({"decision":input.decision}))
  }).await
 }
 pub async fn take_shared_approval(&self,p:&Principal,input:TakeApproval)->Result<Value>{
  key(&input.id)?;let mut guard=self.db.lock().await?;
  (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
   let a=row(db,&input.id).await?;let r=runtime(db,&a.runtime_id).await?;let now=clock(db).await?;
   require_activity(db,p,&r,&input.authority,&self.server,now).await?;
   if a.digest!=input.digest || a.owner_user_id!=p.user_id {return Err(Error::Unauthorized("approval_request_changed"));}
   if !live_request(db,&a,&self.server,now).await? {return Err(Error::Conflict("approval_expired"));}
   if a.decision.is_some() {sql_query("UPDATE hagency_agent_v1.management_shared_approvals SET consumed=true WHERE id=$1 AND NOT consumed").bind::<Text,_>(&a.id).execute(db).await?;}
   Ok(json!({"id":a.id,"digest":a.digest,"decision":a.decision,"ownerApproved":a.decision_by_owner}))
  }).await
 }
}
