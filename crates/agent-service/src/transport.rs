//! Durable owner delivery and replies. Host-only facts are never accepted from HTTP
//! DTOs. ACK confirms local inbox persistence; execution start and completion are
//! separate durable transitions. Unknown executions are never automatically rerun.
use crate::{Error, Result, entity_id, hash, key, secret_token, store::Principal};
use diesel::{
    OptionalExtension, sql_query,
    sql_types::{BigInt, Bool, Nullable, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::Mutex;

#[path = "transport_history.rs"]
mod history;
#[path = "pause_notices.rs"]
pub(crate) mod pause_notices;
#[path = "processing.rs"]
mod processing;
pub use history::{ExecutionHistoryPage, ExecutionHistorySnapshot, HistoryExecution};
pub use processing::ProcessingReceipt;

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_lease_ms: i64,
    pub max_owner_events: i64,
    pub event_ttl_ms: i64,
    pub max_message_bytes: usize,
    pub max_batch: usize,
    pub worker_lease_ms: i64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_lease_ms: 60_000,
            max_owner_events: 10_000,
            event_ttl_ms: 86_400_000,
            max_message_bytes: 64 * 1024,
            max_batch: 100,
            worker_lease_ms: 30_000,
        }
    }
}
impl Limits {
    fn validate(&self) -> Result<()> {
        if !(1000..=120_000).contains(&self.max_lease_ms)
            || !(1..=1_000_000).contains(&self.max_owner_events)
            || !(1000..=2_592_000_000).contains(&self.event_ttl_ms)
            || !(1..=1024 * 1024).contains(&self.max_message_bytes)
            || !(1..=1000).contains(&self.max_batch)
            || !(1000..=120_000).contains(&self.worker_lease_ms)
        {
            return Err(Error::Invalid("invalid_transport_limits"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeaseRef {
    pub agent_id: String,
    pub epoch: i64,
}
#[derive(Debug, Serialize, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    #[diesel(sql_type=Text)]
    pub agent_id: String,
    #[diesel(sql_type=Text)]
    pub owner_user_id: String,
    #[diesel(sql_type=Text)]
    pub device_id: String,
    #[diesel(sql_type=BigInt)]
    pub device_generation: i64,
    #[diesel(sql_type=BigInt)]
    pub epoch: i64,
    #[diesel(sql_type=BigInt)]
    pub expires_at_ms: i64,
}
/// Canonical event parsed from a verified Appservice transaction, never from a
/// client request. Mentions and relation fields retain canonical Matrix meaning.
#[derive(Clone, Debug, Serialize)]
pub struct RoutedEvent {
    pub event_id: String,
    pub room_id: String,
    pub sender_mxid: String,
    pub body: String,
    pub mentioned_mxids: BTreeSet<String>,
    pub thread_root: Option<String>,
    pub encrypted: bool,
    pub is_edit: bool,
}
/// Trusted gateway observation of the exact owner, requester, puppet and Room.
#[derive(Clone, Debug)]
pub struct DeliveryFacts {
    pub owner_mxid: String,
    pub requester_mxid: String,
    pub puppet_mxid: String,
    pub room_id: String,
    pub space_id: String,
    pub observed_at_ms: i64,
    pub owner_in_space: bool,
    pub owner_in_room: bool,
    pub room_in_space: bool,
    pub requester_in_room: bool,
    pub puppet_in_room: bool,
    pub puppet_can_send_message: bool,
    pub encrypted: bool,
    pub owner_direct_valid: bool,
}
#[derive(Debug, Serialize, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct EventHeader {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    pub event_id: String,
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub requester_mxid: String,
    #[diesel(sql_type=Text)]
    pub thread_root: String,
}
#[derive(Debug, Serialize, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    pub agent_id: String,
    #[diesel(sql_type=Text)]
    pub event_id: String,
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub requester_mxid: String,
    #[diesel(sql_type=Text)]
    pub thread_root: String,
    #[diesel(sql_type=Text)]
    pub body: String,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=BigInt)]
    pub binding_generation: i64,
    #[diesel(sql_type=Nullable<BigInt>)]
    pub dispatch_epoch: Option<i64>,
    #[diesel(sql_type=Nullable<Text>)]
    pub dispatch_device_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    pub execution_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    pub outcome: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStart {
    pub dispatch: Dispatch,
    pub newly_started: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitReply {
    pub dispatch_id: String,
    pub execution_id: String,
    pub body: String,
}
#[derive(Debug, Serialize, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct ReplyIntent {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub owner_event_id: String,
    #[diesel(sql_type=Text)]
    pub requester_mxid: String,
    #[diesel(sql_type=Text)]
    pub agent_id: String,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    pub owner_user_id: String,
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub puppet_mxid: String,
    #[diesel(sql_type=Text)]
    pub thread_root: String,
    #[diesel(sql_type=Text)]
    pub body: String,
    #[diesel(sql_type=Text)]
    pub payload_digest: String,
    #[diesel(sql_type=Text)]
    pub matrix_txn_id: String,
    #[diesel(sql_type=BigInt)]
    pub binding_generation: i64,
    #[diesel(sql_type=BigInt)]
    pub dispatch_epoch: i64,
    /// Current explicit send authorization; original execution epoch stays immutable.
    #[diesel(sql_type=BigInt)]
    pub delivery_epoch: i64,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=Bool)]
    pub delivery_blocked: bool,
    #[diesel(sql_type=Nullable<Text>)]
    #[serde(skip_serializing)]
    pub worker_token: Option<String>,
    #[diesel(sql_type=BigInt)]
    pub worker_until_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    pub matrix_event_id: Option<String>,
}
/// Trusted homeserver observation of a prior send, not a client claim. The
/// exact original Matrix transaction ID must be proven; matching text alone is
/// insufficient evidence that this dispatch was sent.
#[derive(Clone, Debug)]
pub struct ObservedReply {
    pub event_id: String,
    pub matrix_txn_id: String,
    pub sender_mxid: String,
    pub room_id: String,
    pub thread_root: String,
    pub body: String,
    pub observed_at_ms: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteResult {
    Ignored,
    Queued { dispatch_id: String },
    Duplicate { dispatch_id: String },
}
#[derive(Clone, diesel::QueryableByName)]
pub struct RoutingScope {
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    pub owner_user_id: String,
    #[diesel(sql_type=Text)]
    pub owner_mxid: String,
    #[diesel(sql_type=Text)]
    pub puppet_mxid: String,
    #[diesel(sql_type=Text)]
    pub agent_id: String,
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub space_id: String,
    #[diesel(sql_type=BigInt)]
    pub binding_generation: i64,
    #[diesel(sql_type=Bool)]
    pub active: bool,
    #[diesel(sql_type=Bool)]
    pub service_paused: bool,
    #[diesel(sql_type=Bool)]
    pub thread_auto_reply: bool,
}
#[derive(diesel::QueryableByName)]
struct Flag {
    #[diesel(sql_type=Bool)]
    matched: bool,
}
#[derive(diesel::QueryableByName)]
struct Clock {
    #[diesel(sql_type=BigInt)]
    now_ms: i64,
}
#[derive(diesel::QueryableByName)]
struct Auth {
    #[diesel(sql_type=BigInt)]
    valid_until_ms: i64,
}
#[derive(diesel::QueryableByName)]
struct Count {
    #[diesel(sql_type=BigInt)]
    total: i64,
}
#[derive(diesel::QueryableByName)]
struct Existing {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    digest: String,
}
#[derive(Clone)]
pub struct TransportStore {
    db: Arc<Mutex<AsyncPgConnection>>,
    limits: Limits,
}
impl TransportStore {
    /// Initialize only new transport tables in the fixed new-domain database.
    pub async fn open(url: &str, limits: Limits) -> Result<Self> {
        limits.validate()?;
        let mut db = AsyncPgConnection::establish(url)
            .await
            .map_err(|_| Error::Unavailable("database_unavailable"))?;
        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328902)")
                .await?;
            let domain = sql_query(
                "SELECT to_regclass('hagency_agent_v1.domain_deployment') IS NOT NULL AS matched",
            )
            .get_result::<Flag>(db)
            .await?;
            if !domain.matched {
                return Err(Error::Conflict("agent_domain_not_initialized"));
            }
            let compatible = sql_query("SELECT (version=4) AS matched FROM hagency_agent_v1.domain_deployment WHERE singleton")
                .get_result::<Flag>(db).await?;
            if !compatible.matched { return Err(Error::Conflict("domain_schema_incompatible")); }
            let initialized = sql_query(
                "SELECT to_regclass('hagency_agent_v1.execution_leases') IS NOT NULL AS matched",
            )
            .get_result::<Flag>(db)
            .await?;
            if !initialized.matched {
                db.batch_execute(include_str!("transport_schema.sql"))
                    .await?;
                db.batch_execute(include_str!("processing_schema.sql")).await?;
            }
            let pause_initialized=sql_query("SELECT to_regclass('hagency_agent_v1.pause_notice_outbox') IS NOT NULL AS matched").get_result::<Flag>(db).await?;
            if !pause_initialized.matched {
                db.batch_execute(include_str!("pause_notices_schema.sql")).await?;
            }
            db.batch_execute("SELECT id FROM hagency_agent_v1.processing_sendable LIMIT 0").await
                .map_err(|_| Error::Conflict("processing_schema_incompatible"))?;
            db.batch_execute("SELECT delivery_epoch FROM hagency_agent_v1.reply_outbox LIMIT 0")
                .await
                .map_err(|_| Error::Conflict("transport_schema_incompatible"))?;
            Ok(())
        })
        .await?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            limits,
        })
    }
    async fn clock(db: &mut AsyncPgConnection, now: i64) -> Result<i64> {
        Ok(sql_query(
            "SELECT greatest($1,(extract(epoch from clock_timestamp())*1000)::bigint) AS now_ms",
        )
        .bind::<BigInt, _>(now)
        .get_result::<Clock>(db)
        .await?
        .now_ms)
    }
    async fn lock(db: &mut AsyncPgConnection) -> Result<()> {
        db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
            .await?;
        Ok(())
    }
    async fn authenticate(
        db: &mut AsyncPgConnection,
        p: &Principal,
        now: i64,
    ) -> Result<(i64, i64)> {
        Self::lock(db).await?;
        let auth=sql_query("SELECT s.valid_until_ms FROM hagency_agent_v1.sessions s JOIN hagency_agent_v1.users u ON u.id=s.user_id WHERE s.id=$1 AND u.id=$2 AND u.mxid=$3 AND u.subject=$4 AND s.client_id=$5 AND u.active AND NOT s.revoked FOR SHARE OF s,u")
            .bind::<Text,_>(&p.session_id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(&p.subject).bind::<Text,_>(&p.client_id).get_result::<Auth>(db).await.map_err(unauthorized)?;
        let (device, generation) = device(p)?;
        sql_query("SELECT true AS matched FROM hagency_agent_v1.devices WHERE id=$1 AND user_id=$2 AND session_id=$3 AND generation=$4 AND NOT revoked FOR SHARE")
            .bind::<Text,_>(device).bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.session_id).bind::<BigInt,_>(generation).get_result::<Flag>(db).await.map_err(unauthorized)?;
        let now = Self::clock(db, now).await?;
        if auth.valid_until_ms <= now {
            return Err(Error::Unauthorized("authorization_expired"));
        }
        Ok((now, auth.valid_until_ms))
    }
    async fn scope(db: &mut AsyncPgConnection, binding: &str) -> Result<RoutingScope> {
        sql_query("SELECT b.id AS binding_id,a.owner_user_id,u.mxid AS owner_mxid,a.puppet_mxid,a.id AS agent_id,b.room_id,coalesce(p.space_id,'') AS space_id,b.generation AS binding_generation,b.thread_auto_reply,(u.active AND a.state='active' AND b.state='active' AND NOT b.admin_project_paused AND NOT b.admin_room_paused AND ((b.scope_kind='project' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))) AS active,(u.active AND a.state='active' AND b.state='suspended' AND b.owner_service_paused AND NOT b.admin_project_paused AND NOT b.admin_room_paused AND ((b.scope_kind='project' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))) AS service_paused FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id WHERE b.id=$1")
            .bind::<Text,_>(binding).get_result(db).await.map_err(unauthorized)
    }
    /// Trusted gateway lookup, never directly exposed as a user HTTP endpoint.
    pub async fn binding_scope(&self, binding_id: &str) -> Result<RoutingScope> {
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::lock(db).await?;
            let scope = Self::scope(db, binding_id).await?;
            if !scope.active {
                return Err(Error::Unauthorized("binding_not_running"));
            }
            Ok(scope)
        })
        .await
    }
    /// Trusted AS routing discovery. Actual Matrix memberships still require live facts.
    pub async fn routing_scopes(&self, room_id: &str) -> Result<Vec<RoutingScope>> {
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;
            let rows=sql_query("SELECT b.id AS binding_id,a.owner_user_id,u.mxid AS owner_mxid,a.puppet_mxid,a.id AS agent_id,b.room_id,coalesce(p.space_id,'') AS space_id,b.generation AS binding_generation,b.thread_auto_reply,(u.active AND a.state='active' AND b.state='active' AND NOT b.admin_project_paused AND NOT b.admin_room_paused AND ((b.scope_kind='project' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))) AS active,(u.active AND a.state='active' AND b.state='suspended' AND b.owner_service_paused AND NOT b.admin_project_paused AND NOT b.admin_room_paused AND ((b.scope_kind='project' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))) AS service_paused FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id WHERE b.room_id=$1")
                .bind::<Text,_>(room_id).load::<RoutingScope>(db).await?;
            Ok(rows.into_iter().filter(|scope|scope.active||scope.service_paused).collect())
        }).await
    }
    /// Device-authenticated lookup of canonical requester/Room metadata. It never
    /// exposes body before gateway membership checks, and cannot cross owner scope.
    pub async fn owned_event_header(
        &self,
        p: &Principal,
        id: &str,
        now: i64,
    ) -> Result<EventHeader> {
        key(id)?;
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authenticate(db,p,now).await?;
            sql_query("SELECT id,binding_id,event_id,room_id,requester_mxid,thread_root FROM hagency_agent_v1.owner_events WHERE id=$1 AND owner_user_id=$2").bind::<Text,_>(id).bind::<Text,_>(&p.user_id).get_result(db).await.map_err(unauthorized)
        }).await
    }
    async fn agent_active(db: &mut AsyncPgConnection, p: &Principal, agent: &str) -> Result<()> {
        crate::domain::require_assigned(db, p, agent).await?;
        sql_query("SELECT true AS matched FROM hagency_agent_v1.agents WHERE id=$1 AND owner_user_id=$2 AND state='active'").bind::<Text,_>(agent).bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await.map_err(unauthorized)?;
        Ok(())
    }
    async fn invalidate_epoch(db: &mut AsyncPgConnection, agent: &str) -> Result<()> {
        // Re-executing an in-flight tool after failover is unsafe. Retain unknown
        // state for explicit operator reconciliation; ACK alone is not execution.
        sql_query("UPDATE hagency_agent_v1.owner_events SET state=CASE WHEN state='running' THEN 'unknown' ELSE 'pending' END WHERE agent_id=$1 AND state IN ('offered','acknowledged','running')").bind::<Text,_>(agent).execute(db).await?;
        sql_query("UPDATE hagency_agent_v1.reply_outbox SET state='cancelled' WHERE agent_id=$1 AND state='pending'").bind::<Text,_>(agent).execute(db).await?;
        Ok(())
    }
    /// `takeover` must be an explicit same-owner user action, never automatic
    /// failover that pretends old disconnected local tools can be forcibly stopped.
    pub async fn acquire_lease(
        &self,
        p: &Principal,
        agent: &str,
        ttl_ms: i64,
        takeover: bool,
        history_snapshot: &ExecutionHistorySnapshot,
        now: i64,
    ) -> Result<Lease> {
        key(agent)?;
        history_snapshot.validate()?;
        if !(1000..=self.limits.max_lease_ms).contains(&ttl_ms) {
            return Err(Error::Invalid("invalid_lease_ttl"));
        }
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            let (now,_)=Self::authenticate(db,p,now).await?;Self::agent_active(db,p,agent).await?;
            if history::snapshot(db,p,agent).await?!=*history_snapshot {return Err(Error::Conflict("execution_history_changed"));}
            let (now,until)=Self::authenticate(db,p,now).await?;let (device,generation)=device(p)?;
            let previous=sql_query("SELECT agent_id,owner_user_id,device_id,device_generation,epoch,expires_at_ms FROM hagency_agent_v1.execution_leases WHERE agent_id=$1").bind::<Text,_>(agent).get_result::<Lease>(db).await.optional()?;
            let epoch=match previous {
                None=>1,
                Some(old) if old.device_id==device && old.device_generation==generation && old.expires_at_ms>now=>old.epoch,
                Some(old)=>{if old.expires_at_ms>now && !takeover {return Err(Error::Conflict("agent_leased_to_another_device"));}Self::invalidate_epoch(db,agent).await?;old.epoch.checked_add(1).ok_or(Error::Conflict("lease_epoch_exhausted"))?}
            };
            let expires=now.checked_add(ttl_ms).ok_or(Error::Invalid("invalid_lease_ttl"))?.min(until);
            Ok(sql_query("INSERT INTO hagency_agent_v1.execution_leases(agent_id,owner_user_id,device_id,device_generation,epoch,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(agent_id) DO UPDATE SET device_id=EXCLUDED.device_id,device_generation=EXCLUDED.device_generation,epoch=EXCLUDED.epoch,expires_at_ms=EXCLUDED.expires_at_ms RETURNING agent_id,owner_user_id,device_id,device_generation,epoch,expires_at_ms")
                .bind::<Text,_>(agent).bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<BigInt,_>(generation).bind::<BigInt,_>(epoch).bind::<BigInt,_>(expires).get_result(db).await?)
        }).await
    }
    async fn lease(
        db: &mut AsyncPgConnection,
        p: &Principal,
        reference: &LeaseRef,
        now: i64,
    ) -> Result<Lease> {
        key(&reference.agent_id)?;
        if reference.epoch <= 0 {
            return Err(Error::Invalid("invalid_lease_epoch"));
        }
        crate::domain::require_assigned(db, p, &reference.agent_id).await?;
        let (id, generation) = device(p)?;
        sql_query("SELECT agent_id,owner_user_id,device_id,device_generation,epoch,expires_at_ms FROM hagency_agent_v1.execution_leases WHERE agent_id=$1 AND owner_user_id=$2 AND device_id=$3 AND device_generation=$4 AND epoch=$5 AND expires_at_ms>greatest($6,(extract(epoch from clock_timestamp())*1000)::bigint)")
            .bind::<Text,_>(&reference.agent_id).bind::<Text,_>(&p.user_id).bind::<Text,_>(id).bind::<BigInt,_>(generation).bind::<BigInt,_>(reference.epoch).bind::<BigInt,_>(now).get_result(db).await.map_err(unauthorized)
    }
    pub async fn renew_lease(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        ttl_ms: i64,
        now: i64,
    ) -> Result<Lease> {
        if !(1000..=self.limits.max_lease_ms).contains(&ttl_ms) {
            return Err(Error::Invalid("invalid_lease_ttl"));
        }
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            let (now,until)=Self::authenticate(db,p,now).await?;Self::agent_active(db,p,&reference.agent_id).await?;Self::lease(db,p,reference,now).await?;
            Ok(sql_query("UPDATE hagency_agent_v1.execution_leases SET expires_at_ms=least($1,$2) WHERE agent_id=$3 RETURNING agent_id,owner_user_id,device_id,device_generation,epoch,expires_at_ms").bind::<BigInt,_>(now.saturating_add(ttl_ms)).bind::<BigInt,_>(until).bind::<Text,_>(&reference.agent_id).get_result(db).await?)
        }).await
    }
    pub async fn release_lease(&self, p: &Principal, reference: &LeaseRef, now: i64) -> Result<()> {
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            let (now, _) = Self::authenticate(db, p, now).await?;
            Self::lease(db, p, reference, now).await?;
            Self::invalidate_epoch(db, &reference.agent_id).await?;
            sql_query(
                "UPDATE hagency_agent_v1.execution_leases SET expires_at_ms=$1 WHERE agent_id=$2",
            )
            .bind::<BigInt, _>(now)
            .bind::<Text, _>(&reference.agent_id)
            .execute(db)
            .await?;
            Ok(())
        })
        .await
    }
    async fn observe_delivery_authority(
        &self,
        principal: Option<&Principal>,
        binding: &str,
        requester: &str,
        event_id: Option<&str>,
        facts: &DeliveryFacts,
        now: i64,
    ) -> Result<()> {
        let mut guard = self.db.lock().await;
        (*guard).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::lock(db).await?;
            let now = Self::clock(db, now).await?;
            let scope = Self::scope(db, binding).await?;
            if let Some(p) = principal {
                Self::authenticate(db, p, now).await?;
                if scope.owner_user_id != p.user_id { return Err(Error::Unauthorized("transport_scope_not_authorized")); }
            }
            observation(&scope, requester, facts, now)?;
            let room = crate::domain::RoomFacts {
                owner_direct_valid:facts.owner_direct_valid,
                owner_mxid: facts.owner_mxid.clone(), room_id: facts.room_id.clone(),
                space_id: facts.space_id.clone(), observed_at_ms: facts.observed_at_ms,
                room_in_space: facts.room_in_space, owner_in_space: facts.owner_in_space,
                owner_in_room: facts.owner_in_room, service_can_invite: false,
                puppet_mxid: Some(facts.puppet_mxid.clone()), puppet_in_room: facts.puppet_in_room,
                encrypted: facts.encrypted,
            };
            crate::domain::DomainStore::fence_membership_loss(db, binding, Some(scope.binding_generation), &room, false, now).await?;
            // Speaking-right loss is a binding-wide proven denial. Never erase
            // uncertain network evidence or let restored permissions replay it.
            let binding_denied = !facts.puppet_can_send_message;
            if binding_denied {
                sql_query("UPDATE hagency_agent_v1.owner_events SET state=CASE WHEN state IN ('running','unknown') THEN 'unknown' ELSE 'cancelled' END,outcome='puppet_send_permission_revoked' WHERE binding_id=$1 AND binding_generation=$2 AND ($3 OR id=$4) AND state IN ('pending','offered','acknowledged','running','unknown')")
                    .bind::<Text,_>(binding).bind::<BigInt,_>(scope.binding_generation).bind::<Bool,_>(binding_denied)
                    .bind::<Nullable<Text>,_>(event_id).execute(db).await?;
                sql_query("UPDATE hagency_agent_v1.reply_outbox SET delivery_blocked=true,state=CASE WHEN state='pending' THEN 'cancelled' ELSE state END WHERE binding_id=$1 AND binding_generation=$2 AND ($3 OR owner_event_id=$4) AND state!='sent'")
                    .bind::<Text,_>(binding).bind::<BigInt,_>(scope.binding_generation).bind::<Bool,_>(binding_denied)
                    .bind::<Nullable<Text>,_>(event_id).execute(db).await?;
            }
            Ok(())
        }).await
    }
    async fn observe_event_authority(
        &self,
        p: &Principal,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<()> {
        let event = {
            let mut guard = self.db.lock().await;
            Self::dispatch(&mut guard, p, id).await?
        };
        self.observe_delivery_authority(
            Some(p),
            &event.binding_id,
            &event.requester_mxid,
            Some(id),
            f,
            now,
        )
        .await
    }
    async fn event_unexpired(
        db: &mut AsyncPgConnection,
        id: &str,
        ttl: i64,
        now: i64,
    ) -> Result<()> {
        let current = Self::clock(db, now).await?;
        let age = sql_query(
            "SELECT created_at_ms>$2 AS matched FROM hagency_agent_v1.owner_events WHERE id=$1",
        )
        .bind::<Text, _>(id)
        .bind::<BigInt, _>(current.saturating_sub(ttl))
        .get_result::<Flag>(db)
        .await?;
        if !age.matched {
            return Err(Error::Conflict("event_expired"));
        }
        Ok(())
    }
    async fn expire_pending(db: &mut AsyncPgConnection, ttl: i64, now: i64) -> Result<usize> {
        Ok(sql_query("UPDATE hagency_agent_v1.owner_events SET state='cancelled',outcome='expired' WHERE state IN ('pending','offered','acknowledged') AND created_at_ms<=$1")
            .bind::<BigInt,_>(now.saturating_sub(ttl)).execute(db).await?)
    }
    /// Trusted AS routing worker: durable queue insert succeeds independently of
    /// client presence. A duplicate event never creates a second dispatch.
    pub async fn ingest_routed(
        &self,
        binding: &str,
        event: RoutedEvent,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<RouteResult> {
        self.ingest_routed_received(binding, event, f, now, now)
            .await
    }
    /// Original durable Appservice receive time, never a homeserver event clock.
    pub async fn ingest_routed_received(
        &self,
        binding: &str,
        event: RoutedEvent,
        f: &DeliveryFacts,
        received_at_ms: i64,
        now: i64,
    ) -> Result<RouteResult> {
        self.observe_delivery_authority(None, binding, &event.sender_mxid, None, f, now)
            .await?;
        key(binding)?;
        matrix_id(&event.event_id, '$')?;
        matrix_id(&event.room_id, '!')?;
        matrix_id(&event.sender_mxid, '@')?;
        message(&event.body, self.limits.max_message_bytes)?;
        if event.encrypted || f.encrypted {
            return Err(Error::Conflict("encrypted_room_requires_client_crypto"));
        }
        if event.is_edit {
            return Ok(RouteResult::Ignored);
        }
        let digest =
            hash(&serde_json::to_string(&event).map_err(|_| Error::Invalid("invalid_event"))?);
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let now=Self::clock(db,now).await?;let scope=Self::scope(db,binding).await?;
            if received_at_ms < 0 || received_at_ms > now { return Err(Error::Invalid("invalid_event_received_time")); }
            if received_at_ms <= now.saturating_sub(self.limits.event_ttl_ms) { return Err(Error::Conflict("event_expired")); }
            Self::expire_pending(db,self.limits.event_ttl_ms,now).await?;
            pause_notices::routing_facts(&scope,&event.sender_mxid,f,now)?;if event.room_id!=scope.room_id {return Err(Error::Unauthorized("event_room_mismatch"));}
            let existing=sql_query("SELECT id,digest FROM hagency_agent_v1.owner_events WHERE binding_id=$1 AND event_id=$2").bind::<Text,_>(binding).bind::<Text,_>(&event.event_id).get_result::<Existing>(db).await.optional()?;
            if let Some(old)=existing {if old.digest!=digest {return Err(Error::Conflict("event_payload_changed"));}return Ok(RouteResult::Duplicate{dispatch_id:old.id});}
            if let Some(duplicate)=Self::existing_pause_notice(db,binding,&event.event_id,&digest).await? {return Ok(duplicate);}
            let puppet_sender=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE puppet_mxid=$1) AS matched").bind::<Text,_>(&event.sender_mxid).get_result::<Flag>(db).await?;
            if puppet_sender.matched || event.sender_mxid.starts_with("@_hagency_") {return Ok(RouteResult::Ignored);}
            let known=match &event.thread_root {Some(root)=>sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.agent_threads WHERE binding_id=$1 AND thread_root=$2) AS matched").bind::<Text,_>(binding).bind::<Text,_>(root).get_result::<Flag>(db).await?.matched,None=>false};
            if !scope.space_id.is_empty()&&!event.mentioned_mxids.contains(&scope.puppet_mxid)&&!(scope.thread_auto_reply && known) {return Ok(RouteResult::Ignored);}
            if scope.service_paused {
                return self.queue_pause_notice(db,&scope,&event,&digest,received_at_ms,now).await;
            }
            let count=sql_query("SELECT count(*) AS total FROM hagency_agent_v1.owner_events WHERE owner_user_id=$1 AND state IN ('pending','offered','acknowledged','running','unknown')").bind::<Text,_>(&scope.owner_user_id).get_result::<Count>(db).await?;
            if count.total>=self.limits.max_owner_events {return Err(Error::Unavailable("owner_queue_full"));}
            // A top-level owner DM uses its immutable Room ID as the context key.
            // Explicit Matrix threads and Project messages retain event roots.
            // Existing queued/outbox roots are never reinterpreted on replay.
            let root=match event.thread_root.as_deref() {
                Some(root)=>{matrix_id(root,'$')?;root},
                None if scope.space_id.is_empty()=>&event.room_id,
                None=>&event.event_id,
            };
            let id=format!("evt_{}",entity_id()?);
            sql_query("INSERT INTO hagency_agent_v1.owner_events(id,binding_id,agent_id,owner_user_id,event_id,room_id,requester_mxid,thread_root,body,digest,binding_generation,state,created_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'pending',$12)")
                .bind::<Text,_>(&id).bind::<Text,_>(binding).bind::<Text,_>(&scope.agent_id).bind::<Text,_>(&scope.owner_user_id).bind::<Text,_>(&event.event_id).bind::<Text,_>(&event.room_id).bind::<Text,_>(&event.sender_mxid).bind::<Text,_>(root).bind::<Text,_>(&event.body).bind::<Text,_>(digest).bind::<BigInt,_>(scope.binding_generation).bind::<BigInt,_>(received_at_ms).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.agent_threads(binding_id,thread_root) VALUES($1,$2) ON CONFLICT DO NOTHING").bind::<Text,_>(binding).bind::<Text,_>(root).execute(db).await?;
            Ok(RouteResult::Queued{dispatch_id:id})
        }).await
    }
    /// Host gateway discovery: these headers are not an authorization to expose
    /// message bodies. Fetch current membership facts, then call claim_event.
    pub async fn event_candidates(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        limit: usize,
        now: i64,
    ) -> Result<Vec<EventHeader>> {
        self.event_candidates_scoped(p, reference, None, limit, now)
            .await
    }
    pub async fn event_candidates_for_binding(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        binding: &str,
        limit: usize,
        now: i64,
    ) -> Result<Vec<EventHeader>> {
        key(binding)?;
        self.event_candidates_scoped(p, reference, Some(binding), limit, now)
            .await
    }
    async fn event_candidates_scoped(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        binding: Option<&str>,
        limit: usize,
        now: i64,
    ) -> Result<Vec<EventHeader>> {
        if limit == 0 || limit > self.limits.max_batch {
            return Err(Error::Invalid("invalid_batch_limit"));
        }
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            let (now,_)=Self::authenticate(db,p,now).await?;Self::lease(db,p,reference,now).await?;Self::agent_active(db,p,&reference.agent_id).await?;
            if let Some(binding) = binding {
                let scope = Self::scope(db,binding).await?;
                if scope.owner_user_id!=p.user_id || scope.agent_id!=reference.agent_id || !scope.active {
                    return Err(Error::Unauthorized("dispatch_scope_revoked"));
                }
            }
            Self::expire_pending(db,self.limits.event_ttl_ms,now).await?;
            Ok(sql_query("SELECT e.id,e.binding_id,e.event_id,e.room_id,e.requester_mxid,e.thread_root FROM hagency_agent_v1.owner_events e JOIN hagency_agent_v1.bindings b ON b.id=e.binding_id WHERE e.owner_user_id=$1 AND e.agent_id=$2 AND e.state IN ('pending','offered','acknowledged') AND e.binding_generation=b.generation AND b.state='active' AND e.created_at_ms>$4 AND ($5::text IS NULL OR e.binding_id=$5) ORDER BY e.created_at_ms,e.id LIMIT $3").bind::<Text,_>(&p.user_id).bind::<Text,_>(&reference.agent_id).bind::<BigInt,_>(limit as i64).bind::<BigInt,_>(now.saturating_sub(self.limits.event_ttl_ms)).bind::<Nullable<Text>,_>(binding).load(db).await?)
        }).await
    }
    async fn dispatch(db: &mut AsyncPgConnection, p: &Principal, id: &str) -> Result<Dispatch> {
        sql_query("SELECT id,binding_id,agent_id,event_id,room_id,requester_mxid,thread_root,body,state,binding_generation,dispatch_epoch,dispatch_device_id,execution_id,outcome FROM hagency_agent_v1.owner_events WHERE id=$1 AND owner_user_id=$2")
            .bind::<Text,_>(id).bind::<Text,_>(&p.user_id).get_result(db).await.map_err(unauthorized)
    }
    async fn dispatch_scope(
        db: &mut AsyncPgConnection,
        p: &Principal,
        reference: &LeaseRef,
        event: &Dispatch,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<RoutingScope> {
        Self::lease(db, p, reference, now).await?;
        let scope = Self::scope(db, &event.binding_id).await?;
        if scope.owner_user_id != p.user_id
            || event.agent_id != reference.agent_id
            || event.binding_generation != scope.binding_generation
        {
            return Err(Error::Unauthorized("dispatch_scope_revoked"));
        }
        facts(&scope, &event.requester_mxid, f, now)?;
        Ok(scope)
    }
    pub async fn claim_event(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<Dispatch> {
        self.observe_event_authority(p, id, f, now).await?;
        key(id)?;
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            let (now,_)=Self::authenticate(db,p,now).await?;let event=Self::dispatch(db,p,id).await?;Self::dispatch_scope(db,p,reference,&event,f,now).await?;
            Self::event_unexpired(db,id,self.limits.event_ttl_ms,now).await?;
            if event.state=="pending" {sql_query("UPDATE hagency_agent_v1.owner_events SET state='offered',dispatch_epoch=$1,dispatch_device_id=$2,execution_id=NULL WHERE id=$3").bind::<BigInt,_>(reference.epoch).bind::<Text,_>(device(p)?.0).bind::<Text,_>(id).execute(db).await?;}
            else if !matches!(event.state.as_str(),"offered"|"acknowledged")||event.dispatch_epoch!=Some(reference.epoch)||event.dispatch_device_id.as_deref()!=Some(device(p)?.0) {return Err(Error::Conflict("dispatch_not_claimable"));}
            Self::dispatch(db,p,id).await
        }).await
    }
    /// Called only after client has durably committed this event into its inbox.
    pub async fn acknowledge(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        id: &str,
        now: i64,
    ) -> Result<()> {
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            let (now, _) = Self::authenticate(db, p, now).await?;
            Self::lease(db, p, reference, now).await?;
            Self::event_unexpired(db, id, self.limits.event_ttl_ms, now).await?;
            let e = Self::dispatch(db, p, id).await?;
            fenced(&e, p, reference)?;
            let scope = Self::scope(db, &e.binding_id).await?;
            if !scope.active || scope.binding_generation != e.binding_generation {
                return Err(Error::Unauthorized("dispatch_scope_revoked"));
            }
            if e.state == "acknowledged" {
                return Ok(());
            }
            if e.state != "offered" {
                return Err(Error::Conflict("dispatch_not_offered"));
            }
            sql_query("UPDATE hagency_agent_v1.owner_events SET state='acknowledged' WHERE id=$1")
                .bind::<Text, _>(id)
                .execute(db)
                .await?;
            Ok(())
        })
        .await
    }
    /// Persist the execution identity before any model/tool call. Repeated same
    /// execution ID resumes an existing attempt; it never authorizes another run.
    pub async fn start_execution(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        id: &str,
        execution_id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<ExecutionStart> {
        self.observe_event_authority(p, id, f, now).await?;
        key(execution_id)?;
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            let (now,_)=Self::authenticate(db,p,now).await?;let e=Self::dispatch(db,p,id).await?;Self::dispatch_scope(db,p,reference,&e,f,now).await?;fenced(&e,p,reference)?;
            if e.state=="running"&&e.execution_id.as_deref()==Some(execution_id) {return Ok(ExecutionStart{dispatch:e,newly_started:false});}
            Self::event_unexpired(db,id,self.limits.event_ttl_ms,now).await?;
            if e.state!="acknowledged" {return Err(Error::Conflict("dispatch_not_acknowledged"));}
            let reused=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.owner_events WHERE agent_id=$1 AND execution_id=$2) AS matched").bind::<Text,_>(&e.agent_id).bind::<Text,_>(execution_id).get_result::<Flag>(db).await?;if reused.matched {return Err(Error::Conflict("execution_id_reused"));}
            sql_query("UPDATE hagency_agent_v1.owner_events SET state='running',execution_id=$1 WHERE id=$2").bind::<Text,_>(execution_id).bind::<Text,_>(id).execute(db).await?;Ok(ExecutionStart{dispatch:Self::dispatch(db,p,id).await?,newly_started:true})
        }).await
    }
    /// Fresh execution authority check immediately before a host tool. It grants
    /// no start/replay and carries no model, filesystem or local approval policy.
    pub async fn authorize_tool_execution(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        id: &str,
        execution_id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<Dispatch> {
        self.observe_event_authority(p, id, f, now).await?;
        key(execution_id)?;
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            let (now, _) = Self::authenticate(db, p, now).await?;
            let event = Self::dispatch(db, p, id).await?;
            Self::dispatch_scope(db, p, reference, &event, f, now).await?;
            fenced(&event, p, reference)?;
            if event.state != "running" || event.execution_id.as_deref() != Some(execution_id) {
                return Err(Error::Conflict("execution_not_running"));
            }
            Self::event_unexpired(db, id, self.limits.event_ttl_ms, now).await?;
            let current = Self::clock(db, now).await?;
            Self::lease(db, p, reference, current).await?;
            Ok(event)
        })
        .await
    }
    /// Completes a rejected/failed run without a reply. `unknown` is conservative
    /// when model usage or tool side effects cannot be established.
    pub async fn finish_without_reply(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        id: &str,
        execution_id: &str,
        outcome: &str,
        now: i64,
    ) -> Result<()> {
        if !matches!(outcome, "rejected" | "failed" | "unknown") {
            return Err(Error::Invalid("invalid_execution_outcome"));
        }
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            let (now, _) = Self::authenticate(db, p, now).await?;
            Self::lease(db, p, reference, now).await?;
            let e = Self::dispatch(db, p, id).await?;
            fenced(&e, p, reference)?;
            let scope = Self::scope(db, &e.binding_id).await?;
            if !scope.active || scope.binding_generation != e.binding_generation {
                return Err(Error::Unauthorized("dispatch_scope_revoked"));
            }
            if matches!(e.state.as_str(), "completed" | "unknown")
                && e.execution_id.as_deref() == Some(execution_id)
                && e.outcome.as_deref() == Some(outcome)
            {
                return Ok(());
            }
            if e.state != "running" || e.execution_id.as_deref() != Some(execution_id) {
                return Err(Error::Conflict("execution_not_running"));
            }
            sql_query("UPDATE hagency_agent_v1.owner_events SET state=$1,outcome=$2 WHERE id=$3")
                .bind::<Text, _>(if outcome == "unknown" {
                    "unknown"
                } else {
                    "completed"
                })
                .bind::<Text, _>(outcome)
                .bind::<Text, _>(id)
                .execute(db)
                .await?;
            Ok(())
        })
        .await
    }
    pub async fn submit_reply(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        request: SubmitReply,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<ReplyIntent> {
        self.observe_event_authority(p, &request.dispatch_id, f, now)
            .await?;
        key(&request.dispatch_id)?;
        key(&request.execution_id)?;
        message(&request.body, self.limits.max_message_bytes)?;
        let digest =
            hash(&serde_json::to_string(&request).map_err(|_| Error::Invalid("invalid_reply"))?);
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            let (now,_)=Self::authenticate(db,p,now).await?;let e=Self::dispatch(db,p,&request.dispatch_id).await?;let scope=Self::dispatch_scope(db,p,reference,&e,f,now).await?;fenced(&e,p,reference)?;
            let old=Self::reply_for_event(db,&e.id).await?;
            if let Some(old)=old {if old.payload_digest!=digest {return Err(Error::Conflict("reply_payload_changed"));}return Ok(old);}
            if e.state!="running"||e.execution_id.as_deref()!=Some(&request.execution_id) {return Err(Error::Conflict("execution_not_running"));}
            let id=format!("rep_{}",entity_id()?);let txn=format!("hagency_{}",hash(&e.id));
            sql_query("INSERT INTO hagency_agent_v1.reply_outbox(id,owner_event_id,agent_id,binding_id,owner_user_id,room_id,puppet_mxid,thread_root,body,payload_digest,matrix_txn_id,binding_generation,dispatch_epoch,delivery_epoch,state,created_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$13,'pending',$14)")
                .bind::<Text,_>(&id).bind::<Text,_>(&e.id).bind::<Text,_>(&e.agent_id).bind::<Text,_>(&e.binding_id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&e.room_id).bind::<Text,_>(&scope.puppet_mxid).bind::<Text,_>(&e.thread_root).bind::<Text,_>(&request.body).bind::<Text,_>(&digest).bind::<Text,_>(&txn).bind::<BigInt,_>(e.binding_generation).bind::<BigInt,_>(reference.epoch).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.owner_events SET state='completed',outcome='replied' WHERE id=$1").bind::<Text,_>(&e.id).execute(db).await?;
            Self::reply(db,&id).await
        }).await
    }
    /// Explicit recovery of a locally durable known result under current authority.
    /// This never reruns execution, changes its original epoch or resolves unknown
    /// model usage/tool side effects. It does not unblock a denied Matrix delivery.
    pub async fn reconcile_known_reply(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        request: SubmitReply,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<ReplyIntent> {
        self.observe_event_authority(p, &request.dispatch_id, f, now)
            .await?;
        key(&request.dispatch_id)?;
        key(&request.execution_id)?;
        message(&request.body, self.limits.max_message_bytes)?;
        let digest =
            hash(&serde_json::to_string(&request).map_err(|_| Error::Invalid("invalid_reply"))?);
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            let (now,_)=Self::authenticate(db,p,now).await?;
            let event=Self::dispatch(db,p,&request.dispatch_id).await?;if event.outcome.as_deref()==Some("puppet_send_permission_revoked") {return Err(Error::Unauthorized("reply_delivery_permanently_blocked"));}
            let scope=Self::dispatch_scope(db,p,reference,&event,f,now).await?;
            if event.execution_id.as_deref()!=Some(&request.execution_id)||event.dispatch_epoch.is_none() {
                return Err(Error::Conflict("original_execution_mismatch"));
            }
            if let Some(old)=Self::reply_for_event(db,&event.id).await? {
                if old.payload_digest!=digest {return Err(Error::Conflict("reply_payload_changed"));}
                if old.state=="sent" {return Ok(old);}
                if old.delivery_blocked {return Err(Error::Unauthorized("reply_delivery_permanently_blocked"));}
                if old.worker_until_ms>now {return Err(Error::Conflict("reply_sender_busy"));}
                if old.state=="cancelled" && (old.worker_token.is_some()||reference.epoch<=old.delivery_epoch) {
                    return Err(Error::Conflict("reply_not_reconcilable"));
                }
                // An ambiguous historical send retains the same txn and unknown state.
                // Only a definitely unsent cancelled intent may return to pending.
                sql_query("UPDATE hagency_agent_v1.reply_outbox SET delivery_epoch=$1,state=CASE WHEN state='cancelled' THEN 'pending' WHEN state='sending' THEN 'unknown' ELSE state END WHERE id=$2")
                    .bind::<BigInt,_>(reference.epoch).bind::<Text,_>(&old.id).execute(db).await?;
                sql_query("INSERT INTO hagency_agent_v1.domain_audit(actor_user_id,operation,object_id,at_ms) VALUES($1,'device.reply.known_reconciled',$2,$3)")
                    .bind::<Text,_>(&p.user_id).bind::<Text,_>(&old.id).bind::<BigInt,_>(now).execute(db).await?;
                let checked_now=Self::clock(db,now).await?;Self::lease(db,p,reference,checked_now).await?;
                return Self::reply(db,&old.id).await;
            }
            if event.state!="unknown" {return Err(Error::Conflict("execution_not_reconcilable"));}
            let id=format!("rep_{}",entity_id()?);let txn=format!("hagency_{}",hash(&event.id));
            sql_query("INSERT INTO hagency_agent_v1.reply_outbox(id,owner_event_id,agent_id,binding_id,owner_user_id,room_id,puppet_mxid,thread_root,body,payload_digest,matrix_txn_id,binding_generation,dispatch_epoch,delivery_epoch,state,created_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,'pending',$15)")
                .bind::<Text,_>(&id).bind::<Text,_>(&event.id).bind::<Text,_>(&event.agent_id).bind::<Text,_>(&event.binding_id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&event.room_id).bind::<Text,_>(&scope.puppet_mxid).bind::<Text,_>(&event.thread_root).bind::<Text,_>(&request.body).bind::<Text,_>(&digest).bind::<Text,_>(&txn).bind::<BigInt,_>(event.binding_generation).bind::<BigInt,_>(event.dispatch_epoch.unwrap()).bind::<BigInt,_>(reference.epoch).bind::<BigInt,_>(now).execute(db).await?;
            // Keep the execution unknown. Known text is not evidence resolving tool
            // effects or model accounting, and cannot start/settle that execution.
            sql_query("UPDATE hagency_agent_v1.owner_events SET outcome='known_reply_reconciled' WHERE id=$1")
                .bind::<Text,_>(&event.id).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.domain_audit(actor_user_id,operation,object_id,at_ms) VALUES($1,'device.reply.known_reconciled',$2,$3)")
                .bind::<Text,_>(&p.user_id).bind::<Text,_>(&id).bind::<BigInt,_>(now).execute(db).await?;
            let checked_now=Self::clock(db,now).await?;Self::lease(db,p,reference,checked_now).await?;
            Self::reply(db,&id).await
        }).await
    }
    async fn reply(db: &mut AsyncPgConnection, id: &str) -> Result<ReplyIntent> {
        sql_query("SELECT id,owner_event_id,agent_id,binding_id,owner_user_id,room_id,puppet_mxid,thread_root,body,payload_digest,matrix_txn_id,binding_generation,dispatch_epoch,delivery_epoch,state,delivery_blocked,worker_token,worker_until_ms,matrix_event_id,(SELECT requester_mxid FROM hagency_agent_v1.owner_events WHERE id=owner_event_id) AS requester_mxid FROM hagency_agent_v1.reply_outbox WHERE id=$1").bind::<Text,_>(id).get_result(db).await.map_err(unauthorized)
    }
    async fn reply_for_event(
        db: &mut AsyncPgConnection,
        event: &str,
    ) -> Result<Option<ReplyIntent>> {
        Ok(sql_query("SELECT id,owner_event_id,agent_id,binding_id,owner_user_id,room_id,puppet_mxid,thread_root,body,payload_digest,matrix_txn_id,binding_generation,dispatch_epoch,delivery_epoch,state,delivery_blocked,worker_token,worker_until_ms,matrix_event_id,(SELECT requester_mxid FROM hagency_agent_v1.owner_events WHERE id=owner_event_id) AS requester_mxid FROM hagency_agent_v1.reply_outbox WHERE owner_event_id=$1").bind::<Text,_>(event).get_result(db).await.optional()?)
    }
    /// Trusted gateway only. A proven denial permanently removes this dispatch
    /// from the delivery front; it does not revive executions or release quotas.
    pub async fn reject_event_delivery(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<bool> {
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let now=Self::clock(db,now).await?;
            let e=sql_query("SELECT id,binding_id,agent_id,event_id,room_id,requester_mxid,thread_root,body,state,binding_generation,dispatch_epoch,dispatch_device_id,execution_id,outcome FROM hagency_agent_v1.owner_events WHERE id=$1").bind::<Text,_>(id).get_result::<Dispatch>(db).await.map_err(unauthorized)?;
            let scope=Self::scope(db,&e.binding_id).await?;observation(&scope,&e.requester_mxid,f,now)?;
            let denied=scope.binding_generation!=e.binding_generation||facts(&scope,&e.requester_mxid,f,now).is_err();
            if !denied||!matches!(e.state.as_str(),"pending"|"offered"|"acknowledged"|"running") {return Ok(false);}
            sql_query("UPDATE hagency_agent_v1.owner_events SET state=CASE WHEN state='running' THEN 'unknown' ELSE 'cancelled' END WHERE id=$1").bind::<Text,_>(id).execute(db).await?;Ok(true)
        }).await
    }
    /// Trusted sender only. Preserve unknown/sending network evidence, fixed txn,
    /// and payload, but permanently prohibit a new send after proven authority loss.
    pub async fn block_reply_delivery(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<bool> {
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let now=Self::clock(db,now).await?;let intent=Self::reply(db,id).await?;
            let scope=Self::scope(db,&intent.binding_id).await?;observation(&scope,&intent.requester_mxid,f,now)?;
            let denied=scope.binding_generation!=intent.binding_generation||facts(&scope,&intent.requester_mxid,f,now).is_err();
            if !denied||intent.state=="sent"||intent.delivery_blocked {return Ok(false);}
            sql_query("UPDATE hagency_agent_v1.reply_outbox SET delivery_blocked=true,state=CASE WHEN state='pending' THEN 'cancelled' ELSE state END WHERE id=$1").bind::<Text,_>(id).execute(db).await?;Ok(true)
        }).await
    }
    /// Trusted maintenance worker. Invalid scope cancels queued work; an already
    /// started attempt becomes unknown. Lease expiry/revocation requeues only work
    /// that has not started. Matrix membership changes are checked by gateway facts
    /// at every sensitive operation, independently of this database maintenance.
    pub async fn invalidate_stale_dispatches(&self, now: i64) -> Result<usize> {
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let now=Self::clock(db,now).await?;
            let ttl_expired=Self::expire_pending(db,self.limits.event_ttl_ms,now).await?;
            let count=sql_query("UPDATE hagency_agent_v1.owner_events e SET state=CASE WHEN e.state='running' THEN 'unknown' ELSE 'cancelled' END WHERE e.state IN ('pending','offered','acknowledged','running') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id WHERE b.id=e.binding_id AND b.generation=e.binding_generation AND b.state='active' AND NOT b.admin_project_paused AND NOT b.admin_room_paused AND a.state='active' AND u.active AND ((b.scope_kind='project' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id)))").execute(db).await?;
            let expired=sql_query("UPDATE hagency_agent_v1.owner_events e SET state=CASE WHEN e.state='running' THEN 'unknown' ELSE 'pending' END WHERE e.state IN ('offered','acknowledged','running') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.execution_leases l JOIN hagency_agent_v1.devices d ON d.id=l.device_id JOIN hagency_agent_v1.sessions s ON s.id=d.session_id WHERE l.agent_id=e.agent_id AND l.epoch=e.dispatch_epoch AND l.device_id=e.dispatch_device_id AND l.device_generation=d.generation AND d.user_id=e.owner_user_id AND s.user_id=e.owner_user_id AND NOT d.revoked AND NOT s.revoked AND s.valid_until_ms>$1 AND l.expires_at_ms>$1)").bind::<BigInt,_>(now).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.reply_outbox o SET state='cancelled' WHERE o.state='pending' AND (NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id WHERE b.id=o.binding_id AND b.generation=o.binding_generation AND b.state='active' AND NOT b.admin_project_paused AND NOT b.admin_room_paused AND a.state='active' AND u.active AND ((b.scope_kind='project' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))) OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.execution_leases l JOIN hagency_agent_v1.agents a ON a.id=l.agent_id JOIN hagency_agent_v1.devices d ON d.id=l.device_id JOIN hagency_agent_v1.sessions s ON s.id=d.session_id WHERE l.agent_id=o.agent_id AND l.epoch=o.delivery_epoch AND a.execution_device_id=l.device_id AND l.device_generation=d.generation AND d.user_id=o.owner_user_id AND s.user_id=o.owner_user_id AND NOT d.revoked AND NOT s.revoked AND s.valid_until_ms>$1 AND l.expires_at_ms>$1))").bind::<BigInt,_>(now).execute(db).await?;
            Ok(count+expired+ttl_expired)
        }).await
    }
    /// Trusted sender worker only: retrieve metadata then fetch live gateway facts.
    /// Do not expose this endpoint to user/device credentials.
    pub async fn reply_candidates(&self, limit: usize, now: i64) -> Result<Vec<ReplyIntent>> {
        self.reply_candidates_after("", limit, now).await
    }
    /// Trusted worker circular scan. The cursor advances even on Matrix failure;
    /// original intents, payloads and transaction identifiers never change.
    pub(crate) async fn reply_candidates_after(
        &self,
        cursor: &str,
        limit: usize,
        now: i64,
    ) -> Result<Vec<ReplyIntent>> {
        if limit == 0 || limit > self.limits.max_batch {
            return Err(Error::Invalid("invalid_batch_limit"));
        }
        let mut db = self.db.lock().await;
        Ok(sql_query("SELECT id,owner_event_id,agent_id,binding_id,owner_user_id,room_id,puppet_mxid,thread_root,body,payload_digest,matrix_txn_id,binding_generation,dispatch_epoch,delivery_epoch,state,delivery_blocked,worker_token,worker_until_ms,matrix_event_id,(SELECT requester_mxid FROM hagency_agent_v1.owner_events WHERE id=owner_event_id) AS requester_mxid FROM hagency_agent_v1.reply_outbox o WHERE state IN ('pending','unknown','sending') AND NOT delivery_blocked AND EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id JOIN hagency_agent_v1.execution_leases l ON l.agent_id=a.id JOIN hagency_agent_v1.devices d ON d.id=l.device_id JOIN hagency_agent_v1.sessions s ON s.id=d.session_id WHERE b.id=o.binding_id AND b.generation=o.binding_generation AND b.state='active' AND a.state='active' AND u.active AND ((b.scope_kind='project' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id)) AND NOT b.admin_project_paused AND NOT b.admin_room_paused AND l.epoch=o.delivery_epoch AND a.execution_device_id=l.device_id AND l.device_generation=d.generation AND NOT d.revoked AND NOT s.revoked AND d.user_id=o.owner_user_id AND s.user_id=o.owner_user_id AND l.expires_at_ms>greatest($1,(extract(epoch from clock_timestamp())*1000)::bigint) AND s.valid_until_ms>greatest($1,(extract(epoch from clock_timestamp())*1000)::bigint)) AND worker_until_ms<=greatest($1,(extract(epoch from clock_timestamp())*1000)::bigint) ORDER BY (id<=$3),id LIMIT $2")
            .bind::<BigInt,_>(now).bind::<BigInt,_>(limit as i64).bind::<Text,_>(cursor).load(&mut *db).await?)
    }
    /// Trusted routing worker scan over the existing durable AS inbox. No body
    /// is exposed through a public DTO, and no retry is acknowledged as routed.
    pub(crate) async fn routing_candidates_after(
        &self,
        cursor: &str,
        limit: usize,
    ) -> Result<Vec<crate::appservice::PendingTransaction>> {
        if limit == 0 || limit > 100 {
            return Err(Error::Invalid("invalid_batch_limit"));
        }
        let mut db = self.db.lock().await;
        Ok(sql_query("SELECT t.id,t.body,t.received_at_ms FROM hagency_agent_v1.inbound_transactions t JOIN hagency_agent_v1.routing_jobs j ON j.transaction_id=t.id WHERE j.state='pending' ORDER BY (t.id<=$1),t.id LIMIT $2").bind::<Text,_>(cursor).bind::<BigInt,_>(limit as i64).load(&mut *db).await?)
    }
    /// Durable send intent before network I/O. Retrying unknown/sending uses the
    /// original sender, room, payload and Matrix transaction ID unchanged.
    pub async fn claim_reply(&self, id: &str, f: &DeliveryFacts, now: i64) -> Result<ReplyIntent> {
        let candidate = {
            let mut guard = self.db.lock().await;
            Self::reply(&mut guard, id).await?
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
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let now=Self::clock(db,now).await?;let intent=Self::reply(db,id).await?;
            if intent.delivery_blocked||intent.state=="sent"||intent.state=="cancelled" {return Err(Error::Conflict("reply_not_sendable"));}
            if intent.worker_until_ms>now {return Err(Error::Conflict("reply_sender_busy"));}
            let scope=Self::scope(db,&intent.binding_id).await?;
            let requester=sql_query("SELECT requester_mxid AS id,digest FROM hagency_agent_v1.owner_events WHERE id=$1").bind::<Text,_>(&intent.owner_event_id).get_result::<Existing>(db).await?;
            facts(&scope,&requester.id,f,now)?;
            if scope.binding_generation!=intent.binding_generation||scope.owner_user_id!=intent.owner_user_id||scope.agent_id!=intent.agent_id||scope.puppet_mxid!=intent.puppet_mxid {return Err(Error::Unauthorized("reply_scope_revoked"));}
            // Revalidate the underlying device/session when actually sending.
            let lease=sql_query("SELECT true AS matched FROM hagency_agent_v1.execution_leases l JOIN hagency_agent_v1.devices d ON d.id=l.device_id JOIN hagency_agent_v1.sessions s ON s.id=d.session_id JOIN hagency_agent_v1.users u ON u.id=d.user_id JOIN hagency_agent_v1.agents a ON a.id=l.agent_id AND a.execution_device_id=l.device_id WHERE l.agent_id=$1 AND l.owner_user_id=$2 AND l.epoch=$3 AND l.device_generation=d.generation AND NOT d.revoked AND NOT s.revoked AND s.user_id=l.owner_user_id AND d.user_id=l.owner_user_id AND u.active AND s.valid_until_ms>greatest($4,(extract(epoch from clock_timestamp())*1000)::bigint) AND l.expires_at_ms>greatest($4,(extract(epoch from clock_timestamp())*1000)::bigint) FOR SHARE OF d,s,u")
                .bind::<Text,_>(&intent.agent_id).bind::<Text,_>(&intent.owner_user_id).bind::<BigInt,_>(intent.delivery_epoch).bind::<BigInt,_>(now).get_result::<Flag>(db).await;
            match lease {Ok(_)=>(),Err(diesel::result::Error::NotFound)=>return Err(Error::Unauthorized("reply_lease_expired")),Err(e)=>return Err(e.into())}
            let deadline=sql_query("SELECT (l.expires_at_ms>greatest($2,(extract(epoch from clock_timestamp())*1000)::bigint) AND s.valid_until_ms>greatest($2,(extract(epoch from clock_timestamp())*1000)::bigint)) AS matched FROM hagency_agent_v1.execution_leases l JOIN hagency_agent_v1.devices d ON d.id=l.device_id JOIN hagency_agent_v1.sessions s ON s.id=d.session_id WHERE l.agent_id=$1").bind::<Text,_>(&intent.agent_id).bind::<BigInt,_>(now).get_result::<Flag>(db).await?;
            if !deadline.matched {return Err(Error::Unauthorized("reply_lease_expired"));}
            facts(&scope,&requester.id,f,Self::clock(db,now).await?)?;
            let worker=secret_token();
            sql_query("UPDATE hagency_agent_v1.reply_outbox SET state='sending',worker_token=$1,worker_until_ms=$2 WHERE id=$3").bind::<Text,_>(worker).bind::<BigInt,_>(now+self.limits.worker_lease_ms).bind::<Text,_>(id).execute(db).await?;Self::reply(db,id).await
        }).await
    }
    /// Trusted recovery worker only. Recording verified historical network effect
    /// is permitted after revocation; this performs no send, never unblocks delivery,
    /// and never starts a new model execution.
    pub async fn reconcile_reply_sent(
        &self,
        id: &str,
        observed: &ObservedReply,
        now: i64,
    ) -> Result<ReplyIntent> {
        matrix_id(&observed.event_id, '$')?;
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let now=Self::clock(db,now).await?;let intent=Self::reply(db,id).await?;
            if observed.observed_at_ms<0||observed.observed_at_ms>now||now-observed.observed_at_ms>30_000 {return Err(Error::Unavailable("matrix_state_unavailable"));}
            if intent.matrix_txn_id!=observed.matrix_txn_id||intent.puppet_mxid!=observed.sender_mxid||intent.room_id!=observed.room_id||intent.thread_root!=observed.thread_root||intent.body!=observed.body {return Err(Error::Conflict("matrix_reply_observation_mismatch"));}
            if intent.state=="sent" {if intent.matrix_event_id.as_deref()!=Some(&observed.event_id) {return Err(Error::Conflict("matrix_reply_identity_changed"));}return Ok(intent);}
            if !matches!(intent.state.as_str(),"unknown"|"sending") {return Err(Error::Conflict("reply_has_no_uncertain_send"));}
            sql_query("UPDATE hagency_agent_v1.reply_outbox SET state='sent',matrix_event_id=$1,worker_until_ms=$2 WHERE id=$3").bind::<Text,_>(&observed.event_id).bind::<BigInt,_>(now).bind::<Text,_>(id).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.domain_audit(actor_user_id,operation,object_id,at_ms) VALUES($1,'worker.reply.reconciled',$2,$3)").bind::<Text,_>(&intent.owner_user_id).bind::<Text,_>(id).bind::<BigInt,_>(now).execute(db).await?;Self::reply(db,id).await
        }).await
    }
    /// Exact sender observed Matrix HTTP 403 after the pre-send observation.
    /// Keep possible earlier network effects unknown, but prohibit every resend.
    pub(crate) async fn confirm_reply_permission_denied(
        &self,
        id: &str,
        worker: &str,
        now: i64,
    ) -> Result<ReplyIntent> {
        let mut guard = self.db.lock().await;
        (*guard).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::lock(db).await?;
            let current = Self::clock(db, now).await?;
            let intent = Self::reply(db, id).await?;
            if intent.worker_token.as_deref() != Some(worker) { return Err(Error::Conflict("stale_reply_worker")); }
            if intent.state == "unknown" && intent.delivery_blocked { return Ok(intent); }
            if intent.state != "sending" { return Err(Error::Conflict("reply_not_sending")); }
            sql_query("UPDATE hagency_agent_v1.reply_outbox SET state='unknown',delivery_blocked=true,worker_until_ms=$1 WHERE id=$2")
                .bind::<BigInt,_>(current).bind::<Text,_>(id).execute(db).await?;
            Self::reply(db,id).await
        }).await
    }
    /// Record actual send outcome even if authority was revoked while HTTP was in
    /// flight. Revocation cannot physically undo an already issued Matrix send.
    pub async fn confirm_reply(
        &self,
        id: &str,
        worker_token: &str,
        event_id: Option<&str>,
        now: i64,
    ) -> Result<ReplyIntent> {
        if let Some(event) = event_id {
            matrix_id(event, '$')?;
        }
        let mut guard = self.db.lock().await;
        let db = &mut *guard;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let intent=Self::reply(db,id).await?;
            if intent.worker_token.as_deref()!=Some(worker_token) {return Err(Error::Conflict("stale_reply_worker"));}
            if intent.state=="sent" {if intent.matrix_event_id.as_deref()!=event_id {return Err(Error::Conflict("matrix_reply_identity_changed"));}return Ok(intent);}
            if intent.state!="sending" {return Err(Error::Conflict("reply_not_sending"));}
            sql_query("UPDATE hagency_agent_v1.reply_outbox SET state=$1,matrix_event_id=$2,worker_until_ms=$3 WHERE id=$4").bind::<Text,_>(if event_id.is_some(){"sent"}else{"unknown"}).bind::<Nullable<Text>,_>(event_id).bind::<BigInt,_>(Self::clock(db,now).await?).bind::<Text,_>(id).execute(db).await?;Self::reply(db,id).await
        }).await
    }
}
fn device(p: &Principal) -> Result<(&str, i64)> {
    match (&p.device_id, p.device_generation) {
        (Some(id), Some(generation)) if generation > 0 => Ok((id, generation)),
        _ => Err(Error::Unauthorized("device_authorization_required")),
    }
}
fn unauthorized(error: diesel::result::Error) -> Error {
    if error == diesel::result::Error::NotFound {
        Error::Unauthorized("transport_scope_not_authorized")
    } else {
        error.into()
    }
}
fn matrix_id(value: &str, sigil: char) -> Result<()> {
    if value.len() > 512
        || !value.starts_with(sigil)
        || value.len() < 2
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        Err(Error::Invalid("invalid_matrix_identifier"))
    } else {
        Ok(())
    }
}
fn message(body: &str, max: usize) -> Result<()> {
    if body.trim().is_empty() || body.len() > max || body.contains('\0') {
        Err(Error::Invalid("invalid_message_body"))
    } else {
        Ok(())
    }
}
fn observation(scope: &RoutingScope, requester: &str, f: &DeliveryFacts, now: i64) -> Result<()> {
    let now = now.max(crate::api::now_ms());
    if f.observed_at_ms < 0 || f.observed_at_ms > now || now - f.observed_at_ms > 30_000 {
        return Err(Error::Unavailable("matrix_state_unavailable"));
    }
    if scope.owner_mxid != f.owner_mxid
        || scope.puppet_mxid != f.puppet_mxid
        || scope.room_id != f.room_id
        || scope.space_id != f.space_id
        || requester != f.requester_mxid
    {
        return Err(Error::Unauthorized("matrix_observation_scope_mismatch"));
    }
    Ok(())
}
fn facts(scope: &RoutingScope, requester: &str, f: &DeliveryFacts, now: i64) -> Result<()> {
    let now = now.max(crate::api::now_ms());
    if f.observed_at_ms < 0 || f.observed_at_ms > now || now - f.observed_at_ms > 30_000 {
        return Err(Error::Unavailable("matrix_state_unavailable"));
    }
    if !scope.active
        || scope.owner_mxid != f.owner_mxid
        || scope.puppet_mxid != f.puppet_mxid
        || scope.room_id != f.room_id
        || scope.space_id != f.space_id
        || requester != f.requester_mxid
        || !f.owner_in_room
        || (if scope.space_id.is_empty() {
            !f.owner_direct_valid || requester != scope.owner_mxid
        } else {
            !f.owner_in_space || !f.room_in_space
        })
        || !f.requester_in_room
        || !f.puppet_in_room
        || !f.puppet_can_send_message
    {
        return Err(Error::Unauthorized("room_delivery_denied"));
    }
    if f.encrypted {
        return Err(Error::Conflict("encrypted_room_requires_client_crypto"));
    }
    Ok(())
}
fn fenced(event: &Dispatch, p: &Principal, lease: &LeaseRef) -> Result<()> {
    if event.agent_id != lease.agent_id
        || event.dispatch_epoch != Some(lease.epoch)
        || event.dispatch_device_id.as_deref() != Some(device(p)?.0)
    {
        Err(Error::Unauthorized("dispatch_epoch_expired"))
    } else {
        Ok(())
    }
}
#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
