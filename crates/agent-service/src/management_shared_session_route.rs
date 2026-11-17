//! No work promotion, token reservation or process qualification happens here.
use super::*;
use crate::management_shared_session_types::{SharedSessionRoute, SharedSessionRouteInput};

async fn candidate(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    id: &str,
    input: &SharedSessionRouteInput,
    server: &str,
) -> Result<Option<(SharedSessionRoute, Membership)>> {
    enabled_demand(policy)?;
    let r = runtime(db, id).await?;
    let original = original(db, &r).await?;
    owner(&original, p)?;
    let now = clock(db).await?;
    let c = command(&r, server)?;
    if r.digest != input.command_digest
        || !matches!(r.state.as_str(), "pending" | "issued")
        || p.device_generation != Some(original.executor_generation)
        || c.expires_at_ms <= now
        || tickets::mode(db, p, id, &input.token, now).await? != "execute"
        || !super::super::current(db, &r, &original, &c, now).await?
        || !runtime_issuing(db, policy, &r).await?
        || head(db, &r.assignment_id)
            .await?
            .is_none_or(|h| h.id != r.id)
    {
        return Err(Error::Conflict("shared_session_route_authority_changed"));
    }
    require_budget_head(db, &r, server).await?;
    // Only an already issued start for one exact retained demand can route a
    // resume. Bootstrap/legacy idle processes get no speculative room guess.
    let demand = sql_query("SELECT DISTINCT demand_id AS id FROM (SELECT demand_id,runtime_id FROM hagency_agent_v1.management_shared_demand_starts UNION ALL SELECT demand_id,runtime_id FROM hagency_agent_v1.management_shared_demand_retry_starts UNION ALL SELECT demand_id,runtime_id FROM hagency_agent_v1.management_shared_budget_demand_starts) starts WHERE runtime_id=$1 LIMIT 2")
        .bind::<Text,_>(id).load::<Id>(db).await?;
    let demand = match demand.as_slice() {
        [] => return Ok(None),
        [demand] => demand,
        _ => return Err(Error::Conflict("shared_session_route_demand_changed")),
    };
    let d = load(db, &demand.id).await?;
    let q = queued(db, &d.intake_id).await?;
    offer(&d, &q)?;
    authorize(db, p, &q, server, now, true).await?;
    let (state, started, first) = if runtime_version(&r)? == 3 {
        let status = budget::status(db, &d).await?;
        (
            status.state,
            status.runtime_command_id,
            budget::oldest(db, &r.assignment_id, q.control_revision).await?,
        )
    } else {
        let status = status(db, &d).await?;
        (
            status.state,
            status.runtime_command_id,
            oldest(db, &r.assignment_id, q.control_revision).await?,
        )
    };
    if state != "start_committed"
        || started.as_deref() != Some(id)
        || q.assignment_id != r.assignment_id
        || q.expires_at_ms <= now
        || first.is_none_or(|first| first.id != q.id)
    {
        return Err(Error::Conflict("shared_session_route_demand_changed"));
    }
    let event: RetainedEvent = decode(&q.event_payload)?;
    let membership = membership_for(db, &r, &event.room_id).await?;
    if allocation_digest(&event)? != q.event_digest
        || event.event_id != q.event_id
        || event.room_id != membership.room_id
        || event.is_edit
        || !membership.addressed_by(&event.sender_mxid, &event.mentioned_mxids, false)
    {
        return Err(Error::Conflict("shared_event_changed"));
    }
    let route = SharedSessionRoute {
        version: 1,
        runtime_command_id: r.id,
        runtime_command_digest: r.digest,
        assignment_id: r.assignment_id.clone(),
        event_id: event.event_id.clone(),
        body_digest: hash(&event.body),
        room_id: event.room_id,
        requester_mxid: event.sender_mxid,
        thread_root: event.thread_root.unwrap_or(event.event_id),
        room_binding: room_binding(&membership, &r.assignment_id),
        encrypted: event.encrypted,
        native_context_version: (!event.encrypted && policy.shared_thread_context()).then_some(3),
    };
    route.validate()?;
    Ok(Some((route, membership)))
}

impl DomainStore {
    pub async fn shared_native_session_route(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedSessionRouteInput,
        gateway: &Gateway,
    ) -> Result<Option<SharedSessionRoute>> {
        key(id)?;
        input.validate()?;
        let found = {
            let mut guard = self.db.lock().await?;
            (*guard)
                .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, 0).await?;
                    candidate(db, p, policy, id, &input, &self.server).await
                })
                .await?
        };
        let Some((route, membership)) = found else {
            return Ok(None);
        };
        let observed = gateway
            .shared_delivery(
                &membership.room_id,
                &membership.space_id,
                &membership.manager_mxid,
                &route.requester_mxid,
                &membership.puppet_mxid,
            )
            .await?;
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, 0).await?;
                let Some((current, membership)) =
                    candidate(db, p, policy, id, &input, &self.server).await?
                else {
                    return Err(Error::Conflict("shared_session_route_demand_changed"));
                };
                if current != route || observed.requester_mxid != current.requester_mxid {
                    return Err(Error::Conflict("shared_session_route_demand_changed"));
                }
                facts(&membership, &observed)?;
                if observed.encrypted != current.encrypted {
                    return Err(Error::Conflict("shared_session_route_room_changed"));
                }
                Ok(Some(current))
            })
            .await
    }
}
