//! Bounded route group; authentication, query rejection and final session recheck stay in the parent.
use super::*;

pub(super) async fn call(
    req: &mut Request,
    domain: &DomainStore,
    policy: &ManagementPolicy,
    p: &crate::store::Principal,
    _gateway: &crate::gateway::Gateway,
    parts: &[&str],
) -> Result<Option<Value>> {
    Ok(Some(match (req.method(), parts) {
        (&Method::POST, ["catalog"]) => {
            serde_json::to_value(domain.management_catalog(p, body(req).await?).await?)
                .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::POST, ["projects"]) => {
            return Err(Error::Invalid("projects_removed_use_resource_pools"));
        }
        (&Method::POST, ["projects", "list"]) => serde_json::to_value(
            domain
                .management_projects(p, policy, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::GET, ["projects", id]) => {
            json!({"project":domain.management_project(p,policy,id).await?})
        }
        (&Method::POST, ["projects", id, "revoke"]) => {
            json!({"project":domain.revoke_management_project(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["project-requests", id, "decide"]) => {
            json!({"project":domain.decide_management_project(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["project-commands", id, "recover"]) => {
            let result = domain
                .recover_management_project_command(p, policy, id, body(req).await?)
                .await?;
            json!({"recorded":result.is_some(),"project":result})
        }
        (&Method::POST, ["commands", id, "recover"]) => {
            let result = domain
                .recover_management_command(p, policy, id, body(req).await?)
                .await?;
            json!({"recorded":result.is_some(),"admission":result})
        }
        (&Method::POST, ["contributions"]) => {
            json!({"admission":domain.create_contribution(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["contributions", "list"]) => {
            serde_json::to_value(domain.contributions(p, policy, body(req).await?).await?)
                .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::GET, ["contributions", id]) => {
            json!({"admission":domain.contribution(p,policy,id).await?})
        }
        (&Method::POST, ["contributions", id, "revoke"]) => {
            json!({"admission":domain.revoke_contribution(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["contributions", id, "resume"]) => {
            json!({"admission":domain.resume_contribution(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["contributions", id, "verify"]) => {
            json!({"admission":domain.verify_contribution(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["inbox"]) => {
            serde_json::to_value(domain.management_inbox(p, policy, body(req).await?).await?)
                .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::GET, ["requests", id]) => {
            json!({"request":domain.management_request(p,policy,id).await?})
        }
        (&Method::POST, ["requests", id, "decide"]) => {
            json!({"admission":domain.decide_management_request(p,policy,id,body(req).await?).await?})
        }
        _ => return Ok(None),
    }))
}
