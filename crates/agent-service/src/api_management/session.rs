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
        (&Method::GET, ["session"]) => {
            let mut features = vec![
                "resource-owner-coordinator-v1",
                crate::management_agent_cleanup_types::AGENT_CLEANUP_FEATURE,
                crate::management_agent_settlement_types::AGENT_SETTLEMENT_FEATURE,
                // Read/recovery/Stop remain available when issuance is disabled.
                crate::management_shared_intake_types::SHARED_RUN_CONTROL_FEATURE,
            ];
            // A first owner grant must be discoverable before any capacity
            // exists. Allocation readiness remains a separate check below.
            if policy.project_reservations() {
                features.push(crate::management_allocation_types::ADMISSION_BOUND_GRANTS_FEATURE);
            }
            if domain.management_allocation_supported(p, policy).await? {
                features.push(crate::management_allocation_types::ALLOCATION_FEATURE);
                features.push(crate::management_allocation_types::ALLOCATION_REVIEW_FEATURE);
            }
            // Discovery describes the explicitly enabled protocol. Capacity
            // readiness independently gates effects; cleanup remains visible
            // when reservation issuance is disabled or no live hold remains.

            if policy.agent_admission() && policy.project_reservations() {
                features.push(crate::management_pool_types::POOL_AGENTS_FEATURE);
                features.push(crate::management_agent_types::AGENT_ADMISSION_FEATURE);
            }
            if policy.shared_execution() {
                features.push(crate::management_shared_work_types::SHARED_WORK_HISTORY_FEATURE);
            }
            json!({"protocol":PROTOCOL,"account":p.mxid,"device":p.device_id,"deviceGeneration":p.device_generation,"administrator":policy.administrator(),"canAdmitResources":p.mxid==policy.administrator(),"canApproveProjects":false,"features":features})
        }
        _ => return Ok(None),
    }))
}
