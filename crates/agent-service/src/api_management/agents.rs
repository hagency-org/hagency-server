//! Bounded route group; authentication, query rejection and final session recheck stay in the parent.
use super::*;

pub(super) async fn call(
    req: &mut Request,
    domain: &DomainStore,
    policy: &ManagementPolicy,
    p: &crate::store::Principal,
    gateway: &crate::gateway::Gateway,
    parts: &[&str],
) -> Result<Option<Value>> {
    Ok(Some(match (req.method(), parts) {
        (&Method::POST, ["agent-runtime", "poll"]) => {
            json!({"delivery":domain.poll_managed_runtime(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["agent-runtime", id, "recover"]) => {
            json!({"delivery":domain.recover_managed_runtime(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["agent-runtime", id, "result"]) => serde_json::to_value(
            domain
                .record_managed_runtime(p, policy, id, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::GET, ["agent-requests", id, "runtime"]) => {
            json!({"runtime":domain.managed_runtime(p,policy,id).await?})
        }
        (&Method::POST, ["agent-matrix", "poll"]) => {
            json!({"delivery":domain.poll_managed_agent_matrix(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["agent-matrix", id, "recover"]) => {
            json!({"delivery":domain.recover_managed_agent_matrix(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["agent-matrix", id, "result"]) => serde_json::to_value(
            domain
                .record_managed_agent_matrix(p, policy, id, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::GET, ["agent-requests", id, "matrix"]) => {
            json!({"membership":domain.managed_agent_matrix(p,policy,id).await?})
        }
        (&Method::POST, ["agent-executor", "poll"]) => {
            json!({"delivery":domain.poll_managed_agent_executor(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["agent-executor", id, "recover"]) => {
            json!({"delivery":domain.recover_managed_agent_executor(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["agent-executor", id, "result"]) => serde_json::to_value(
            domain
                .record_managed_agent_executor_result(p, policy, id, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::GET, ["agent-requests", id, "assignment"]) => {
            json!({"assignment":domain.managed_agent_assignment(p,policy,id).await?})
        }
        (&Method::POST, ["agent-removals", "list"]) => {
            json!({"requests":domain.managed_agent_removals(p,body(req).await?).await?})
        }
        (&Method::GET, ["agent-requests", id, "removal"]) => {
            json!({"removal":domain.managed_agent_removal(p,id).await?})
        }
        (&Method::POST, ["agent-requests", id, "revoke"]) => {
            json!({"agentRequest":domain.revoke_managed_agent_request(p,policy,id,body(req).await?).await?})
        }
        (&Method::GET, ["projects", id, "agent-readiness"]) => {
            json!({"readiness":domain.management_agent_readiness(p,policy,id).await?})
        }
        (&Method::POST, ["projects", id, "agent-requests"]) => {
            json!({"agentRequest":domain.request_managed_agent(p,policy,id,body(req).await?,gateway).await?})
        }
        (&Method::POST, ["agent-requests", "list"]) => serde_json::to_value(
            domain
                .management_agent_requests(p, policy, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::GET, ["agent-requests", id]) => {
            json!({"agentRequest":domain.management_agent_request(p,policy,id).await?})
        }
        (&Method::POST, ["agent-requests", id, "decide"]) => {
            json!({"agentRequest":domain.decide_managed_agent(p,policy,id,body(req).await?,gateway).await?})
        }
        (&Method::POST, ["agent-requests", id, "cancel"]) => {
            json!({"agentRequest":domain.cancel_managed_agent_request(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["agent-commands", id, "recover"]) => {
            domain
                .recover_managed_agent_command(p, policy, id, body(req).await?)
                .await?
        }
        (&Method::PUT, ["projects", id, "administrator"]) => {
            json!({"readiness":domain.assign_management_project_admin(p,policy,id,body(req).await?).await?})
        }
        _ => return Ok(None),
    }))
}
