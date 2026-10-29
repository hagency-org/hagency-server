//! Restriction-only operations remain reachable when new admission is disabled.
use super::*;
use crate::management_agent_cleanup_types::*;
use crate::management_agent_executor_types::ManagedAgentFenceReceipt;

#[derive(QueryableByName)]
struct Json {
    #[diesel(sql_type=Text)]
    payload: String,
}
#[derive(QueryableByName)]
struct Fence {
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Nullable<Text>)]
    receipt: Option<String>,
}
async fn cleanup_visible(db: &mut AsyncPgConnection, r: &Row, p: &Principal) -> Result<Roles> {
    super::visible(db,r,p).await
}
impl DomainStore {
    pub async fn managed_agent_removal(
        &self,
        p: &Principal,
        id: &str,
    ) -> Result<AgentRemovalProgress> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, crate::api::now_ms()).await?;
            let r = row(db, id).await?;
            let a = cleanup_visible(db, &r, p).await?;
            let fence = sql_query("SELECT w.state,w.receipt FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1 AND w.operation='agent.fence'")
                .bind::<Text,_>(id).get_result::<Fence>(db).await.optional()?;
            let native = fence.as_ref().and_then(|f|f.receipt.as_deref()).map(decode::<ManagedAgentFenceReceipt>).transpose()?;
            let original_reserve = sql_query("SELECT w.receipt AS payload FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1 AND w.operation='agent.reserve' AND w.receipt IS NOT NULL").bind::<Text,_>(id).get_result::<Json>(db).await.optional()?.map(|v|decode::<ManagedAgentReservationReceipt>(&v.payload)).transpose()?;
            let native_fence = fence.map(|f| AgentFenceProgress { delivery_state:f.state, native_state:native.as_ref().map(|v|v.state.clone()), ledger_revision:native.as_ref().map(|v|v.ledger_revision), process_state:native.as_ref().map(|v|v.process_state.clone()) });
            let central_fenced = matches!(r.state.as_str(),"cancelled"|"revoked"|"rejected") || sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_intents WHERE request_id=$1 AND state='fenced') AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;
            let process = sql_query("SELECT json_build_object('kind',kind,'state',payload::jsonb->>'state','ledgerRevision',ledger_revision)::text AS payload FROM (SELECT 'shared_text' AS kind,o.payload,o.ledger_revision FROM hagency_agent_v1.management_shared_process_receipts o JOIN hagency_agent_v1.management_shared_runtimes x ON x.id=o.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=x.assignment_id WHERE a.request_id=$1 AND x.id=(SELECT head.id FROM hagency_agent_v1.management_shared_runtimes head WHERE head.assignment_id=a.id ORDER BY coalesce((head.payload::jsonb->>'processAttempt')::bigint,1) DESC LIMIT 1) UNION ALL SELECT 'qualification',o.payload,o.ledger_revision FROM hagency_agent_v1.management_agent_runtime_observations o JOIN hagency_agent_v1.management_agent_runtime_commands c ON c.id=o.command_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=c.assignment_id WHERE a.request_id=$1 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes head WHERE head.assignment_id=a.id)) observations ORDER BY ledger_revision DESC LIMIT 1").bind::<Text,_>(id).get_result::<Json>(db).await.optional()?.map(|v|decode::<AgentProcessObservation>(&v.payload)).transpose()?;
            let matrix = sql_query("SELECT json_build_object('state',m.state,'cleanupObservedAtMs',m.cleanup_observed_at_ms)::text AS payload FROM hagency_agent_v1.management_agent_matrix m JOIN hagency_agent_v1.management_shared_assignments a ON a.id=m.assignment_id WHERE a.request_id=$1").bind::<Text,_>(id).get_result::<Json>(db).await.optional()?.map(|v|decode::<AgentMatrixCleanup>(&v.payload)).transpose()?;
            let observed = sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work_receipts o JOIN hagency_agent_v1.management_shared_work w ON w.id=o.work_id JOIN hagency_agent_v1.management_shared_runtimes x ON x.id=w.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=x.assignment_id WHERE a.request_id=$1) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;
            let known = usage::known(db,usage::Scope::Agent(id)).await?;
            let known = (observed || native.is_some()).then(||known.max(native.as_ref().map_or(0,|v|v.known_consumed_tokens)));
            // Null/unknown cleanup cannot erase a previously confirmed hold.
            // Only a complete native fence can lower that original balance;
            // subsequent consumed usage remains an independent lower bound.
            let mut retained_floor = native.as_ref().and_then(|v|v.retained_tokens);
            if native.as_ref().is_none_or(|v|!v.usage_complete) {
                retained_floor = retained_floor.into_iter().chain(original_reserve.as_ref().filter(|v|v.state=="reserved").map(|v|v.retained_tokens)).max();
                retained_floor = retained_floor.into_iter().chain(budget::cleanup_commitment(db,id,&self.server).await?).max();
            }
            retained_floor = retained_floor.into_iter().chain(known).max();
            let settlement = super::settlement::cleanup_balance(db,id,&self.server).await?;
            let known=known.into_iter().chain(settlement.map(|v|v.0)).max();
            if let Some((_,floor,_))=settlement { retained_floor=Some(floor); }
            let usage = AgentCleanupUsage {period_key:r.period_key, known_consumed_tokens:known, retained_tokens_floor:retained_floor, fence_usage_complete:native.as_ref().map(|v|v.usage_complete), requires_reconciliation:settlement.map_or_else(||native.as_ref().is_some_and(|v|known.is_some_and(|n|n>v.known_consumed_tokens)),|v|v.2)};
            let definition: AgentDefinition = decode(&r.definition)?;
            Ok(AgentRemovalProgress { request_id:r.id, project_id:r.plan_id, project_name:a.project_name, proposed_agent_id:r.proposed_agent_id, agent_name:definition.name, request_revision:r.revision, can_cancel:p.user_id==r.manager_user_id && matches!(r.state.as_str(),"pending_approval"|"approved_waiting_executor"), can_revoke:p.user_id==a.administrator_user_id && r.state=="approved_waiting_executor", admission_state:r.state, central_fenced, native_fence, process_observation:process, matrix, usage })
        }).await
    }
    pub async fn managed_agent_removals(
        &self,
        p: &Principal,
        input: AgentRemovalQuery,
    ) -> Result<AgentRemovalPage> {
        for id in [&input.project_id, &input.after].into_iter().flatten() {
            key(id)?;
        }
        let limit = input.limit.unwrap_or(16);
        if !(1..=16).contains(&limit) {
            return Err(Error::Invalid("invalid_management_limit"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection|{
            Self::authorize(db,p,crate::api::now_ms()).await?;
            if let Some(project)=&input.project_id {
                let a=roles(db,project).await?;
                if a.manager_user_id!=p.user_id && a.administrator_user_id!=p.user_id {return Err(Error::NotFound("agent_request_not_found"));}
            }
            let mut rows=sql_query("SELECT json_build_object('requestId',r.id,'projectId',r.plan_id,'projectName',q.name,'agentName',r.definition::jsonb->>'name')::text AS payload FROM hagency_agent_v1.management_agent_requests r JOIN hagency_agent_v1.management_projects q ON q.id=r.plan_id JOIN hagency_agent_v1.management_project_authorities a ON a.plan_id=r.plan_id WHERE (r.manager_user_id=$1 OR a.administrator_user_id=$1) AND ($2::text IS NULL OR r.plan_id=$2) AND r.id>$3 ORDER BY r.id LIMIT $4").bind::<Text,_>(&p.user_id).bind::<Nullable<Text>,_>(input.project_id.as_deref()).bind::<Text,_>(input.after.as_deref().unwrap_or("")).bind::<BigInt,_>(i64::from(limit)+1).load::<Json>(db).await?;
            let more=rows.len()>usize::from(limit);rows.truncate(usize::from(limit));
            let items=rows.into_iter().map(|v|decode::<AgentRemovalIndexItem>(&v.payload)).collect::<Result<Vec<_>>>()?;
            let next_after=if more {items.last().map(|v|v.request_id.clone())}else{None};
            Ok(AgentRemovalPage{items,next_after})
        }).await
    }
}
