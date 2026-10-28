//! Encrypted task preparation/ACK. No plaintext or cryptographic trust flag is accepted.
use super::*;
use diesel::sql_types::Jsonb;
use serde_json::{Value, json};
pub(crate) const CAPABILITY: &str = "agent-encrypted-admission-v1";
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Projection {
    pub thread_root: Option<String>,
    pub mentioned_mxids: BTreeSet<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prepare {
    pub lease: LeaseRef,
    pub inbox_id: String,
    pub source_digest: String,
    pub projection: Projection,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reference {
    pub lease: LeaseRef,
    pub inbox_id: String,
    pub source_digest: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Admission {
    pub inbox_id: String,
    pub source_digest: String,
    pub agent_generation: i64,
    pub owner_direct: bool,
    pub dispatch: Dispatch,
}
#[derive(diesel::QueryableByName)]
struct Mapping {
    #[diesel(sql_type=Text)]
    dispatch_id: String,
    #[diesel(sql_type=Text)]
    source_digest: String,
    #[diesel(sql_type=Jsonb)]
    projection: Value,
}
impl TransportStore {
    pub(crate) async fn prepare_ciphertext(
        &self,
        p: &Principal,
        b: &Prepare,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<Admission> {
        key(&b.inbox_id)?;
        if b.projection.mentioned_mxids.len() > 128 {
            return Err(Error::Invalid("invalid_encrypted_projection"));
        }
        for id in &b.projection.mentioned_mxids {
            matrix_id(id, '@')?;
        }
        if let Some(root) = &b.projection.thread_root {
            matrix_id(root, '$')?;
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
   let(c,now)=Self::current_ciphertext(db,p,&b.lease,&b.inbox_id,f,self.limits.event_ttl_ms,now).await?;
   if c.source_digest!=b.source_digest{return Err(Error::Conflict("ciphertext_changed"));}
   let scope=Self::scope(db,&c.binding_id).await?;let owner_direct=scope.space_id.is_empty();
   let root=b.projection.thread_root.as_deref().unwrap_or(if owner_direct{&c.room_id}else{&c.event_id});
   let projection=serde_json::to_value(&b.projection).map_err(|_|Error::Invalid("invalid_encrypted_projection"))?;
   let existing=sql_query("SELECT dispatch_id,source_digest,projection FROM hagency_agent_v1.encrypted_dispatches WHERE inbox_id=$1").bind::<Text,_>(&c.id).get_result::<Mapping>(db).await.optional()?;
   let id=if let Some(old)=existing{
    if old.source_digest!=c.source_digest||old.projection!=projection{return Err(Error::Conflict("encrypted_projection_changed"));}
    let e=Self::dispatch(db,p,&old.dispatch_id).await?;
    if e.state=="encrypted_pending"{sql_query("UPDATE hagency_agent_v1.owner_events SET state='encrypted_offered',dispatch_epoch=$1,dispatch_device_id=$2 WHERE id=$3 AND execution_id IS NULL").bind::<BigInt,_>(b.lease.epoch).bind::<Text,_>(device(p)?.0).bind::<Text,_>(&old.dispatch_id).execute(db).await?;}
    else if e.execution_id.is_none()&&(e.dispatch_epoch!=Some(b.lease.epoch)||e.dispatch_device_id.as_deref()!=Some(device(p)?.0)){return Err(Error::Conflict("dispatch_not_claimable"));}
    old.dispatch_id
   }else{
    if !owner_direct&&!b.projection.mentioned_mxids.contains(&scope.puppet_mxid){
     let known=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.agent_threads WHERE binding_id=$1 AND thread_root=$2) AS matched").bind::<Text,_>(&scope.binding_id).bind::<Text,_>(root).get_result::<Flag>(db).await?.matched;
     if !scope.thread_auto_reply||b.projection.thread_root.is_none()||!known{return Err(Error::Conflict("encrypted_message_not_addressed"));}
    }
    let seal=ciphertexts::seal_digest(&c)?;
    let replay=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.encrypted_dispatches WHERE seal_digest=$1) AS matched").bind::<Text,_>(&seal).get_result::<Flag>(db).await?.matched;
    if replay{return Err(Error::Conflict("encrypted_ciphertext_replayed"));}
    let id=format!("evt_{}",entity_id()?);let body=ciphertexts::canonical(&c.event)?;let digest=hash(&ciphertexts::canonical(&json!(["agent-encrypted-admission-v1",c.source_digest,projection]))?);
    sql_query("INSERT INTO hagency_agent_v1.encrypted_dispatches(dispatch_id,inbox_id,source_digest,projection,seal_digest) VALUES($1,$2,$3,$4,$5)").bind::<Text,_>(&id).bind::<Text,_>(&c.id).bind::<Text,_>(&c.source_digest).bind::<Jsonb,_>(&projection).bind::<Text,_>(&seal).execute(db).await?;
    sql_query("INSERT INTO hagency_agent_v1.owner_events(id,binding_id,agent_id,owner_user_id,event_id,room_id,requester_mxid,thread_root,body,digest,binding_generation,state,dispatch_epoch,dispatch_device_id,created_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'encrypted_offered',$12,$13,$14)")
     .bind::<Text,_>(&id).bind::<Text,_>(&c.binding_id).bind::<Text,_>(&c.agent_id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&c.event_id).bind::<Text,_>(&c.room_id).bind::<Text,_>(&c.sender_mxid).bind::<Text,_>(root).bind::<Text,_>(&body).bind::<Text,_>(digest).bind::<BigInt,_>(c.binding_generation).bind::<BigInt,_>(b.lease.epoch).bind::<Text,_>(device(p)?.0).bind::<BigInt,_>(c.received_at_ms).execute(db).await?;
    sql_query("INSERT INTO hagency_agent_v1.agent_threads(binding_id,thread_root) VALUES($1,$2) ON CONFLICT DO NOTHING").bind::<Text,_>(&c.binding_id).bind::<Text,_>(root).execute(db).await?;
    id
   };
   let current=Self::clock(db,now).await?;Self::lease(db,p,&b.lease,current).await?;
   Ok(Admission{inbox_id:c.id,source_digest:c.source_digest,agent_generation:c.agent_generation,owner_direct,dispatch:Self::dispatch(db,p,&id).await?})
  }).await
    }
    pub(crate) async fn ciphertext_admission(
        &self,
        p: &Principal,
        b: &Reference,
        f: &DeliveryFacts,
        ack: bool,
        now: i64,
    ) -> Result<Admission> {
        key(&b.inbox_id)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
   let(c,now)=Self::current_ciphertext(db,p,&b.lease,&b.inbox_id,f,self.limits.event_ttl_ms,now).await?;
   if c.source_digest!=b.source_digest{return Err(Error::Conflict("ciphertext_changed"));}
   let m=sql_query("SELECT dispatch_id,source_digest,projection FROM hagency_agent_v1.encrypted_dispatches WHERE inbox_id=$1 AND source_digest=$2").bind::<Text,_>(&c.id).bind::<Text,_>(&c.source_digest).get_result::<Mapping>(db).await.map_err(unauthorized)?;
   let e=Self::dispatch(db,p,&m.dispatch_id).await?;
   if ack{
    fenced(&e,p,&b.lease)?;
    if !matches!(e.state.as_str(),"encrypted_offered"|"encrypted_acknowledged"){return Err(Error::Conflict("encrypted_dispatch_not_offered"));}
    if e.state=="encrypted_offered"{sql_query("UPDATE hagency_agent_v1.owner_events SET state='encrypted_acknowledged' WHERE id=$1").bind::<Text,_>(&e.id).execute(db).await?;}
   }
   let current=Self::clock(db,now).await?;Self::lease(db,p,&b.lease,current).await?;
   let scope=Self::scope(db,&c.binding_id).await?;
   Ok(Admission{inbox_id:c.id,source_digest:c.source_digest,agent_generation:c.agent_generation,owner_direct:scope.space_id.is_empty(),dispatch:Self::dispatch(db,p,&e.id).await?})
  }).await
    }
}
