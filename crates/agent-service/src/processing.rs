//! Fixed, durable processing reactions. Never accepts a Matrix target from a device.
use super::*;
use diesel::{BoolExpressionMethods, ExpressionMethods, IntoSql, QueryDsl};
use diesel::{QueryableByName, sql_types::Jsonb};
use serde_json::{Value, json};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingReceipt {
    pub id: String,
    pub dispatch_id: String,
    pub execution_id: String,
    pub state: String,
    pub matrix_event_id: Option<String>,
}
#[derive(diesel::Queryable, QueryableByName)]
pub(crate) struct ProcessingIntent {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub owner_event_id: String,
    #[diesel(sql_type=Text)]
    pub execution_id: String,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub puppet_mxid: String,
    #[diesel(sql_type=Text)]
    pub matrix_txn_id: String,
    #[diesel(sql_type=Jsonb)]
    pub content: Value,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=Bool)]
    pub delivery_blocked: bool,
    #[diesel(sql_type=Nullable<Text>)]
    pub worker_token: Option<String>,
    #[diesel(sql_type=BigInt)]
    pub worker_until_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    pub matrix_event_id: Option<String>,
    #[diesel(sql_type=Text)]
    pub requester_mxid: String,
}
impl ProcessingIntent {
    fn receipt(self) -> ProcessingReceipt {
        ProcessingReceipt {
            id: self.id,
            dispatch_id: self.owner_event_id,
            execution_id: self.execution_id,
            state: self.state,
            matrix_event_id: self.matrix_event_id,
        }
    }
}
const SELECT: &str = "SELECT o.id,o.owner_event_id,o.execution_id,o.binding_id,o.room_id,o.puppet_mxid,o.matrix_txn_id,o.content,o.state,o.delivery_blocked,o.worker_token,o.worker_until_ms,o.matrix_event_id,e.requester_mxid FROM hagency_agent_v1.processing_outbox o JOIN hagency_agent_v1.owner_events e ON e.id=o.owner_event_id";
impl TransportStore {
    async fn processing(db: &mut AsyncPgConnection, id: &str) -> Result<ProcessingIntent> {
        Ok(sql_query(format!("{SELECT} WHERE o.id=$1"))
            .bind::<Text, _>(id)
            .get_result(db)
            .await?)
    }
    /// Enqueue only after the device has actually begun this running execution.
    /// A completed same-attempt retry can read an existing receipt, never insert.
    pub async fn mark_processing(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        dispatch_id: &str,
        execution_id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<ProcessingReceipt> {
        key(execution_id)?;
        self.observe_event_authority(p, dispatch_id, f, now).await?;
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            let (now,_)=Self::authenticate(db,p,now).await?;
            let e=Self::dispatch(db,p,dispatch_id).await?;
            let scope=Self::dispatch_scope(db,p,lease,&e,f,now).await?;
            fenced(&e,p,lease)?;
            if e.execution_id.as_deref()!=Some(execution_id)||!matches!(e.state.as_str(),"running"|"completed") {
                return Err(Error::Conflict("execution_not_running"));
            }
            let existing=sql_query(format!("{SELECT} WHERE o.owner_event_id=$1"))
                .bind::<Text,_>(dispatch_id).get_result::<ProcessingIntent>(db).await.optional()?;
            if let Some(existing)=existing {return Ok(existing.receipt());}
            if e.state!="running" {return Err(Error::Conflict("execution_not_running"));}
            Self::event_unexpired(db,dispatch_id,self.limits.event_ttl_ms,now).await?;
            let id=format!("processing_{}",hash(dispatch_id));
            let txn=format!("hg-processing-{}",hash(dispatch_id));
            let content=json!({"m.relates_to":{"rel_type":"m.annotation","event_id":e.event_id,"key":"👀"}});
            let current=Self::clock(db,now).await?;
            Self::lease(db,p,lease,current).await?;
            facts(&scope,&e.requester_mxid,f,current)?;
            {
                use crate::schema::agent_tables::processing_outbox as q;
                let params = (
                    &id,
                    dispatch_id,
                    execution_id,
                    &e.agent_id,
                    &e.binding_id,
                    &p.user_id,
                    &e.room_id,
                    &e.event_id,
                    &scope.puppet_mxid,
                    e.binding_generation,
                    lease.epoch,
                    device(p)?.0,
                    content,
                    txn,
                    current,
                );
                diesel::insert_into(q::table)
                    .values((
                        q::id.eq(&params.0),
                        q::owner_event_id.eq(&params.1),
                        q::execution_id.eq(&params.2),
                        q::agent_id.eq(&params.3),
                        q::binding_id.eq(&params.4),
                        q::owner_user_id.eq(&params.5),
                        q::room_id.eq(&params.6),
                        q::event_id.eq(&params.7),
                        q::puppet_mxid.eq(&params.8),
                        q::binding_generation.eq(&params.9),
                        q::dispatch_epoch.eq(&params.10),
                        q::dispatch_device_id.eq(&params.11),
                        q::content.eq(&params.12),
                        q::matrix_txn_id.eq(&params.13),
                        q::state.eq("pending"),
                        q::created_at_ms.eq(&params.14),
                    ))
                    .execute(db)
                    .await
            }?;
            Ok(Self::processing(db,&id).await?.receipt())
        }).await
    }).await
    }
    /// Circular trusted scan; failed Rooms do not monopolize the bounded batch.
    pub(crate) async fn processing_candidates(
        &self,
        cursor: &str,
        limit: usize,
        now: i64,
    ) -> Result<Vec<ProcessingIntent>> {
        if limit == 0 || limit > self.limits.max_batch {
            return Err(Error::Invalid("invalid_batch_limit"));
        }
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;
            sql_query("UPDATE hagency_agent_v1.processing_outbox o SET state='cancelled' WHERE state='pending' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.processing_sendable v WHERE v.id=o.id)").execute(db).await?;
            Ok(sql_query(format!("{SELECT} WHERE o.state IN ('pending','unknown','sending') AND NOT o.delivery_blocked AND o.worker_until_ms<=greatest($1,(extract(epoch from clock_timestamp())*1000)::bigint) AND EXISTS(SELECT 1 FROM hagency_agent_v1.processing_sendable v WHERE v.id=o.id) ORDER BY (o.id<=$2),o.id LIMIT $3"))
                .bind::<BigInt,_>(now).bind::<Text,_>(cursor).bind::<BigInt,_>(limit as i64).load(db).await?)
        }).await
    }).await
    }
    #[cfg(test)]
    pub(crate) async fn claim_processing(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<ProcessingIntent> {
        self.claim_processing_guarded(id, f, now, None).await
    }
    pub(crate) async fn claim_processing_guarded(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
        gate: Option<&crate::worker_runtime::ClaimGate>,
    ) -> Result<ProcessingIntent> {
        let candidate = {
            self.db
                .with_connection(async |db| Self::processing(&mut *db, id).await)
                .await?
        };
        self.observe_delivery_authority(
            None,
            &candidate.binding_id,
            &candidate.requester_mxid,
            Some(&candidate.owner_event_id),
            f,
            now,
        )
        .await?;
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;
            {
                use crate::schema::agent_tables::processing_outbox as q;
                let params = (id,);
                q::table
                    .select((true.into_sql::<diesel::sql_types::Bool>(),))
                    .filter((q::id).eq(&params.0))
                    .for_update()
                    .get_result::<Flag>(db)
                    .await
            }?;
            let current=Self::clock(db,now).await?;
            let intent=Self::processing(db,id).await?;
            if intent.delivery_blocked||!matches!(intent.state.as_str(),"pending"|"unknown"|"sending") {return Err(Error::Conflict("processing_not_sendable"));}
            if intent.worker_until_ms>current {return Err(Error::Conflict("processing_sender_busy"));}
            let scope=Self::scope(db,&intent.binding_id).await?;
            facts(&scope,&intent.requester_mxid,f,current)?;
            let live=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.processing_sendable WHERE id=$1) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?;
            if !live.matched {return Err(Error::Unauthorized("processing_authority_expired"));}
            let _admission=gate.map(crate::worker_runtime::ClaimGate::admit).transpose()?;
            {
                use crate::schema::agent_tables::processing_outbox as q;
                let params = (secret_token(), current + self.limits.worker_lease_ms, id);
                diesel::update(q::table)
                    .filter((q::id).eq(&params.2))
                    .set((
                        q::state.eq("sending"),
                        q::worker_token.eq(&params.0),
                        q::worker_until_ms.eq(&params.1),
                    ))
                    .execute(db)
                    .await
            }?;
            Self::processing(db,id).await
        }).await
    }).await
    }
    /// A valid negative observation permanently prevents reaction retries.
    pub(crate) async fn block_processing(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<()> {
        let candidate = {
            self.db
                .with_connection(async |db| Self::processing(&mut *db, id).await)
                .await?
        };
        self.observe_delivery_authority(
            None,
            &candidate.binding_id,
            &candidate.requester_mxid,
            Some(&candidate.owner_event_id),
            f,
            now,
        )
        .await?;
        self.block_processing_observed(id, f, now).await
    }
    // The independently committed preflight above may be followed by a wait for
    // the transaction lock. Recheck evidence freshness inside this final stage.
    pub(super) async fn block_processing_observed(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<()> {
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let current=Self::clock(db,now).await?;
            let candidate=Self::processing(db,id).await?;
            let scope=Self::scope(db,&candidate.binding_id).await?;
            observation(&scope,&candidate.requester_mxid,f,current)?;
            if facts(&scope,&candidate.requester_mxid,f,current).is_ok() {return Ok(());}
            sql_query("UPDATE hagency_agent_v1.processing_outbox SET delivery_blocked=true,state=CASE WHEN state='pending' THEN 'cancelled' ELSE state END WHERE id=$1 AND state<>'sent'")
                .bind::<Text,_>(id).execute(db).await?;Ok(())
        }).await
    }).await
    }
    /// Recording an already issued HTTP result never grants new delivery authority.
    pub(crate) async fn confirm_processing(
        &self,
        id: &str,
        worker: &str,
        event: Option<&str>,
        permission_denied: bool,
        now: i64,
    ) -> Result<()> {
        if let Some(event) = event {
            matrix_id(event, '$')?;
        }
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::lock(db).await?;
                    let intent = Self::processing(db, id).await?;
                    if intent.worker_token.as_deref() != Some(worker) {
                        return Err(Error::Conflict("stale_processing_worker"));
                    }
                    if intent.state == "sent" && intent.matrix_event_id.as_deref() == event {
                        return Ok(());
                    }
                    if intent.state != "sending" {
                        return Err(Error::Conflict("processing_not_sending"));
                    }
                    {
                        use crate::schema::agent_tables::processing_outbox as q;
                        let params = (
                            if event.is_some() { "sent" } else { "unknown" },
                            event,
                            permission_denied,
                            Self::clock(db, now).await?,
                            id,
                        );
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.4))
                            .set((
                                q::state.eq(&params.0),
                                q::matrix_event_id.eq(&params.1),
                                q::delivery_blocked.eq((q::delivery_blocked)
                                    .or(params.2.into_sql::<diesel::sql_types::Bool>())),
                                q::worker_until_ms.eq(&params.3),
                            ))
                            .execute(db)
                            .await
                    }?;
                    Ok(())
                })
                .await
            })
            .await
    }
}
