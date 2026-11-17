//! Cleanup-only routes retain the existing SDK session and device authorization.
use super::*;
pub(super) async fn call(
    req: &mut Request,
    domain: &DomainStore,
    p: &crate::store::Principal,
    parts: &[&str],
) -> Result<Option<Value>> {
    Ok(Some(match (req.method(), parts) {
        (&Method::GET, ["agent-requests", id, "settlement"]) => {
            json!({"settlement":Box::pin(domain.managed_agent_settlement(p,id)).await?})
        }
        (&Method::POST, ["agent-requests", id, "settlement"]) => {
            json!({"requested":Box::pin(domain.request_agent_settlement(p,id,body(req).await?)).await?})
        }
        (&Method::POST, ["agent-settlement-executor", "poll"]) => {
            json!({"delivery":Box::pin(domain.poll_agent_settlement(p,body(req).await?)).await?})
        }
        (&Method::POST, ["agent-settlement-executor", id, "recover"]) => {
            json!({"delivery":Box::pin(domain.recover_agent_settlement(p,id,body(req).await?)).await?})
        }
        (&Method::POST, ["agent-settlement-executor", id, "result"]) => {
            json!({"receipt":Box::pin(domain.record_agent_settlement(p,id,body(req).await?)).await?})
        }
        _ => return Ok(None),
    }))
}
