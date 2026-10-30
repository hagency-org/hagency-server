//! Bounded route group; authentication, query rejection and final session recheck stay in the parent.
use super::*;

pub(super) async fn call(
    req: &mut Request,
    domain: &DomainStore,
    policy: &ManagementPolicy,
    p: &crate::store::Principal,
    gateway: &crate::gateway::Gateway,
    parts: &[&str],
    matrix: Option<&crate::matrix_client::MatrixClient>,
) -> Result<Option<Value>> {
    if parts.get(1)==Some(&"crypto") && !policy.shared_budget_execution() {return Err(Error::Unavailable("shared_encryption_unavailable"));}
    Ok(Some(match (req.method(), parts) {
        (&Method::POST,["shared-execution","crypto","poll"]) => {
            #[derive(serde::Deserialize)] #[serde(rename_all="camelCase",deny_unknown_fields)] struct Poll { after: Option<String> }
            let input:Poll=body(req).await?;
            domain.shared_crypto_poll(p,input.after.as_deref()).await?
        }
        (&Method::POST,["shared-execution","crypto","device"]) => domain.shared_crypto_device(p,body(req).await?,matrix.ok_or(Error::Unavailable("matrix_unavailable"))?).await?,
        (&Method::POST,["shared-execution","crypto","thread"]) => domain.shared_crypto_thread(p,body(req).await?).await?,
        (&Method::POST,["shared-execution","crypto","send"]) => {
            #[derive(serde::Deserialize)] #[serde(rename_all="camelCase",deny_unknown_fields)] struct Send {work_id:String}
            let input:Send=body(req).await?;
            domain.authorize_shared_encrypted_reply(p,policy,&input.work_id,gateway).await?;
            json!({"authorized":true})
        }
        (&Method::POST,["shared-execution","crypto","confirm"]) => domain.shared_crypto_confirm(p,body(req).await?,gateway).await?,
        (&Method::POST,["shared-execution","approvals","register"])=>domain.register_shared_approval(p,body(req).await?).await?,
        (&Method::POST,["shared-execution","approvals","query"])=>domain.shared_approvals(p,body(req).await?).await?,
        (&Method::POST,["shared-execution","approvals","get"])=>{
            #[derive(serde::Deserialize)] #[serde(deny_unknown_fields)] struct Lookup{id:String}
            let input:Lookup=body(req).await?;domain.shared_approval(p,&input.id).await?
        },
        (&Method::POST,["shared-execution","approvals","decide"])=>domain.decide_shared_approval(p,body(req).await?).await?,
        (&Method::POST,["shared-execution","approvals","take"])=>domain.take_shared_approval(p,body(req).await?).await?,
        (&Method::POST, ["shared-execution", "work", id, "input-mode"]) => serde_json::to_value(
            domain
                .shared_work_input_mode(p, id, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["shared-execution", "work", id, "reply", "observe"]) => {
            serde_json::to_value(
                Box::pin(domain.observe_shared_reply(p, id, body(req).await?)).await?,
            )
            .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::POST, ["shared-execution", "work", id, "context"]) => serde_json::to_value(
            Box::pin(domain.freeze_shared_context(p, policy, id, body(req).await?, gateway))
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["shared-execution", "work", id, "context", "recover"]) => {
            serde_json::to_value(
                domain
                    .recover_shared_context(p, id, body(req).await?)
                    .await?,
            )
            .map_err(|_| Error::Unavailable("encoding_failed"))?
        }
        (&Method::POST, ["shared-execution", "work", id, "context", "start"]) => {
            serde_json::to_value(
                Box::pin(domain.start_shared_context_work(
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

        (&Method::POST, ["shared-execution", "demands", "poll"]) => serde_json::to_value(
            domain
                .poll_shared_demand(p, policy, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["shared-execution", "demands", id, "start"]) => {
            json!({"delivery":Box::pin(domain.start_shared_demand(p,policy,id,body(req).await?,gateway)).await?})
        }
        (&Method::POST, ["shared-execution", "demands", id, "recover"]) => {
            json!({"demand":domain.recover_shared_demand(p,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "demands", id, "block"]) => {
            json!({"demand":domain.block_shared_demand(p,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "work", id, "demand"]) => {
            json!({"demand":domain.shared_demand_work_binding(p,id,body(req).await?).await?})
        }

        (
            &Method::GET,
            [
                "shared-execution",
                "assignments",
                id,
                "control",
                "readiness",
            ],
        ) => {
            json!({"readiness":domain.shared_run_control_readiness(p,policy,id).await?})
        }
        (&Method::GET, ["shared-execution", "assignments", id, "control"]) => {
            json!({"control":domain.shared_run_control(p,id).await?})
        }
        (&Method::POST, ["shared-execution", "assignments", id, "control"]) => {
            serde_json::to_value(
                Box::pin(domain.change_shared_run_control(
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
        (&Method::POST, ["shared-execution", "assignments", id, "intake", "query"]) => {
            json!({"intake":domain.shared_intake_history(p,id,body(req).await?).await?})
        }

        (&Method::POST, ["shared-execution", "attempts", "start"]) => {
            json!({"delivery":domain.start_shared_attempt(p,policy,body(req).await?,gateway).await?})
        }
        (&Method::POST, ["shared-execution", "attempts", "poll"]) => {
            json!({"delivery":domain.poll_shared_attempt(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "attempts", id, "recover"]) => {
            json!({"delivery":domain.recover_shared_attempt(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "attempts", id, "result"]) => serde_json::to_value(
            domain
                .record_shared_attempt_process(p, policy, id, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["shared-execution", "work", "poll"]) => {
            let input: crate::management_shared_work_types::SharedWorkPollInput = body(req).await?;
            json!({"work":domain.poll_shared_work(p,policy,input.authority,gateway).await?})
        }
        (&Method::POST, ["shared-execution", "work", "ack"]) => {
            domain
                .acknowledge_shared_work(p, policy, body(req).await?, gateway)
                .await?;
            json!({"acknowledged":true})
        }
        (&Method::POST, ["shared-execution", "work", "start"]) => serde_json::to_value(
            domain
                .start_shared_work(p, policy, body(req).await?, gateway)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["shared-execution", "work", id, "recover"]) => {
            json!({"ticket":domain.recover_shared_work_result(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "work", id, "result"]) => serde_json::to_value(
            domain
                .record_shared_work_result(p, policy, id, body_with_limit(req, 64 * 1024).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["agent-work", "list"]) => {
            json!({"requests":domain.shared_work_index(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["agent-requests", id, "work", "query"]) => {
            json!({"work":domain.shared_work_history(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "runtime", "poll"]) => {
            json!({"delivery":domain.poll_shared_runtime(p,policy,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "runtime", id, "recover"]) => {
            json!({"delivery":domain.recover_shared_runtime(p,policy,id,body(req).await?).await?})
        }
        (&Method::POST, ["shared-execution", "runtime", id, "result"]) => serde_json::to_value(
            domain
                .record_shared_process(p, policy, id, body(req).await?)
                .await?,
        )
        .map_err(|_| Error::Unavailable("encoding_failed"))?,
        (&Method::POST, ["shared-execution", "activity", "renew"]) => {
            json!({"activity":domain.renew_shared_activity(p,policy,body(req).await?,gateway).await?})
        }
        (&Method::GET, ["agent-requests", id, "execution"]) => {
            json!({"execution":domain.shared_execution_status(p,policy,id).await?})
        }
        _ => return Ok(None),
    }))
}
