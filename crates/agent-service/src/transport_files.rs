use super::*;
use crate::file_delivery::{
    CHUNK_BYTES, Delivery, FileLookup, FileObservation, MAX_BYTES, MAX_CHUNKS, Manifest, Target,
};
use diesel::sql_types::{Binary, Jsonb};
use diesel::{BoolExpressionMethods, ExpressionMethods, IntoSql, QueryDsl};
use serde_json::Value;
const SELECT: &str = "SELECT id,request_id,owner_event_id,execution_id,agent_id,binding_id,owner_user_id,room_id,puppet_mxid,requester_mxid,thread_root,binding_generation,dispatch_epoch,dispatch_device_id,manifest,manifest_digest,received_bytes,state,mxc_url,worker_phase,worker_token,worker_until_ms,matrix_event_id,created_at_ms FROM hagency_agent_v1.file_outbox";
#[derive(diesel::Queryable, diesel::QueryableByName)]
pub(crate) struct FileRow {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    request_id: String,
    #[diesel(sql_type=Text)]
    pub owner_event_id: String,
    #[diesel(sql_type=Text)]
    execution_id: String,
    #[diesel(sql_type=Text)]
    agent_id: String,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub puppet_mxid: String,
    #[diesel(sql_type=Text)]
    pub requester_mxid: String,
    #[diesel(sql_type=Text)]
    thread_root: String,
    #[diesel(sql_type=BigInt)]
    binding_generation: i64,
    #[diesel(sql_type=BigInt)]
    dispatch_epoch: i64,
    #[diesel(sql_type=Text)]
    dispatch_device_id: String,
    #[diesel(sql_type=Jsonb)]
    manifest: Value,
    #[diesel(sql_type=Text)]
    manifest_digest: String,
    #[diesel(sql_type=BigInt)]
    received_bytes: i64,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=Nullable<Text>)]
    mxc_url: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    worker_phase: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    worker_token: Option<String>,
    #[diesel(sql_type=BigInt)]
    worker_until_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    matrix_event_id: Option<String>,
    #[diesel(sql_type=BigInt)]
    created_at_ms: i64,
}
#[derive(diesel::Queryable, diesel::QueryableByName)]
struct Blob {
    #[diesel(sql_type=Binary)]
    content: Vec<u8>,
}
/// Private original claim. Never serialize it or recreate it from a public view.
pub(crate) struct FileClaim {
    pub row: FileRow,
    pub token: String,
    pub bytes: Option<Vec<u8>>,
}
impl FileRow {
    pub(crate) fn upload_filename(&self) -> Result<String> {
        Ok(self.manifest()?.filename)
    }
    fn manifest(&self) -> Result<Manifest> {
        let manifest: Manifest = serde_json::from_value(self.manifest.clone())
            .map_err(|_| Error::Unavailable("invalid_stored_file"))?;
        if manifest.digest()? != self.manifest_digest
            || self.received_bytes < 0
            || self.received_bytes as u64 > manifest.bytes
        {
            return Err(Error::Unavailable("invalid_stored_file"));
        }
        Ok(manifest)
    }
    fn public(&self) -> Result<Delivery> {
        Ok(Delivery {
            id: self.id.clone(),
            dispatch_id: self.owner_event_id.clone(),
            execution_id: self.execution_id.clone(),
            request_id: self.request_id.clone(),
            created_at_ms: self.created_at_ms,
            manifest: self.manifest()?,
            manifest_digest: self.manifest_digest.clone(),
            received_bytes: self.received_bytes as u64,
            state: self.state.clone(),
            upload_accepted: self.mxc_url.is_some(),
            matrix_event_id: self.matrix_event_id.clone(),
        })
    }
    pub(crate) fn content(&self) -> Result<Value> {
        let f = self.manifest()?;
        Ok(crate::file_delivery::content(
            &f.filename,
            f.caption.as_deref(),
            f.bytes,
            self.mxc_url
                .as_deref()
                .ok_or(Error::Conflict("file_not_uploaded"))?,
            &self.id,
            &self.room_id,
            &self.thread_root,
        ))
    }
}
async fn row(db: &mut AsyncPgConnection, id: &str) -> Result<FileRow> {
    sql_query(format!("{SELECT} WHERE id=$1"))
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(|e| {
            if e == diesel::result::Error::NotFound {
                Error::NotFound("file_delivery_not_found")
            } else {
                e.into()
            }
        })
}
/// Read a legacy prefix plus the immutable appended chunks once at queue/claim.
/// Staging writes never concatenate the growing file.
async fn file_bytes(db: &mut AsyncPgConnection, id: &str) -> Result<Vec<u8>> {
    let blob=sql_query("SELECT o.content||coalesce((SELECT string_agg(c.content,''::bytea ORDER BY c.byte_offset) FROM hagency_agent_v1.file_chunks c WHERE c.file_id=o.id),''::bytea) AS content FROM hagency_agent_v1.file_outbox o WHERE o.id=$1")
        .bind::<Text,_>(id).get_result::<Blob>(db).await?;
    Ok(blob.content)
}
/// Replay compares only the requested range, including ranges crossing a legacy
/// prefix or chunk boundary. Never transfer the entire staged file for a retry.
async fn file_range(
    db: &mut AsyncPgConnection,
    id: &str,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>> {
    let blob=sql_query("SELECT substring(o.content FROM $2+1 FOR $3)||coalesce((SELECT string_agg(substring(c.content FROM greatest($2-c.byte_offset,0)+1 FOR least(c.byte_offset+octet_length(c.content),$2+$3)-greatest(c.byte_offset,$2)),''::bytea ORDER BY c.byte_offset) FROM hagency_agent_v1.file_chunks c WHERE c.file_id=o.id AND c.byte_offset>$2-65536 AND c.byte_offset<$2+$3 AND c.byte_offset+octet_length(c.content)>$2),''::bytea) AS content FROM hagency_agent_v1.file_outbox o WHERE o.id=$1")
        .bind::<Text,_>(id).bind::<diesel::sql_types::Integer,_>(offset as i32).bind::<diesel::sql_types::Integer,_>(len as i32).get_result::<Blob>(db).await?;
    Ok(blob.content)
}
fn target_matches(r: &FileRow, p: &Principal, target: &Target) -> Result<()> {
    if r.owner_user_id != p.user_id
        || r.owner_event_id != target.dispatch_id
        || r.execution_id != target.execution_id
    {
        return Err(Error::NotFound("file_delivery_not_found"));
    }
    Ok(())
}
impl TransportStore {
    /// Shared trusted observations fence original file intents even after the
    /// model's completed outcome, without rewriting that outcome or text replies.
    pub(super) async fn block_files_observed(
        db: &mut AsyncPgConnection,
        scope: &RoutingScope,
        requester: &str,
        f: &DeliveryFacts,
    ) -> Result<()> {
        let binding_denied = !scope.active
            || !f.owner_in_room
            || !f.puppet_in_room
            || !f.puppet_can_send_message
            || f.encrypted
            || if scope.space_id.is_empty() {
                !f.owner_direct_valid
            } else {
                !f.owner_in_space || !f.room_in_space
            };
        if binding_denied || !f.requester_in_room {
            sql_query("INSERT INTO hagency_agent_v1.file_delivery_denials(owner_event_id) SELECT e.id FROM hagency_agent_v1.owner_events e WHERE e.binding_id=$1 AND e.binding_generation=$2 AND ($3 OR e.requester_mxid=$4) AND (e.state IN ('pending','offered','acknowledged','running') OR EXISTS(SELECT 1 FROM hagency_agent_v1.file_outbox o WHERE o.owner_event_id=e.id AND o.state NOT IN ('sent','cancelled'))) ON CONFLICT DO NOTHING")
                .bind::<Text,_>(&scope.binding_id).bind::<BigInt,_>(scope.binding_generation).bind::<Bool,_>(binding_denied).bind::<Text,_>(requester).execute(db).await?;
        }
        Ok(())
    }
    async fn file_allowed(db: &mut AsyncPgConnection, event: &str) -> Result<()> {
        let denied=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.file_delivery_denials WHERE owner_event_id=$1) AS matched").bind::<Text,_>(event).get_result::<Flag>(db).await?;
        if denied.matched {
            Err(Error::Unauthorized("file_delivery_permanently_blocked"))
        } else {
            Ok(())
        }
    }
    async fn writable_file(
        db: &mut AsyncPgConnection,
        p: &Principal,
        lease: &LeaseRef,
        target: &Target,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<FileRow> {
        target.validate()?;
        let r = row(db, &target.file_id).await?;
        if r.dispatch_epoch != lease.epoch || r.dispatch_device_id != device(p)?.0 {
            return Err(Error::Unauthorized("dispatch_epoch_expired"));
        }
        target_matches(&r, p, target)?;
        let event = Self::dispatch(db, p, &target.dispatch_id).await?;
        Self::dispatch_scope(db, p, lease, &event, f, now).await?;
        Self::file_allowed(db, &target.dispatch_id).await?;
        fenced(&event, p, lease)?;
        if event.state != "running" || event.execution_id.as_deref() != Some(&target.execution_id) {
            return Err(Error::Conflict("execution_not_running"));
        }
        Ok(r)
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_file(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        dispatch: &str,
        execution: &str,
        request_id: &str,
        file: Manifest,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<Delivery> {
        key(request_id)?;
        key(execution)?;
        let digest = file.digest()?;
        self.observe_event_authority(p, dispatch, f, now).await?;
        if !f.requester_in_room {
            self.reject_event_delivery(dispatch, f, now).await?;
            return Err(Error::Unauthorized("requester_membership_revoked"));
        }
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let (now,_)=Self::authenticate(db,p,now).await?;
            let e=Self::dispatch(db,p,dispatch).await?;Self::dispatch_scope(db,p,lease,&e,f,now).await?;Self::file_allowed(db,dispatch).await?;fenced(&e,p,lease)?;
            if e.state!="running"||e.execution_id.as_deref()!=Some(execution) {return Err(Error::Conflict("execution_not_running"))}
            let prior=sql_query(format!("{SELECT} WHERE owner_event_id=$1 AND request_id=$2")).bind::<Text,_>(dispatch).bind::<Text,_>(request_id).get_result::<FileRow>(db).await.optional()?;
            if let Some(prior)=prior {if prior.manifest_digest!=digest||prior.manifest!=serde_json::to_value(&file).map_err(|_|Error::Invalid("invalid_file_manifest"))?||prior.execution_id!=execution {return Err(Error::Conflict("file_request_changed"))}return prior.public();}
            let count={
                use crate::schema::agent_tables::file_outbox as q;
                let params = (&p.user_id,);
                q::table
                    .select((diesel::dsl::count_star(),))
                    .filter(
                        ((q::owner_user_id).eq(&params.0))
                            .and(diesel::dsl::not(q::state.eq_any(vec!["sent", "cancelled"]))),
                    )
                    .get_result::<Count>(db)
                    .await
            }?;
            let held=sql_query("SELECT coalesce(sum((manifest->>'bytes')::bigint),0)::bigint AS total FROM hagency_agent_v1.file_outbox WHERE owner_user_id=$1 AND state NOT IN ('sent','cancelled')").bind::<Text,_>(&p.user_id).get_result::<Count>(db).await?;
            if count.total>=4096||held.total.saturating_add(file.bytes as i64)>128*1024*1024 {return Err(Error::Conflict("file_capacity_exceeded"))}
            let scope=Self::scope(db,&e.binding_id).await?;let id=format!("fil_{}",entity_id()?);
            {
                use crate::schema::agent_tables::file_outbox as q;
                let params = (
                    &id,
                    request_id,
                    dispatch,
                    execution,
                    &e.agent_id,
                    &e.binding_id,
                    &p.user_id,
                    &e.room_id,
                    &scope.puppet_mxid,
                    &e.requester_mxid,
                    &e.thread_root,
                    e.binding_generation,
                    lease.epoch,
                    device(p)?.0,
                    serde_json::to_value(&file).map_err(|_| Error::Invalid("invalid_file_manifest"))?,
                    digest,
                    now,
                );
                diesel::insert_into(q::table)
                    .values((
                        q::id.eq(&params.0),
                        q::request_id.eq(&params.1),
                        q::owner_event_id.eq(&params.2),
                        q::execution_id.eq(&params.3),
                        q::agent_id.eq(&params.4),
                        q::binding_id.eq(&params.5),
                        q::owner_user_id.eq(&params.6),
                        q::room_id.eq(&params.7),
                        q::puppet_mxid.eq(&params.8),
                        q::requester_mxid.eq(&params.9),
                        q::thread_root.eq(&params.10),
                        q::binding_generation.eq(&params.11),
                        q::dispatch_epoch.eq(&params.12),
                        q::dispatch_device_id.eq(&params.13),
                        q::manifest.eq(&params.14),
                        q::manifest_digest.eq(&params.15),
                        q::state.eq("staging"),
                        q::created_at_ms.eq(&params.16),
                    ))
                    .execute(db)
                    .await
            }?;
            row(db,&id).await?.public()
        }).await
    }).await
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn stage_file_chunk(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        target: &Target,
        offset: u64,
        bytes: &[u8],
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<Delivery> {
        if bytes.is_empty() || bytes.len() > CHUNK_BYTES || offset > MAX_BYTES as u64 {
            return Err(Error::Invalid("invalid_file_chunk"));
        }
        self.observe_event_authority(p, &target.dispatch_id, f, now)
            .await?;
        if !f.requester_in_room {
            self.reject_event_delivery(&target.dispatch_id, f, now)
                .await?;
            return Err(Error::Unauthorized("requester_membership_revoked"));
        }
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::lock(db).await?;
                    let (now, _) = Self::authenticate(db, p, now).await?;
                    let r = Self::writable_file(db, p, lease, target, f, now).await?;
                    if r.state != "staging" {
                        return Err(Error::Conflict("file_not_staging"));
                    }
                    let manifest = r.manifest()?;
                    if offset.saturating_add(bytes.len() as u64) > manifest.bytes {
                        return Err(Error::Invalid("file_size_mismatch"));
                    }
                    if offset < r.received_bytes as u64 {
                        if offset + bytes.len() as u64 > r.received_bytes as u64 {
                            return Err(Error::Conflict("file_chunk_changed"));
                        }
                        let stored = file_range(db, &r.id, offset, bytes.len()).await?;
                        if stored != bytes {
                            return Err(Error::Conflict("file_chunk_changed"));
                        }
                        return r.public();
                    }
                    if offset != r.received_bytes as u64 {
                        return Err(Error::Conflict("file_chunk_gap"));
                    }
                    let chunks = {
                        use crate::schema::agent_tables::file_chunks as q;
                        let params = (&r.id,);
                        q::table
                            .select((diesel::dsl::count_star(),))
                            .filter((q::file_id).eq(&params.0))
                            .get_result::<Count>(db)
                            .await
                    }?
                    .total as usize;
                    // Never accept a tiny chunk that leaves too few slots to complete
                    // the original manifest with legal maximum-size chunks.
                    let remaining = manifest.bytes - offset - bytes.len() as u64;
                    if chunks >= MAX_CHUNKS
                        || remaining > (MAX_CHUNKS - chunks - 1) as u64 * CHUNK_BYTES as u64
                    {
                        return Err(Error::Invalid("file_chunk_limit"));
                    }
                    {
                        use crate::schema::agent_tables::file_chunks as q;
                        let params = (&r.id, offset as i32, bytes);
                        diesel::insert_into(q::table)
                            .values((
                                q::file_id.eq(&params.0),
                                q::byte_offset.eq(&params.1),
                                q::content.eq(&params.2),
                            ))
                            .execute(db)
                            .await
                    }?;
                    {
                        use crate::schema::agent_tables::file_outbox as q;
                        let params = (&r.id, bytes.len() as i64);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.0))
                            .set((q::received_bytes.eq(q::received_bytes + params.1),))
                            .execute(db)
                            .await
                    }?;
                    row(db, &r.id).await?.public()
                })
                .await
            })
            .await
    }
    pub async fn queue_file(
        &self,
        p: &Principal,
        lease: &LeaseRef,
        target: &Target,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<Delivery> {
        self.observe_event_authority(p, &target.dispatch_id, f, now)
            .await?;
        if !f.requester_in_room {
            self.reject_event_delivery(&target.dispatch_id, f, now)
                .await?;
            return Err(Error::Unauthorized("requester_membership_revoked"));
        }
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::lock(db).await?;
                    let (now, _) = Self::authenticate(db, p, now).await?;
                    let r = Self::writable_file(db, p, lease, target, f, now).await?;
                    if r.state != "staging" {
                        return r.public();
                    }
                    let f = r.manifest()?;
                    let bytes = file_bytes(db, &r.id).await?;
                    if bytes.len() as u64 != f.bytes
                        || r.received_bytes as u64 != f.bytes
                        || hash_bytes(&bytes) != f.sha256
                    {
                        return Err(Error::Conflict("file_bytes_mismatch"));
                    }
                    {
                        use crate::schema::agent_tables::file_outbox as q;
                        let params = (&r.id,);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.0))
                            .set((q::state.eq("queued"),))
                            .execute(db)
                            .await
                    }?;
                    row(db, &r.id).await?.public()
                })
                .await
            })
            .await
    }
    /// Historical safe status. This never grants current upload/send authority.
    pub async fn file_delivery(
        &self,
        p: &Principal,
        target: &Target,
        now: i64,
    ) -> Result<Delivery> {
        target.validate()?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authenticate(db, p, now).await?;
                    let r = row(db, &target.file_id).await?;
                    target_matches(&r, p, target)?;
                    r.public()
                })
                .await
            })
            .await
    }
    /// Read-only recovery of the original preparation, never a new write grant.
    /// The transport/auth transaction lock serializes absence with prepare and
    /// terminal execution transitions, including across Server instances.
    pub async fn lookup_file(
        &self,
        p: &Principal,
        request: &FileLookup,
        now: i64,
    ) -> Result<FileObservation> {
        request.validate()?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authenticate(db, p, now).await?;
                    let event = Self::dispatch(db, p, &request.dispatch_id).await?;
                    if event.execution_id.as_deref() != Some(&request.execution_id) {
                        return Err(Error::NotFound("file_execution_not_found"));
                    }
                    let prior = sql_query(format!(
                        "{SELECT} WHERE owner_event_id=$1 AND execution_id=$2 AND request_id=$3"
                    ))
                    .bind::<Text, _>(&request.dispatch_id)
                    .bind::<Text, _>(&request.execution_id)
                    .bind::<Text, _>(&request.request_id)
                    .get_result::<FileRow>(db)
                    .await
                    .optional()?;
                    Ok(FileObservation {
                        request: request.clone(),
                        delivery: prior.map(|r| r.public()).transpose()?,
                        preparation_closed: matches!(
                            event.state.as_str(),
                            "unknown" | "completed" | "cancelled"
                        ),
                    })
                })
                .await
            })
            .await
    }
    pub(crate) async fn expire_files(&self, now: i64) -> Result<()> {
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let now=Self::clock(db,now).await?;
            {
                use crate::schema::agent_tables::file_outbox as q;
                let params = (now,);
                diesel::update(q::table)
                    .filter(
                        (q::state.eq_any(vec!["uploading", "sending"])).and((q::worker_until_ms).le(&params.0)),
                    )
                    .set((q::state.eq("unknown"),))
                    .execute(db)
                    .await
            }?;
            sql_query("UPDATE hagency_agent_v1.file_outbox SET state='cancelled',content=''::bytea WHERE state IN ('staging','queued','uploaded') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.file_sendable v WHERE v.id=file_outbox.id)").execute(db).await?;Ok(())
        }).await
    }).await
    }
    pub(crate) async fn file_candidates(&self, cursor: &str) -> Result<Vec<FileRow>> {
        self.db
            .with_connection(async |db| {
                Ok(sql_query(format!(
                    "{SELECT} WHERE state IN ('queued','uploaded') ORDER BY (id<=$1),id LIMIT 10"
                ))
                .bind::<Text, _>(cursor)
                .load(&mut *db)
                .await?)
            })
            .await
    }
    #[cfg(test)]
    pub(crate) async fn claim_file(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<FileClaim> {
        self.claim_file_guarded(id, f, now, None).await
    }
    pub(crate) async fn claim_file_guarded(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
        gate: Option<&crate::worker_runtime::ClaimGate>,
    ) -> Result<FileClaim> {
        let original = {
            self.db
                .with_connection(async |db| row(&mut *db, id).await)
                .await?
        };
        self.observe_delivery_authority(
            None,
            &original.binding_id,
            &original.requester_mxid,
            Some(&original.owner_event_id),
            f,
            now,
        )
        .await?;
        if !f.requester_in_room {
            self.reject_event_delivery(&original.owner_event_id, f, now)
                .await?;
            return Err(Error::Unauthorized("requester_membership_revoked"));
        }
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;
            {
                use crate::schema::agent_tables::file_outbox as q;
                let params = (id,);
                q::table
                    .select((true.into_sql::<diesel::sql_types::Bool>(),))
                    .filter((q::id).eq(&params.0))
                    .for_update()
                    .get_result::<Flag>(db)
                    .await
            }?;
            let now=Self::clock(db,now).await?;let r=row(db,id).await?;
            if !matches!(r.state.as_str(),"queued"|"uploaded") {return Err(Error::Conflict("file_not_sendable"))}
            let scope=Self::scope(db,&r.binding_id).await?;facts(&scope,&r.requester_mxid,f,now)?;Self::file_allowed(db,&r.owner_event_id).await?;
            if scope.binding_generation!=r.binding_generation||scope.owner_user_id!=r.owner_user_id||scope.agent_id!=r.agent_id||scope.puppet_mxid!=r.puppet_mxid {return Err(Error::Unauthorized("file_scope_revoked"))}
            // Revocation may use another database connection. Hold the actual
            // device/session/user rows until the irreversible intent commits.
            let locked=sql_query("SELECT true AS matched FROM hagency_agent_v1.execution_leases l JOIN hagency_agent_v1.devices d ON d.id=l.device_id JOIN hagency_agent_v1.sessions s ON s.id=d.session_id JOIN hagency_agent_v1.users u ON u.id=d.user_id WHERE l.agent_id=$1 AND l.owner_user_id=$2 AND l.epoch=$3 AND l.device_id=$4 AND l.device_generation=d.generation AND d.user_id=l.owner_user_id AND s.user_id=l.owner_user_id AND NOT d.revoked AND NOT s.revoked AND u.active FOR SHARE OF d,s,u")
                .bind::<Text,_>(&r.agent_id).bind::<Text,_>(&r.owner_user_id).bind::<BigInt,_>(r.dispatch_epoch).bind::<Text,_>(&r.dispatch_device_id).get_result::<Flag>(db).await;
            match locked {Ok(_)=>(),Err(diesel::result::Error::NotFound)=>return Err(Error::Unauthorized("file_lease_expired")),Err(e)=>return Err(e.into())}
            let phase=if r.state=="queued" {"upload"}else{"send"};let state=if phase=="upload" {"uploading"}else{"sending"};
            let bytes=if phase=="upload" {let bytes=file_bytes(db,id).await?;let manifest=r.manifest()?;if bytes.len() as u64!=manifest.bytes||hash_bytes(&bytes)!=manifest.sha256 {return Err(Error::Unavailable("invalid_stored_file"))}Some(bytes)}else{None};
            // Recheck after lock waits and byte hashing, using the real DB clock.
            let now=Self::clock(db,now).await?;facts(&scope,&r.requester_mxid,f,now)?;
            let live=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.file_sendable WHERE id=$1) AND EXISTS(SELECT 1 FROM hagency_agent_v1.owner_events WHERE id=$2 AND created_at_ms>$3) AS matched").bind::<Text,_>(id).bind::<Text,_>(&r.owner_event_id).bind::<BigInt,_>(now.saturating_sub(self.limits.event_ttl_ms)).get_result::<Flag>(db).await?;
            if !live.matched {return Err(Error::Unauthorized("file_lease_expired"))}
            let _admission=gate.map(crate::worker_runtime::ClaimGate::admit).transpose()?;
            let token=secret_token();{
                use crate::schema::agent_tables::file_outbox as q;
                let params = (id, state, phase, &token, now + 60_000);
                diesel::update(q::table)
                    .filter((q::id).eq(&params.0))
                    .set((
                        q::state.eq(&params.1),
                        q::worker_phase.eq(&params.2),
                        q::worker_token.eq(&params.3),
                        q::worker_until_ms.eq(&params.4),
                    ))
                    .execute(db)
                    .await
            }?;
            Ok(FileClaim{row:row(db,id).await?,token,bytes})
        }).await
    }).await
    }
    pub(crate) async fn confirm_file_upload(
        &self,
        id: &str,
        token: &str,
        mxc: Option<&str>,
        now: i64,
    ) -> Result<()> {
        if let Some(url) = mxc {
            crate::attachments::Attachment::from_content(
                &serde_json::json!({"msgtype":"m.file","body":"file","url":url}),
            )?;
        }
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::lock(db).await?;
                    let now = Self::clock(db, now).await?;
                    let r = row(db, id).await?;
                    if r.worker_phase.as_deref() != Some("upload")
                        || r.worker_token.as_deref() != Some(token)
                    {
                        return Err(Error::Conflict("stale_file_worker"));
                    }
                    if let Some(known) = &r.mxc_url {
                        if Some(known.as_str()) != mxc {
                            return Err(Error::Conflict("file_media_changed"));
                        }
                        return Ok(());
                    }
                    if !matches!(r.state.as_str(), "uploading" | "unknown") {
                        return Err(Error::Conflict("file_not_uploading"));
                    }
                    let state =
                        if r.state == "uploading" && r.worker_until_ms > now && mxc.is_some() {
                            "uploaded"
                        } else {
                            "unknown"
                        };
                    if r.state == "unknown" && mxc.is_none() {
                        return Ok(());
                    }
                    sql_query(
                        "UPDATE hagency_agent_v1.file_outbox SET state=$2,mxc_url=$3 WHERE id=$1",
                    )
                    .bind::<Text, _>(id)
                    .bind::<Text, _>(state)
                    .bind::<Nullable<Text>, _>(mxc)
                    .execute(db)
                    .await?;
                    Ok(())
                })
                .await
            })
            .await
    }
    pub(crate) async fn confirm_file_sent(
        &self,
        id: &str,
        token: &str,
        event: Option<&str>,
    ) -> Result<()> {
        if let Some(event) = event {
            matrix_id(event, '$')?;
        }
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;let r=row(db,id).await?;
            if r.worker_phase.as_deref()!=Some("send")||r.worker_token.as_deref()!=Some(token) {return Err(Error::Conflict("stale_file_worker"))}
            if r.state=="sent" {if r.matrix_event_id.as_deref()!=event {return Err(Error::Conflict("file_message_changed"))}return Ok(())}
            if !matches!(r.state.as_str(),"sending"|"unknown") {return Err(Error::Conflict("file_not_sending"))}
            if event.is_none()&&r.state=="unknown" {return Ok(())}
            sql_query("UPDATE hagency_agent_v1.file_outbox SET state=$2,matrix_event_id=$3,content=CASE WHEN $3::text IS NOT NULL THEN ''::bytea ELSE content END WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(if event.is_some(){"sent"}else{"unknown"}).bind::<Nullable<Text>,_>(event).execute(db).await?;Ok(())
        }).await
    }).await
    }
    /// Verified Appservice observation can settle an original possible send.
    /// It never retries an upload or publication, or queues model inference.
    pub(crate) async fn observe_file_event(&self, event: &Value) -> Result<bool> {
        if event["type"] != "m.room.message" || event["content"]["msgtype"] != "m.file" {
            return Ok(false);
        }
        let Some(id) = event["content"]["org.hagency.file_delivery_id"].as_str() else {
            return Ok(false);
        };
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;
            let r=match row(db,id).await {Ok(r)=>r,Err(Error::NotFound(_))=>return Ok(false),Err(e)=>return Err(e)};
            if r.worker_phase.as_deref()!=Some("send") || !matches!(r.state.as_str(),"sending"|"unknown"|"sent") || event["sender"]!=r.puppet_mxid||event["room_id"]!=r.room_id||event["content"]!=r.content()? {return Ok(false)}
            let event=event["event_id"].as_str().ok_or(Error::Invalid("invalid_event"))?;matrix_id(event,'$')?;
            if r.state=="sent" {if r.matrix_event_id.as_deref()!=Some(event){return Err(Error::Conflict("file_message_changed"))}return Ok(true)}
            sql_query("UPDATE hagency_agent_v1.file_outbox SET state='sent',matrix_event_id=$2,content=''::bytea WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(event).execute(db).await?;Ok(true)
        }).await
    }).await
    }
}
fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
