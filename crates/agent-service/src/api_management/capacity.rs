//! Bounded route group; authentication, query rejection and final session recheck stay in the parent.
use super::*;

pub(super) async fn call(
    req: &mut Request,
    domain: &DomainStore,
    policy: &ManagementPolicy,
    p: &crate::store::Principal,
    gateway: &crate::gateway::Gateway,
    matrix: Option<&crate::matrix_client::MatrixClient>,
    parts: &[&str],
) -> Result<Option<Value>> {
    Ok(Some(match (req.method(), parts) {
        (&Method::GET, ["projects", id, "setup"]) => {
            json!({"readiness":domain.management_project_setup(p,policy,id).await?})
        }
        (&Method::POST, ["projects", id, "setup"]) => {
            json!({"readiness":domain.begin_management_project_setup(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["project-setups", id, "cancel"]) => {
            json!({"readiness":domain.cancel_management_project_setup(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["project-setups", id, "rebase"]) => {
            json!({"readiness":domain.rebase_management_project_setup(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["project-setups", id, "confirm"]) => {
            let input = body(req).await?;
            let joined = domain
                .accept_management_setup_service(p, policy, id, &input, gateway, matrix)
                .await?;
            let result = domain
                .confirm_management_project_setup(p, policy, id, input, gateway)
                .await;
            // A join may have committed before a cancellation or lost response.
            // Keep the original native command pending rather than classifying
            // a post-join validation failure as a definitely unapplied command.
            let readiness = result.map_err(|e| {
                if joined {
                    Error::Unavailable("project_setup_membership_outcome_unknown")
                } else {
                    e
                }
            })?;
            json!({"readiness":readiness})
        }
        (&Method::POST, ["project-setup-commands", id, "recover"]) => {
            domain
                .recover_management_setup_command(p, policy, id, body(req).await?)
                .await?
        }
        (&Method::GET, ["grants", id, "settlement"]) => {
            json!({"settlement":domain.management_parent_settlement(p,policy,id).await?})
        }
        (&Method::POST, ["grants", id, "close"]) => {
            json!({"settlement":domain.close_management_parent(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["grants", id, "settle"]) => {
            json!({"settlement":domain.settle_management_parent(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["parent-commands", id, "recover"]) => {
            domain
                .recover_management_parent_command(p, policy, id, body(req).await?)
                .await?
        }
        (&Method::POST, ["grant-executor-commands", "poll"]) => {
            json!({"delivery":domain.poll_management_parent_executor(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["grant-executor-commands", id, "recover"]) => {
            json!({"delivery":domain.recover_management_parent_executor(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["grant-executor-commands", id, "result"]) => serde_json::to_value(
            domain
                .record_management_parent_result(p, policy, id, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["contributions", id, "grants"]) => {
            json!({"grant":domain.publish_management_grant(p,policy,id,body(req).await?).await?})
        }
        (&Method::GET, ["grants", id]) => {
            json!({"grant":domain.management_resource_grant(p,policy,id).await?})
        }
        (&Method::POST, ["grants", id, "revoke"]) => {
            json!({"grant":domain.revoke_management_grant(p,policy,id,body(req).await?).await?})
        }
        (&Method::GET, ["allocations", id]) => {
            json!({"detail":domain.management_allocation_detail(p,policy,id).await?})
        }
        (&Method::POST, ["projects", id, "allocations", "query"]) => {
            json!({"allocations":domain.management_project_allocations(p,policy,id,body(req).await?).await?})
        }
        (&Method::GET, ["projects", id, "allocation"]) => {
            json!({"allocation":domain.management_project_allocation(p,policy,id).await?})
        }
        (&Method::POST, ["allocation-requests", id, "decide"]) => {
            json!({"allocation":domain.decide_management_allocation(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["allocations", id, "release"]) => {
            json!({"allocation":domain.release_management_allocation(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["allocation-commands", id, "recover"]) => {
            domain
                .recover_management_allocation_command(p, policy, id, body(req).await?)
                .await?
        }
        (&Method::POST, ["executor-commands", "poll"]) => {
            json!({"delivery":domain.poll_management_executor(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["executor-commands", id, "recover"]) => {
            json!({"delivery":domain.recover_management_executor(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["executor-commands", id, "result"]) => serde_json::to_value(
            domain
                .record_management_executor_result(p, policy, id, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        _ => return Ok(None),
    }))
}
