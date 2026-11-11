//! V3 issuance is explicit; unchanged v1/v2 routes cannot claim v3 work.
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
        (&Method::POST, ["shared-execution", "budget-attempts", "start"]) => {
            json!({"delivery":Box::pin(domain.start_shared_budget_attempt(p,policy,body(req).await?,gateway)).await?})
        }
        (&Method::POST, ["shared-execution", "budget-attempts", "poll"]) => {
            json!({"delivery":domain.poll_shared_budget_attempt(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "budget-attempts", id, "recover"]) => {
            json!({"delivery":domain.recover_shared_budget_attempt(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "budget-attempts", id, "result"]) => {
            serde_json::to_value(
                domain
                    .record_shared_budget_attempt_process(p, policy, id, body(req).await?)
                    .await?,
            )
            .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::POST, ["shared-execution", "budget-activity", "renew"]) => {
            json!({"activity":Box::pin(domain.renew_shared_budget_activity(p,policy,body(req).await?,gateway)).await?})
        }
        (&Method::POST, ["shared-execution", "budget-work", "poll"]) => {
            let input: crate::management_shared_work_types::SharedWorkPollInput = body(req).await?;
            json!({"work":Box::pin(domain.poll_shared_budget_work(p,policy,input.authority,gateway)).await?})
        }
        (&Method::POST, ["shared-execution", "budget-work", "ack"]) => {
            Box::pin(domain.acknowledge_shared_budget_work(p, policy, body(req).await?, gateway))
                .await?;
            json!({"acknowledged":true})
        }
        (&Method::POST, ["shared-execution", "budget-work", "start"]) => serde_json::to_value(
            Box::pin(domain.start_shared_budget_work(p, policy, body(req).await?, gateway)).await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["shared-execution", "budget-work", id, "input-mode"]) => {
            serde_json::to_value(
                domain
                    .shared_budget_work_input_mode(p, id, body(req).await?)
                    .await?,
            )
            .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::POST, ["shared-execution", "budget-work", id, "context"]) => {
            serde_json::to_value(
                Box::pin(domain.freeze_shared_budget_context(
                    p,
                    policy,
                    id,
                    body(req).await?,
                    gateway,
                ))
                .await?,
            )
            .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::POST, ["shared-execution", "budget-work", id, "context", "start"]) => {
            serde_json::to_value(
                Box::pin(domain.start_shared_budget_context_work(
                    p,
                    policy,
                    id,
                    body(req).await?,
                    gateway,
                ))
                .await?,
            )
            .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::POST, ["shared-execution", "budget-work", id, "refuse-unissued"]) => {
            json!({"refusal":domain.refuse_unissued_shared_budget_work(p,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "budget-work", id, "refusal", "recover"]) => {
            json!({"refusal":domain.recover_shared_budget_work_refusal(p,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "budget-demands", "poll"]) => serde_json::to_value(
            domain
                .poll_shared_budget_demand(p, policy, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["shared-execution", "budget-demands", id, "recover"]) => {
            json!({"demand":domain.recover_shared_budget_demand(p,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "budget-demands", id, "start"]) => {
            json!({"delivery":Box::pin(domain.start_shared_budget_demand(p,policy,id,body(req).await?,gateway)).await?})
        }
        (&Method::POST, ["shared-execution", "budget-demands", id, "block"]) => {
            json!({"demand":domain.block_shared_budget_demand(p,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "budget-work", id, "demand"]) => {
            json!({"demand":domain.shared_budget_demand_work_binding(p,id,body(req).await?).await?})
        }
        _ => return Ok(None),
    }))
}
