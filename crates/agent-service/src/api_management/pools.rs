use super::*;

pub(super) async fn call(
    req: &mut Request,
    domain: &DomainStore,
    policy: &ManagementPolicy,
    p: &crate::store::Principal,
    parts: &[&str],
) -> Result<Option<Value>> {
    Ok(Some(match (req.method(), parts) {
        (&Method::POST, ["pool-agent-commands", id, "recover"]) => {
            domain
                .recover_pool_agent_command(p, id, body(req).await?)
                .await?
        }
        (&Method::POST, ["pools", "query"]) => {
            serde_json::to_value(domain.resource_pools(p, policy, body(req).await?).await?)
                .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::POST, ["pools", pool, "agents"]) => json!({
            "request": domain.request_pool_agent(p, policy, pool, body(req).await?).await?
        }),
        (&Method::POST, ["pool-agent-requests", "query"]) => {
            serde_json::to_value(domain.pool_agent_requests(p, body(req).await?).await?)
                .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::GET, ["pool-agent-requests", id]) => json!({
            "request": domain.pool_agent_request(p, id).await?
        }),
        (&Method::POST, ["pool-agent-requests", id, "decide"]) => json!({
            "request": domain.decide_pool_agent(p, policy, id, body(req).await?).await?
        }),
        (&Method::POST, ["pool-agent-requests", id, "stop"]) => json!({
            "request": domain.stop_pool_agent(p, id, body(req).await?).await?
        }),
        _ => return Ok(None),
    }))
}
