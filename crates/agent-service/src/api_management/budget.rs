//! Phase-A budget evidence routes. Discovery does not advertise executable top-up support.
use super::*;
pub(super) async fn call(
    req: &mut Request,
    domain: &DomainStore,
    policy: &ManagementPolicy,
    p: &crate::store::Principal,
    parts: &[&str],
) -> Result<Option<Value>> {
    Ok(Some(match (req.method(), parts) {
        (&Method::GET, ["agent-requests", id, "budget"]) => {
            json!({"budget":domain.agent_budget(p,policy,id).await?})
        }
        (&Method::POST, ["agent-requests", id, "topups"]) => serde_json::to_value(
            Box::pin(domain.request_agent_topup(p, policy, id, body(req).await?)).await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["agent-requests", id, "topups", "query"]) => {
            json!({"topUps":domain.query_agent_topups(p,policy,id,body(req).await?).await?})
        }
        (&Method::GET, ["agent-topups", id]) => {
            json!({"topUp":domain.agent_topup(p,policy,id).await?})
        }
        (&Method::POST, ["agent-topups", id, "decide"]) => serde_json::to_value(
            Box::pin(domain.decide_agent_topup(p, policy, id, body(req).await?)).await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["agent-topups", id, "cancel"]) => serde_json::to_value(
            Box::pin(domain.cancel_agent_topup(p, policy, id, body(req).await?)).await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["agent-topup-commands", id, "recover"]) => {
            domain
                .recover_agent_topup(p, policy, id, body(req).await?)
                .await?
        }
        (&Method::POST, ["agent-budget-executor", "poll"]) => {
            json!({"delivery":Box::pin(domain.poll_agent_budget(p,policy,body(req).await?)).await?})
        }
        (&Method::POST, ["agent-budget-executor", id, "recover"]) => {
            json!({"delivery":Box::pin(domain.recover_agent_budget(p,policy,id,body(req).await?)).await?})
        }
        (&Method::POST, ["agent-budget-executor", id, "result"]) => {
            json!({"receipt":Box::pin(domain.record_agent_budget(p,id,body(req).await?)).await?})
        }
        _ => return Ok(None),
    }))
}
