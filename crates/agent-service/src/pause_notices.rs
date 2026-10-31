//! Automatic feedback for an explicitly paused binding. Uses its own durable
//! outbox: a paused request must never become an inference after resuming.
use super::*;
use diesel::{BoolExpressionMethods, ExpressionMethods, IntoSql, QueryDsl};

pub(crate) const BODY: &str = "Agent service is paused in this Room. Ask its owner to resume it.";
const SELECT: &str = "SELECT id,binding_id,requester_mxid,digest,binding_generation,matrix_txn_id,thread_root,state,worker_token FROM hagency_agent_v1.pause_notice_outbox";
#[derive(Debug, diesel::Queryable, diesel::QueryableByName)]
pub(crate) struct PauseNotice {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub binding_id: String,
    #[diesel(sql_type=Text)]
    pub requester_mxid: String,
    #[diesel(sql_type=Text)]
    pub digest: String,
    #[diesel(sql_type=BigInt)]
    pub binding_generation: i64,
    #[diesel(sql_type=Text)]
    pub matrix_txn_id: String,
    #[diesel(sql_type=Text)]
    pub thread_root: String,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=Nullable<Text>)]
    pub worker_token: Option<String>,
}
/// Relax only the explicit pause bit; all membership, requester, encryption,
/// ownership and Matrix speaking-right requirements stay identical to replies.
pub(super) fn routing_facts(
    scope: &RoutingScope,
    requester: &str,
    f: &DeliveryFacts,
    now: i64,
) -> Result<()> {
    let mut authorized = scope.clone();
    authorized.active |= authorized.service_paused;
    facts(&authorized, requester, f, now)
}
impl TransportStore {
    pub(super) async fn existing_pause_notice(
        db: &mut AsyncPgConnection,
        binding: &str,
        event: &str,
        digest: &str,
    ) -> Result<Option<RouteResult>> {
        let old = sql_query(format!("{SELECT} WHERE binding_id=$1 AND event_id=$2"))
            .bind::<Text, _>(binding)
            .bind::<Text, _>(event)
            .get_result::<PauseNotice>(db)
            .await
            .optional()?;
        match old {
            Some(old) if old.digest != digest => Err(Error::Conflict("event_payload_changed")),
            Some(old) => Ok(Some(RouteResult::Duplicate {
                dispatch_id: old.id,
            })),
            None => Ok(None),
        }
    }
    pub(super) async fn queue_pause_notice(
        &self,
        db: &mut AsyncPgConnection,
        scope: &RoutingScope,
        event: &RoutedEvent,
        digest: &str,
        received: i64,
        now: i64,
    ) -> Result<RouteResult> {
        // Keep suppression records as dedup tombstones, even across pause/resume.
        // A requester can trigger at most one notice per binding in 30 seconds.
        let throttled = sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.pause_notice_outbox WHERE binding_id=$1 AND requester_mxid=$2 AND queued_at_ms>$3 AND state!='cancelled') AS matched")
            .bind::<Text,_>(&scope.binding_id).bind::<Text,_>(&event.sender_mxid)
            .bind::<BigInt,_>(now.saturating_sub(30_000)).get_result::<Flag>(db).await?.matched;
        let pending = sql_query("SELECT count(*) AS total FROM hagency_agent_v1.pause_notice_outbox n JOIN hagency_agent_v1.bindings b ON b.id=n.binding_id JOIN hagency_agent_v1.agents a ON a.id=b.agent_id WHERE a.owner_user_id=$1 AND n.state IN ('pending','sending','unknown')")
            .bind::<Text,_>(&scope.owner_user_id).get_result::<Count>(db).await?.total;
        if !throttled && pending >= self.limits.max_owner_events {
            return Err(Error::Unavailable("owner_notice_queue_full"));
        }
        let root = match event.thread_root.as_deref() {
            Some(root) => {
                matrix_id(root, '$')?;
                root
            }
            None if scope.space_id.is_empty() => &event.room_id,
            None => &event.event_id,
        };
        let id = format!("notice_{}", entity_id()?);
        let txn = format!(
            "hg-paused-{}",
            hash(&format!("{}:{}", scope.binding_id, event.event_id))
        );
        {
            use crate::schema::agent_tables::pause_notice_outbox as q;
            let params = (
                &id,
                &scope.binding_id,
                &event.event_id,
                &event.sender_mxid,
                digest,
                scope.binding_generation,
                txn,
                root,
                received,
                if throttled { "cancelled" } else { "pending" },
                now,
            );
            diesel::insert_into(q::table)
                .values((
                    q::id.eq(&params.0),
                    q::binding_id.eq(&params.1),
                    q::event_id.eq(&params.2),
                    q::requester_mxid.eq(&params.3),
                    q::digest.eq(&params.4),
                    q::binding_generation.eq(&params.5),
                    q::matrix_txn_id.eq(&params.6),
                    q::thread_root.eq(&params.7),
                    q::created_at_ms.eq(&params.8),
                    q::state.eq(&params.9),
                    q::queued_at_ms.eq(&params.10),
                ))
                .execute(db)
                .await
        }?;
        if !throttled {
            {
                use crate::schema::agent_tables::agent_threads as q;
                let params = (&scope.binding_id, root);
                diesel::insert_into(q::table)
                    .values((q::binding_id.eq(&params.0), q::thread_root.eq(&params.1)))
                    .on_conflict_do_nothing()
                    .execute(db)
                    .await
            }?;
        }
        Ok(RouteResult::Ignored)
    }
    pub(crate) async fn pause_notice_candidates(
        &self,
        cursor: &str,
        now: i64,
    ) -> Result<Vec<PauseNotice>> {
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;
            let now = Self::clock(db,now).await?;
            sql_query("UPDATE hagency_agent_v1.pause_notice_outbox n SET state='cancelled' WHERE n.state IN ('pending','sending','unknown') AND (n.created_at_ms<=$1 OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id WHERE b.id=n.binding_id AND b.generation=n.binding_generation AND b.state='suspended' AND b.owner_service_paused AND NOT b.admin_project_paused AND NOT b.admin_room_paused AND a.state='active' AND u.active AND ((b.scope_kind='project' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))))")
                .bind::<BigInt,_>(now.saturating_sub(self.limits.event_ttl_ms)).execute(db).await?;
            // All transactions older than this time are rejected at ingest. Keep
            // recent final rows to make AS replay deterministic after a restart.
            {
                use crate::schema::agent_tables::pause_notice_outbox as q;
                let params = (now.saturating_sub(self.limits.event_ttl_ms), now);
                diesel::delete(q::table)
                    .filter(
                        ((q::state.eq_any(vec!["sent", "cancelled"])).and((q::created_at_ms).le(&params.0)))
                            .and((q::worker_until_ms).le(&params.1)),
                    )
                    .execute(db)
                    .await
            }?;
            Ok(sql_query(format!("{SELECT} WHERE state IN ('pending','unknown','sending') AND worker_until_ms<=$1 ORDER BY (id<=$2),id LIMIT 20"))
                .bind::<BigInt,_>(now).bind::<Text,_>(cursor).load(db).await?)
        }).await
    }).await
    }
    pub(crate) async fn pause_notice_scope(&self, binding: &str) -> Result<RoutingScope> {
        self.db
            .with_connection(async |db| {
                let scope = Self::scope(&mut *db, binding).await?;
                if !scope.service_paused {
                    return Err(Error::Unauthorized("binding_not_paused"));
                }
                Ok(scope)
            })
            .await
    }
    #[cfg(test)]
    pub(crate) async fn claim_pause_notice(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<PauseNotice> {
        self.claim_pause_notice_guarded(id, f, now, None).await
    }
    pub(crate) async fn claim_pause_notice_guarded(
        &self,
        id: &str,
        f: &DeliveryFacts,
        now: i64,
        gate: Option<&crate::worker_runtime::ClaimGate>,
    ) -> Result<PauseNotice> {
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::lock(db).await?;
            {
                use crate::schema::agent_tables::pause_notice_outbox as q;
                let params = (id,);
                q::table
                    .select((true.into_sql::<diesel::sql_types::Bool>(),))
                    .filter((q::id).eq(&params.0))
                    .for_update()
                    .get_result::<Flag>(db)
                    .await
            }?;
            let now = Self::clock(db,now).await?;
            let n = sql_query(format!("{SELECT} WHERE id=$1"))
                .bind::<Text,_>(id).get_result::<PauseNotice>(db).await?;
            let scope = Self::scope(db,&n.binding_id).await?;
            let valid = scope.service_paused && scope.binding_generation == n.binding_generation;
            let authorized = if valid {
                match routing_facts(&scope,&n.requester_mxid,f,now) {
                    Ok(()) => true,
                    Err(e @ Error::Unavailable(_)) => return Err(e),
                    Err(_) => false,
                }
            } else {false};
            if !authorized {
                {
                    use crate::schema::agent_tables::pause_notice_outbox as q;
                    let params = (id,);
                    diesel::update(q::table)
                        .filter(
                            ((q::id).eq(&params.0))
                                .and(diesel::dsl::not(q::state.eq_any(vec!["sent", "cancelled"]))),
                        )
                        .set((q::state.eq("cancelled"),))
                        .execute(db)
                        .await
                }?;
                return Ok(None);
            }
            if !matches!(n.state.as_str(),"pending"|"unknown"|"sending") {return Ok(None);}
            let _admission=gate.map(crate::worker_runtime::ClaimGate::admit).transpose()?;
            let worker = secret_token();
            Ok(sql_query(format!("UPDATE hagency_agent_v1.pause_notice_outbox SET state='sending',worker_token=$2,worker_until_ms=$3 WHERE id=$1 AND worker_until_ms<=$4 RETURNING {}",SELECT.strip_prefix("SELECT ").unwrap().split(" FROM ").next().unwrap()))
                .bind::<Text,_>(id).bind::<Text,_>(worker).bind::<BigInt,_>(now+self.limits.worker_lease_ms).bind::<BigInt,_>(now)
                .get_result::<PauseNotice>(db).await.optional()?)
        }).await?.ok_or(Error::Conflict("pause_notice_not_sendable"))
    }).await
    }
    pub(crate) async fn confirm_pause_notice(
        &self,
        id: &str,
        worker: &str,
        outcome: &Result<String>,
        now: i64,
    ) -> Result<()> {
        self.db.with_connection(async |db| {

        let state = match outcome {
            Ok(_) => "sent",
            Err(Error::Conflict("matrix_permission_missing")) => "cancelled",
            Err(_) => "unknown",
        };
        sql_query("UPDATE hagency_agent_v1.pause_notice_outbox SET state=$3,matrix_event_id=$4,worker_token=NULL,worker_until_ms=$5 WHERE id=$1 AND worker_token=$2 AND state='sending'")
            .bind::<Text,_>(id).bind::<Text,_>(worker).bind::<Text,_>(state)
            .bind::<Nullable<Text>,_>(outcome.as_ref().ok().map(String::as_str))
            .bind::<BigInt,_>(if state=="unknown" {now+5000} else {0}).execute(&mut *db).await?;
        Ok(())
    }).await
    }
}
