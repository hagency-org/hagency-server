//! Encrypted task preparation/ACK. No plaintext or cryptographic trust flag is accepted.
use super::*;
use diesel::sql_types::Jsonb;
use diesel::{BoolExpressionMethods, ExpressionMethods, QueryDsl};
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
#[derive(diesel::Queryable, diesel::QueryableByName)]
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
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
   let(c,now)=Self::current_ciphertext(db,p,&b.lease,&b.inbox_id,f,self.limits.event_ttl_ms,now).await?;
   if c.source_digest!=b.source_digest{return Err(Error::Conflict("ciphertext_changed"));}
   let scope=Self::scope(db,&c.binding_id).await?;let owner_direct=scope.space_id.is_empty();
   let root=b.projection.thread_root.as_deref().unwrap_or(if owner_direct{&c.room_id}else{&c.event_id});
   let projection=serde_json::to_value(&b.projection).map_err(|_|Error::Invalid("invalid_encrypted_projection"))?;
   let existing={
       use crate::schema::agent_tables::encrypted_dispatches as q;
       let params = (&c.id,);
       q::table
           .select((q::dispatch_id, q::source_digest, q::projection))
           .filter((q::inbox_id).eq(&params.0))
           .get_result::<Mapping>(db)
           .await
   }.optional()?;
   let id=if let Some(old)=existing{
    if old.source_digest!=c.source_digest||old.projection!=projection{return Err(Error::Conflict("encrypted_projection_changed"));}
    let e=Self::dispatch(db,p,&old.dispatch_id).await?;
    if e.state=="encrypted_pending"{{
        use crate::schema::agent_tables::owner_events as q;
        let params = (b.lease.epoch, device(p)?.0, &old.dispatch_id);
        diesel::update(q::table)
            .filter(((q::id).eq(&params.2)).and(q::execution_id.is_null()))
            .set((
                q::state.eq("encrypted_offered"),
                q::dispatch_epoch.eq(&params.0),
                q::dispatch_device_id.eq(&params.1),
            ))
            .execute(db)
            .await
    }?;}
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
    {
        use crate::schema::agent_tables::encrypted_dispatches as q;
        let params = (&id, &c.id, &c.source_digest, &projection, &seal);
        diesel::insert_into(q::table)
            .values((
                q::dispatch_id.eq(&params.0),
                q::inbox_id.eq(&params.1),
                q::source_digest.eq(&params.2),
                q::projection.eq(&params.3),
                q::seal_digest.eq(&params.4),
            ))
            .execute(db)
            .await
    }?;
    {
        use crate::schema::agent_tables::owner_events as q;
        let params = (
            &id,
            &c.binding_id,
            &c.agent_id,
            &p.user_id,
            &c.event_id,
            &c.room_id,
            &c.sender_mxid,
            root,
            &body,
            digest,
            c.binding_generation,
            b.lease.epoch,
            device(p)?.0,
            c.received_at_ms,
        );
        diesel::insert_into(q::table)
            .values((
                q::id.eq(&params.0),
                q::binding_id.eq(&params.1),
                q::agent_id.eq(&params.2),
                q::owner_user_id.eq(&params.3),
                q::event_id.eq(&params.4),
                q::room_id.eq(&params.5),
                q::requester_mxid.eq(&params.6),
                q::thread_root.eq(&params.7),
                q::body.eq(&params.8),
                q::digest.eq(&params.9),
                q::binding_generation.eq(&params.10),
                q::state.eq("encrypted_offered"),
                q::dispatch_epoch.eq(&params.11),
                q::dispatch_device_id.eq(&params.12),
                q::created_at_ms.eq(&params.13),
            ))
            .execute(db)
            .await
    }?;
    {
        use crate::schema::agent_tables::agent_threads as q;
        let params = (&c.binding_id, root);
        diesel::insert_into(q::table)
            .values((q::binding_id.eq(&params.0), q::thread_root.eq(&params.1)))
            .on_conflict_do_nothing()
            .execute(db)
            .await
    }?;
    id
   };
   let current=Self::clock(db,now).await?;Self::lease(db,p,&b.lease,current).await?;
   Ok(Admission{inbox_id:c.id,source_digest:c.source_digest,agent_generation:c.agent_generation,owner_direct,dispatch:Self::dispatch(db,p,&id).await?})
  }).await
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
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    let (c, now) = Self::current_ciphertext(
                        db,
                        p,
                        &b.lease,
                        &b.inbox_id,
                        f,
                        self.limits.event_ttl_ms,
                        now,
                    )
                    .await?;
                    if c.source_digest != b.source_digest {
                        return Err(Error::Conflict("ciphertext_changed"));
                    }
                    let m = {
                        use crate::schema::agent_tables::encrypted_dispatches as q;
                        let params = (&c.id, &c.source_digest);
                        q::table
                            .select((q::dispatch_id, q::source_digest, q::projection))
                            .filter(
                                ((q::inbox_id).eq(&params.0)).and((q::source_digest).eq(&params.1)),
                            )
                            .get_result::<Mapping>(db)
                            .await
                    }
                    .map_err(unauthorized)?;
                    let e = Self::dispatch(db, p, &m.dispatch_id).await?;
                    if ack {
                        fenced(&e, p, &b.lease)?;
                        if !matches!(
                            e.state.as_str(),
                            "encrypted_offered" | "encrypted_acknowledged"
                        ) {
                            return Err(Error::Conflict("encrypted_dispatch_not_offered"));
                        }
                        if e.state == "encrypted_offered" {
                            {
                                use crate::schema::agent_tables::owner_events as q;
                                let params = (&e.id,);
                                diesel::update(q::table)
                                    .filter((q::id).eq(&params.0))
                                    .set((q::state.eq("encrypted_acknowledged"),))
                                    .execute(db)
                                    .await
                            }?;
                        }
                    }
                    let current = Self::clock(db, now).await?;
                    Self::lease(db, p, &b.lease, current).await?;
                    let scope = Self::scope(db, &c.binding_id).await?;
                    Ok(Admission {
                        inbox_id: c.id,
                        source_digest: c.source_digest,
                        agent_generation: c.agent_generation,
                        owner_direct: scope.space_id.is_empty(),
                        dispatch: Self::dispatch(db, p, &e.id).await?,
                    })
                })
                .await
            })
            .await
    }
}
