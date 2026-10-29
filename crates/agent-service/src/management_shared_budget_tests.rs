use super::*;
#[derive(QueryableByName)]
struct Clock {
    #[diesel(sql_type=BigInt)]
    now: i64,
}
use crate::management_shared_work_types::{SharedWorkRecoverInput, SharedWorkResultUpload};
use crate::{management_agent_budget_types::*, management_shared_budget_types::*};

async fn apply_delta(
    f: &Fixture,
    r: &ManagedAgentRequest,
    admin: &Actor,
    amount: i64,
    revision: i64,
) -> SharedExecutionBudget {
    let b = f
        .domain
        .agent_budget(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap();
    let t = f
        .domain
        .request_agent_topup(
            &f.outsider.p,
            &f.policy,
            &r.id,
            RequestAgentTopUp { approve_immediately: false, owner_funded: false,
                command_id: format!("delta_{revision}"),
                expected_request_revision: r.revision,
                expected_assignment_generation: 1,
                expected_budget_revision: b.budget_revision,
                expected_budget_digest: b.budget_digest,
                add_tokens: amount,
                purpose: "Existing approved agent continuation".into(),
            },
        )
        .await
        .unwrap();
    f.domain
        .decide_agent_topup(
            &admin.p,
            &f.policy,
            &t.top_up.id,
            DecideAgentTopUp {
                command_id: format!("approve_{revision}"),
                expected_revision: 1,
                expected_authority_revision: 1,
                decision: Decision::Approve,
            },
        )
        .await
        .unwrap();
    let d = f
        .domain
        .poll_agent_budget(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    let c = &d.command;
    let receipt = AgentBudgetReceipt {
        version: 1,
        command_id: c.command_id.clone(),
        command_digest: d.command_digest.clone(),
        top_up_id: c.top_up_id.clone(),
        agent_reservation_id: c.reserve_receipt.agent_reservation_id.clone().unwrap(),
        assignment_id: c.assignment_id.clone(),
        original_reserve_receipt_digest: c.reserve_receipt_digest.clone(),
        state: "applied".into(),
        expected_budget_revision: c.expected_budget_revision,
        previous_budget_digest: c.expected_budget_digest.clone(),
        budget_revision: c.expected_budget_revision + 1,
        budget_digest: next_budget_digest(c, revision).unwrap(),
        previous_ceiling_tokens: c.expected_ceiling_tokens,
        applied_delta_tokens: c.add_tokens,
        effective_ceiling_tokens: c.expected_ceiling_tokens + c.add_tokens,
        ledger_revision: revision,
        period_key: c.reserve_command.period_key.clone(),
        reason: None,
    };
    f.domain
        .record_agent_budget(
            &f.owner.p,
            &c.command_id,
            AgentBudgetResult {
                command_digest: d.command_digest.clone(),
                token: d.token,
                receipt: receipt.clone(),
            },
        )
        .await
        .unwrap();
    SharedExecutionBudget {
        version: 1,
        original_reserve_receipt_digest: c.reserve_receipt_digest.clone(),
        budget_revision: receipt.budget_revision,
        budget_digest: receipt.budget_digest.clone(),
        effective_ceiling_tokens: receipt.effective_ceiling_tokens,
        applied_command_id: c.command_id.clone(),
        applied_command_digest: d.command_digest,
        applied_receipt_digest: allocation_digest(&receipt).unwrap(),
    }
}
async fn fixture() -> (
    Fixture,
    ManagedAgentRequest,
    Actor,
    MatrixFixture,
    SharedBudgetAttemptStartInput,
) {
    let (mut f, r, admin, _, m) = Box::pin(prepared()).await;
    f.policy = f
        .policy
        .with_shared_successors(true)
        .with_agent_budget_deltas(true)
        .with_shared_budget_execution(true);
    let (d, _) = qualify(&f).await;
    let stop = attempts::stop(&f, &d, 220).await;
    let pin = apply_delta(&f, &r, &admin, 25, 230).await;
    let old = attempts::start(&d, &stop);
    let mut obs = old.budget_observation;
    obs.allocated_tokens = pin.effective_ceiling_tokens;
    obs.remaining_tokens = pin.effective_ceiling_tokens - obs.retained_tokens;
    obs.observed_ledger_revision = 231;
    (
        f,
        r,
        admin,
        m,
        SharedBudgetAttemptStartInput {
        web_search: false, tool_policy: None,
            start_intent_id: old.start_intent_id,
            assignment_id: old.assignment_id,
            predecessor: old.predecessor,
            execution_budget: pin,
            budget_observation: obs,
        },
    )
}
fn evidence(
    d: &SharedBudgetAttemptDelivery,
    state: &str,
    revision: i64,
) -> SharedBudgetAttemptResultInput {
    SharedBudgetAttemptResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: SharedBudgetAttemptProcessReceipt {
            version: 3,
            observation_id: format!("{}_obs_{revision}", d.command.command_id),
            command_id: d.command.command_id.clone(),
            command_digest: d.command_digest.clone(),
            assignment_id: d.command.assignment_id.clone(),
            agent_reservation_id: d.command.agent_reservation_id.clone(),
            process_attempt: d.command.process_attempt,
            original_runtime_digest: d.command.original_runtime_digest.clone(),
            budget_revision: d.command.execution_budget.budget_revision,
            budget_digest: d.command.execution_budget.budget_digest.clone(),
            ledger_revision: revision,
            process_digest: Some("d".repeat(64)),
            state: state.into(),
            policy_digest: (state == "qualified").then(|| "e".repeat(64)),
        },
    }
}
async fn started(
    f: &Fixture,
    m: &MatrixFixture,
    i: SharedBudgetAttemptStartInput,
) -> (SharedBudgetAttemptDelivery, SharedActivityLease) {
    let d = f
        .domain
        .start_shared_budget_attempt(&f.owner.p, &f.policy, i, &m.gateway)
        .await
        .unwrap();
    let a = f
        .domain
        .record_shared_budget_attempt_process(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            evidence(&d, "qualified", 240),
        )
        .await
        .unwrap()
        .activity
        .unwrap();
    (d, a)
}
async fn queued(
    f: &Fixture,
    r: &ManagedAgentRequest,
    m: &MatrixFixture,
    a: &SharedActivityLease,
    id: &str,
) -> SharedWorkItem {
    let e = work::message(f, r, id).await;
    let now = {
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS now")
            .get_result::<Clock>(&mut db)
            .await
            .unwrap()
            .now
    };
    f.domain
        .route_shared_event(&f.policy, &e, now, &m.gateway)
        .await
        .unwrap();
    f.domain
        .poll_shared_budget_work(&f.owner.p, &f.policy, a.authority.clone(), &m.gateway)
        .await
        .unwrap()
        .unwrap()
}
fn input(a: &SharedActivityLease, w: &SharedWorkItem) -> SharedWorkInput {
    SharedWorkInput {
        authority: a.authority.clone(),
        work_id: w.id.clone(),
        work_digest: allocation_digest(w).unwrap(),
    }
}
async fn usage(
    f: &Fixture,
    d: &SharedBudgetAttemptDelivery,
    w: &SharedWorkItem,
    revision: i64,
    consumed: i64,
    state: &str,
) -> SharedWorkResultUpload {
    let t = f
        .domain
        .recover_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            SharedWorkRecoverInput {
                runtime_command_digest: d.command_digest.clone(),
                work_digest: allocation_digest(w).unwrap(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    let body = (state == "completed").then(|| "Original approved answer".to_string());
    SharedWorkResultUpload {
        work_digest: t.work_digest.clone(),
        token: t.token,
        receipt: SharedWorkReceipt {
            version: 1,
            observation_id: format!("{}_usage_{revision}", w.id),
            work_id: w.id.clone(),
            work_digest: t.work_digest,
            execution_id: w.execution_id.clone(),
            call_id: format!("call_{}", w.id),
            runtime_command_digest: d.command_digest.clone(),
            process_digest: "d".repeat(64),
            ledger_revision: revision,
            state: state.into(),
            known_consumed_tokens: consumed,
            retained_tokens: Some(consumed),
            usage_complete: true,
            body_digest: body.as_ref().map(|v| crate::hash(v)),
            body,
        },
    }
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_original_assignment_applied_head_replay_and_old_routes_refuse() {
    let (f, r, _admin, m, i) = Box::pin(fixture()).await;
    let original = json!(i);
    let (d, a) = started(&f, &m, i.clone()).await;
    assert_eq!(d.command.execution_budget.effective_ceiling_tokens, 125);
    assert_eq!(
        d.command
            .original_runtime_command
            .reserve_command
            .requested_tokens,
        100
    );
    let replay = f
        .domain
        .start_shared_budget_attempt(&f.owner.p, &f.policy, i, &m.gateway)
        .await
        .unwrap();
    assert!(!replay.execution_allowed);
    assert_eq!(replay.command_digest, d.command_digest);
    assert_eq!(original["assignmentId"], d.command.assignment_id);
    assert!(
        f.domain
            .poll_shared_work(&f.owner.p, &f.policy, a.authority.clone(), &m.gateway)
            .await
            .is_err()
    );
    let w = queued(&f, &r, &m, &a, "v3_first").await;
    assert!(
        f.domain
            .shared_work_input_mode(
                &f.owner.p,
                &w.id,
                crate::management_shared_context_types::SharedWorkEvidenceInput {
                    runtime_command_digest: d.command_digest.clone(),
                    work_digest: allocation_digest(&w).unwrap()
                }
            )
            .await
            .is_err()
    );
    f.domain
        .acknowledge_shared_budget_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .start_shared_budget_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
    assert!(
        !f.domain
            .start_shared_budget_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_head_change_produces_permanent_exact_no_start_fence() {
    let (f, r, admin, m, i) = Box::pin(fixture()).await;
    let (d, a) = started(&f, &m, i).await;
    let w = queued(&f, &r, &m, &a, "v3_unissued").await;
    f.domain
        .acknowledge_shared_budget_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    let proof = SharedBudgetWorkRefusalInput {
        runtime_command_digest: d.command_digest.clone(),
        work_digest: allocation_digest(&w).unwrap(),
        input_digest: w.body_digest.clone(),
    };
    assert!(
        f.domain
            .refuse_unissued_shared_budget_work(&f.owner.p, &w.id, proof.clone())
            .await
            .is_err()
    );
    let newer = apply_delta(&f, &r, &admin, 10, 250).await;
    assert!(
        f.domain
            .start_shared_budget_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .is_err()
    );
    let no = f
        .domain
        .refuse_unissued_shared_budget_work(&f.owner.p, &w.id, proof.clone())
        .await
        .unwrap();
    assert_eq!(no.refusal.current_budget_digest, newer.budget_digest);
    assert_eq!(
        json!(no),
        json!(
            f.domain
                .refuse_unissued_shared_budget_work(&f.owner.p, &w.id, proof.clone())
                .await
                .unwrap()
        )
    );
    let got = f
        .domain
        .recover_shared_budget_work_refusal(&f.owner.p, &w.id, proof.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(json!(got), json!(no));
    let mut wrong = proof;
    wrong.input_digest = "f".repeat(64);
    assert!(
        f.domain
            .recover_shared_budget_work_refusal(&f.owner.p, &w.id, wrong)
            .await
            .is_err()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("UPDATE hagency_agent_v1.management_shared_work SET state='started',started_at_ms=1,start_epoch=1 WHERE id=$1").bind::<Text,_>(&w.id).execute(&mut db).await.is_err());
    let mut zero = usage(&f, &d, &w, 260, 0, "failed").await;
    zero.receipt.known_consumed_tokens = 1;
    assert!(
        f.domain
            .record_shared_work_result(&f.owner.p, &f.policy, &w.id, zero.clone())
            .await
            .is_err()
    );
    zero.receipt.known_consumed_tokens = 0;
    let settled = f
        .domain
        .record_shared_work_result(&f.owner.p, &f.policy, &w.id, zero)
        .await
        .unwrap();
    assert_eq!(settled.state, "failed");
    assert!(settled.reply_state.is_none());
}

#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_later_head_preserves_issued_usage_reply_and_stop() {
    let (f, r, admin, m, i) = Box::pin(fixture()).await;
    let (d, a) = started(&f, &m, i).await;
    let w = queued(&f, &r, &m, &a, "v3_already_issued").await;
    f.domain
        .acknowledge_shared_budget_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    f.domain
        .start_shared_budget_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    apply_delta(&f, &r, &admin, 10, 250).await;
    let status = f
        .domain
        .shared_execution_status(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.state, "qualified");
    assert!(!status.work_available);
    assert!(
        f.domain
            .refuse_unissued_shared_budget_work(
                &f.owner.p,
                &w.id,
                SharedBudgetWorkRefusalInput {
                    runtime_command_digest: d.command_digest.clone(),
                    work_digest: allocation_digest(&w).unwrap(),
                    input_digest: w.body_digest.clone(),
                }
            )
            .await
            .is_err()
    );
    let renewed = f
        .domain
        .renew_shared_budget_activity(
            &f.owner.p,
            &f.policy,
            SharedActivityRenewInput {
                authority: a.authority,
                renewal_id: "v3_reply_after_delta".into(),
            },
            &m.gateway,
        )
        .await
        .unwrap();
    assert!(renewed.expires_at_ms > 0);
    let receipt = usage(&f, &d, &w, 260, 137, "completed").await;
    let ack = f
        .domain
        .record_shared_work_result(&f.owner.p, &f.policy, &w.id, receipt)
        .await
        .unwrap();
    assert_eq!(ack.reply_state.as_deref(), Some("pending"));
    let sender = work::sender().await;
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &sender.matrix, &mut String::new())
        .await
        .unwrap();
    assert_eq!(sender.calls.lock().unwrap().len(), 1);
    let old = f
        .domain
        .agent_budget(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap();
    assert_eq!(
        (old.effective_ceiling_tokens, old.known_consumed_tokens),
        (135, 137)
    );
    f.domain
        .record_shared_budget_attempt_process(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            evidence(&d, "stopped", 270),
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_fresh_owner_generation_head_and_disabled_guards() {
    let (f, r, admin, m, i) = Box::pin(fixture()).await;
    for actor in [&f.outsider, &admin, &f.coordinator, &f.admin] {
        assert!(
            f.domain
                .start_shared_budget_attempt(&actor.p, &f.policy, i.clone(), &m.gateway)
                .await
                .is_err()
        );
    }
    let mut wrong = f
        .auth
        .authenticate(&f.owner.token, now_ms(), true)
        .await
        .unwrap();
    wrong.device_generation = wrong.device_generation.map(|v| v + 1);
    assert!(
        f.domain
            .start_shared_budget_attempt(&wrong, &f.policy, i.clone(), &m.gateway)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .start_shared_budget_attempt(
                &f.owner.p,
                &f.policy.clone().with_shared_budget_execution(false),
                i.clone(),
                &m.gateway
            )
            .await
            .is_err()
    );
    let mut changed = i.clone();
    changed.execution_budget.applied_receipt_digest = "f".repeat(64);
    assert!(
        f.domain
            .start_shared_budget_attempt(&f.owner.p, &f.policy, changed, &m.gateway)
            .await
            .is_err()
    );
    apply_delta(&f, &r, &admin, 10, 250).await;
    assert!(
        f.domain
            .start_shared_budget_attempt(&f.owner.p, &f.policy, i, &m.gateway)
            .await
            .is_err()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert_eq!(
        sql_query(
            "SELECT count(*) AS count FROM hagency_agent_v1.management_shared_budget_attempts x JOIN hagency_agent_v1.management_shared_assignments a ON a.id=x.assignment_id WHERE a.request_id=$1"
        )
        .bind::<Text, _>(&r.id)
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count,
        0
    );
}

#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_context_exact_input_old_routes_and_http_recovery() {
    use crate::management_shared_context_types::*;
    let (mut f, r, _admin, m, i) = Box::pin(fixture()).await;
    f.policy = f.policy.with_shared_thread_context(true);
    let (d, a) = started(&f, &m, i).await;
    let w = queued(&f, &r, &m, &a, "v3_context").await;
    f.domain
        .acknowledge_shared_budget_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    let c = f
        .domain
        .freeze_shared_budget_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            SharedContextInput {
                authority: a.authority.clone(),
                work_digest: allocation_digest(&w).unwrap(),
            },
            &m.gateway,
        )
        .await
        .unwrap();
    assert!(c.context.history.is_empty());
    let svc = service(&f, true);
    let mode=post(&svc,&format!("shared-execution/budget-work/{}/input-mode",w.id),&f.owner.token,
        json!({"runtimeCommandDigest":d.command_digest,"workDigest":allocation_digest(&w).unwrap()}),StatusCode::OK).await;
    assert_eq!(mode["mode"], "thread_context_v1");
    assert!(mode["observedAtMs"].is_i64());
    assert!(
        f.domain
            .start_shared_budget_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .is_err()
    );
    let mut start = SharedContextStartInput {
        authority: a.authority.clone(),
        work_digest: allocation_digest(&w).unwrap(),
        context_digest: c.context_digest,
        input_digest: c.input_digest,
    };
    let good = start.clone();
    start.input_digest = "f".repeat(64);
    assert!(
        f.domain
            .start_shared_budget_context_work(&f.owner.p, &f.policy, &w.id, start, &m.gateway)
            .await
            .is_err()
    );
    assert!(
        f.domain
            .start_shared_budget_context_work(&f.owner.p, &f.policy, &w.id, good, &m.gateway)
            .await
            .unwrap()
            .start
            .newly_started
    );
    f.policy = f.policy.with_shared_budget_execution(false);
    let svc = service(&f, true);
    let recovered = post(
        &svc,
        &format!(
            "shared-execution/budget-attempts/{}/recover",
            d.command.command_id
        ),
        &f.owner.token,
        json!({"commandDigest":d.command_digest,"observationId":null}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(recovered["delivery"]["command"]["version"], 3);
    assert_eq!(recovered["delivery"]["executionAllowed"], false);
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_observe_first_poll_then_original_execute_promotes_once() {
    let (f, _r, _admin, m, i) = Box::pin(fixture()).await;
    let d = f
        .domain
        .start_shared_budget_attempt(&f.owner.p, &f.policy, i.clone(), &m.gateway)
        .await
        .unwrap();
    let recovered = f
        .domain
        .start_shared_budget_attempt(&f.owner.p, &f.policy, i, &m.gateway)
        .await
        .unwrap();
    let mut observed = evidence(&d, "qualified", 240);
    observed.token = recovered.token;
    let a = f
        .domain
        .record_shared_budget_attempt_process(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            observed,
        )
        .await
        .unwrap();
    assert_eq!(a.state, "issued");
    assert!(a.activity.is_none());
    let poll = f
        .domain
        .poll_shared_budget_attempt(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    assert!(!poll.execution_allowed);
    let a = f
        .domain
        .record_shared_budget_attempt_process(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            evidence(&d, "qualified", 240),
        )
        .await
        .unwrap();
    assert!(a.activity.is_some());
    let disabled = f.policy.clone().with_shared_budget_execution(false);
    let history = f
        .domain
        .recover_shared_budget_attempt(
            &f.owner.p,
            &disabled,
            &d.command.command_id,
            crate::management_agent_runtime_types::ManagedRuntimeRecoverInput {
                command_digest: d.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    let mut stopped = evidence(&d, "stopped", 241);
    stopped.token = history.token;
    f.domain
        .record_shared_budget_attempt_process(&f.owner.p, &disabled, &d.command.command_id, stopped)
        .await
        .unwrap();
    let old = f
        .domain
        .record_shared_budget_attempt_process(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            evidence(&d, "qualified", 240),
        )
        .await
        .unwrap();
    assert_eq!(old.state, "stopped");
    assert!(old.activity.is_none());
}

#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_original_block_reassessment_fifo_same_event_and_lost_start() {
    use crate::management_shared_demand_types::*;
    let (mut f, r, admin, a, m) = Box::pin(prepared()).await;
    f.policy = f
        .policy
        .with_shared_successors(true)
        .with_shared_durable_intake(true)
        .with_shared_demands(true)
        .with_agent_budget_deltas(true)
        .with_shared_budget_execution(true);
    intake_tests::arm(&f, &a, &m).await;
    let (original, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &original, 220).await;
    let e = work::message(&f, &r, "original_blocked_event").await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("SELECT pg_sleep(0.002)").await.unwrap();
    let now = sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS now")
        .get_result::<Clock>(&mut db)
        .await
        .unwrap()
        .now;
    f.domain
        .route_shared_event(&f.policy, &e, now, &m.gateway)
        .await
        .unwrap();
    let old = f
        .domain
        .poll_shared_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .demand
        .unwrap();
    let mut old_start = attempts::start(&original, &stopped);
    old_start.budget_observation.retained_tokens = 100;
    old_start.budget_observation.remaining_tokens = 0;
    let original_block_input = SharedDemandBlockInput {
        demand_digest: old.demand_digest.clone(),
        expected_control_revision: old.offer.control_revision,
        reason: "budget_blocked".into(),
        budget: Some(old_start.budget_observation.clone()),
    };
    let original_block_payload = serde_json::to_string(&original_block_input).unwrap();
    let original_block_digest = crate::hash(&original_block_payload);
    let canonical_block_digest = allocation_digest(&original_block_input).unwrap();
    assert_ne!(original_block_digest, canonical_block_digest,
        "v2 stored struct order must exercise the native canonical-proof mismatch");
    f.domain
        .block_shared_demand(
            &f.owner.p,
            &old.offer.demand_id,
            original_block_input,
        )
        .await
        .unwrap();
    // Recreate the deployed v25 validator around a retained v2 block. Upgrade
    // through DomainStore::open, including rollback and a second idempotent open.
    let legacy_schema = include_str!("management_shared_budget_schema.sql");
    let legacy_validator = legacy_schema
        .split("CREATE FUNCTION hagency_agent_v1.validate_shared_budget_demand()")
        .nth(1).unwrap()
        .split("CREATE TRIGGER validate_shared_budget_demand_head")
        .next().unwrap();
    db.batch_execute(&format!("CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_budget_demand(){legacy_validator}")).await.unwrap();
    db.batch_execute("DROP FUNCTION hagency_agent_v1.canonical_shared_block_json(jsonb); ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check; UPDATE hagency_agent_v1.management_deployment SET version=25; ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=25)").await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.canonical_shared_block_json(jsonb) RETURNS text LANGUAGE sql AS 'SELECT ''migration_collision''' ").await.unwrap();
    assert!(DomainStore::open(&f.url, "example.test", "_hagency_test_").await.is_err());
    #[derive(QueryableByName)]
    struct SchemaVersion { #[diesel(sql_type=BigInt)] version: i64 }
    assert_eq!(sql_query("SELECT version::bigint AS version FROM hagency_agent_v1.management_deployment").get_result::<SchemaVersion>(&mut db).await.unwrap().version, 25);
    db.batch_execute("DROP FUNCTION hagency_agent_v1.canonical_shared_block_json(jsonb)").await.unwrap();
    for _ in 0..2 {
        f.domain = DomainStore::open(&f.url, "example.test", "_hagency_test_").await.unwrap();
    }
    assert_eq!(sql_query("SELECT version::bigint AS version FROM hagency_agent_v1.management_deployment").get_result::<SchemaVersion>(&mut db).await.unwrap().version, 27);
    let pin = apply_delta(&f, &r, &admin, 25, 230).await;
    let offered = f
        .domain
        .poll_shared_budget_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .demand
        .unwrap();
    assert_eq!(json!(offered.offer), json!(old.offer));
    assert_eq!(
        offered.original_block.as_ref().unwrap().reason,
        "budget_blocked"
    );
    assert_eq!(offered.execution_budget, pin);
    assert_eq!(offered.original_block.as_ref().unwrap().payload_digest, canonical_block_digest,
        "the actual database projection must match native's canonical original-block proof");
    let mut budget = old_start.budget_observation;
    budget.observed_ledger_revision = 231;
    budget.allocated_tokens = 125;
    budget.remaining_tokens = 25;
    let input = SharedBudgetDemandStartInput {
        demand_digest: offered.demand_digest.clone(),
        expected_control_revision: offered.offer.control_revision,
        original_block: offered.original_block.clone(),
        start: SharedBudgetAttemptStartInput {
        web_search: false, tool_policy: None,
            start_intent_id: old_start.start_intent_id,
            assignment_id: a.id.clone(),
            predecessor: old_start.predecessor,
            execution_budget: pin,
            budget_observation: budget,
        },
    };
    let d = f
        .domain
        .start_shared_budget_demand(
            &f.owner.p,
            &f.policy,
            &offered.offer.demand_id,
            input.clone(),
            &m.gateway,
        )
        .await
        .unwrap();
    let disabled = f.policy.clone().with_shared_budget_execution(false);
    #[derive(QueryableByName)]
    struct RetainedOriginalBlock {
        #[diesel(sql_type=Text)]
        payload: String,
    }
    let retained = sql_query("SELECT payload FROM hagency_agent_v1.management_shared_demand_blocks WHERE demand_id=$1 AND digest=$2")
        .bind::<Text,_>(&old.offer.demand_id).bind::<Text,_>(&original_block_digest)
        .get_result::<RetainedOriginalBlock>(&mut db).await.unwrap();
    assert_eq!(retained.payload, original_block_payload, "reassessment retains the original bytes and storage digest");
    let replay = f
        .domain
        .start_shared_budget_demand(
            &f.owner.p,
            &disabled,
            &offered.offer.demand_id,
            input,
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(replay.command_digest, d.command_digest);
    assert!(!replay.execution_allowed);
    let activity = f
        .domain
        .record_shared_budget_attempt_process(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            evidence(&d, "qualified", 240),
        )
        .await
        .unwrap()
        .activity
        .unwrap();
    let work = f
        .domain
        .poll_shared_budget_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(work.event_id, e.event_id);
    assert_eq!(work.created_at_ms, now);
    let b = f
        .domain
        .shared_budget_demand_work_binding(
            &f.owner.p,
            &work.id,
            SharedDemandWorkInput {
                runtime_command_digest: d.command_digest,
                work_digest: allocation_digest(&work).unwrap(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.demand_digest, old.demand_digest);
    let s = f
        .domain
        .recover_shared_budget_demand(
            &f.owner.p,
            &old.offer.demand_id,
            SharedDemandRecoverInput {
                demand_digest: old.demand_digest,
            },
        )
        .await
        .unwrap();
    assert_eq!(s.state, "bound");
    let old_block=sql_query("SELECT count(*) AS count FROM hagency_agent_v1.management_shared_demand_blocks WHERE demand_id=$1").bind::<Text,_>(&old.offer.demand_id).get_result::<Count>(&mut db).await.unwrap();
    assert_eq!(old_block.count, 1);
}

async fn running(f: &mut Fixture, m: &MatrixFixture, id: &str) {
    f.policy = f
        .policy
        .clone()
        .with_shared_durable_intake(true)
        .with_shared_demands(true);
    f.domain
        .change_shared_run_control(
            &f.outsider.p,
            &f.policy,
            id,
            intake_tests::change("budget_run", 1, "running"),
            &m.gateway,
        )
        .await
        .unwrap();
}
async fn retain_event(f: &Fixture, r: &ManagedAgentRequest, m: &MatrixFixture, id: &str) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("SELECT pg_sleep(0.002)").await.unwrap();
    let now = sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS now")
        .get_result::<Clock>(&mut db)
        .await
        .unwrap()
        .now;
    f.domain
        .route_shared_event(&f.policy, &work::message(f, r, id).await, now, &m.gateway)
        .await
        .unwrap();
    now
}
async fn offered(f: &Fixture) -> SharedBudgetDemandStatus {
    f.domain
        .poll_shared_budget_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .demand
        .unwrap()
}
fn demand_input(
    d: &SharedBudgetDemandStatus,
    start: SharedBudgetAttemptStartInput,
) -> SharedBudgetDemandStartInput {
    SharedBudgetDemandStartInput {
        demand_digest: d.demand_digest.clone(),
        expected_control_revision: d.offer.control_revision,
        original_block: d.original_block.clone(),
        start,
    }
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_idle_promotion_fifo_never_creates_second_start() {
    use crate::management_shared_demand_types::*;
    let (mut f, r, _, m, i) = Box::pin(fixture()).await;
    running(&mut f, &m, &i.assignment_id).await;
    let (d, a) = started(&f, &m, i).await;
    let first = retain_event(&f, &r, &m, "budget_idle_first").await;
    retain_event(&f, &r, &m, "budget_idle_second").await;
    let offer = offered(&f).await;
    assert_eq!(offer.offer.event_id, "$budget_idle_first");
    for _ in 0..2 {
        let w = f
            .domain
            .poll_shared_budget_work(&f.owner.p, &f.policy, a.authority.clone(), &m.gateway)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (w.event_id.as_str(), w.created_at_ms),
            ("$budget_idle_first", first)
        );
        let b = f
            .domain
            .shared_budget_demand_work_binding(
                &f.owner.p,
                &w.id,
                SharedDemandWorkInput {
                    runtime_command_digest: d.command_digest.clone(),
                    work_digest: allocation_digest(&w).unwrap(),
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(b.demand_digest, offer.demand_digest);
    }
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert_eq!(
        sql_query(
            "SELECT count(*) AS count FROM hagency_agent_v1.management_shared_budget_demand_starts"
        )
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count,
        0
    );
    assert_eq!(sql_query("SELECT count(*) AS count FROM hagency_agent_v1.management_shared_work WHERE runtime_id=$1").bind::<Text,_>(&d.command.command_id).get_result::<Count>(&mut db).await.unwrap().count,1);
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_stop_run_aba_retains_original_event_and_rejects_old_offer() {
    use crate::management_shared_demand_types::*;
    let (mut f, r, _, m, i) = Box::pin(fixture()).await;
    running(&mut f, &m, &i.assignment_id).await;
    retain_event(&f, &r, &m, "budget_before_stop").await;
    let old = offered(&f).await;
    let original = demand_input(&old, i.clone());
    let paused = f
        .domain
        .change_shared_run_control(
            &f.outsider.p,
            &f.policy,
            &i.assignment_id,
            intake_tests::change("budget_stop", 2, "paused"),
            &m.gateway,
        )
        .await
        .unwrap();
    f.domain
        .change_shared_run_control(
            &f.outsider.p,
            &f.policy,
            &i.assignment_id,
            intake_tests::change("budget_run_again", paused.applied_revision, "running"),
            &m.gateway,
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_budget_demand(
                &f.owner.p,
                &f.policy,
                &old.offer.demand_id,
                original,
                &m.gateway
            )
            .await
            .is_err()
    );
    let history = f
        .domain
        .recover_shared_budget_demand(
            &f.owner.p,
            &old.offer.demand_id,
            SharedDemandRecoverInput {
                demand_digest: old.demand_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(history.state, "cancelled");
    assert_eq!(history.offer.event_id, old.offer.event_id);
    assert!(
        f.domain
            .poll_shared_budget_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .demand
            .is_none()
    );
    retain_event(&f, &r, &m, "budget_after_run").await;
    let next = offered(&f).await;
    assert_ne!(next.offer.demand_id, old.offer.demand_id);
    assert_eq!(next.offer.control_revision, 4);
    f.domain
        .start_shared_budget_demand(
            &f.owner.p,
            &f.policy,
            &next.offer.demand_id,
            demand_input(&next, i),
            &m.gateway,
        )
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_unrelated_head_advance_retains_terminal_original_offer() {
    use crate::management_shared_demand_types::*;
    let (mut f, r, _, m, i) = Box::pin(fixture()).await;
    running(&mut f, &m, &i.assignment_id).await;
    retain_event(&f, &r, &m, "budget_original_head").await;
    let old = offered(&f).await;
    let mut independent = i.clone();
    independent.start_intent_id = "manual_new_head".into();
    let (d, _) = started(&f, &m, independent).await;
    f.domain
        .record_shared_budget_attempt_process(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            evidence(&d, "stopped", 250),
        )
        .await
        .unwrap();
    assert!(
        f.domain
            .poll_shared_budget_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .demand
            .is_none()
    );
    let saved = f
        .domain
        .recover_shared_budget_demand(
            &f.owner.p,
            &old.offer.demand_id,
            SharedDemandRecoverInput {
                demand_digest: old.demand_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.state, "head_changed");
    assert_eq!(json!(saved.offer), json!(old.offer));
    assert!(
        f.domain
            .start_shared_budget_demand(
                &f.owner.p,
                &f.policy,
                &old.offer.demand_id,
                demand_input(&old, i),
                &m.gateway
            )
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_schema19_atomic_upgrade_reopen_preserves_original_receipts() {
    let (f, r, _, m, i) = Box::pin(fixture()).await;
    let before = json!(
        f.domain
            .agent_budget(&f.outsider.p, &f.policy, &r.id)
            .await
            .unwrap()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(include_str!("management_pool_agents_remove_test.sql")).await.unwrap();
    db.batch_execute(include_str!("management_project_manager_remove_test.sql")).await.unwrap();
    db.batch_execute(include_str!("management_shared_budget_remove_test.sql"))
        .await
        .unwrap();
    db.batch_execute("CREATE TABLE hagency_agent_v1.management_shared_work_refusals(dummy text)")
        .await
        .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert_eq!(
        sql_query("SELECT version::bigint AS count FROM hagency_agent_v1.management_deployment")
            .get_result::<Count>(&mut db)
            .await
            .unwrap()
            .count,
        19
    );
    db.batch_execute("DROP TABLE hagency_agent_v1.management_shared_work_refusals")
        .await
        .unwrap();
    let domain = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    assert_eq!(
        before,
        json!(
            domain
                .agent_budget(&f.outsider.p, &f.policy, &r.id)
                .await
                .unwrap()
        )
    );
    let d = domain
        .start_shared_budget_attempt(&f.owner.p, &f.policy, i.clone(), &m.gateway)
        .await
        .unwrap();
    let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    assert_eq!(
        d.command_digest,
        reopened
            .start_shared_budget_attempt(&f.owner.p, &f.policy, i, &m.gateway)
            .await
            .unwrap()
            .command_digest
    );
    assert!(
        db.batch_execute("DELETE FROM hagency_agent_v1.management_shared_budget_attempts")
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn budget_execution_v3_unstarted_receipt_advances_head_without_process_or_work() {
    let(f,_r,_admin,m,mut i)=Box::pin(fixture()).await;
    let d=f.domain.start_shared_budget_attempt(&f.owner.p,&f.policy,i.clone(),&m.gateway).await.unwrap();
    let mut receipt=evidence(&d,"not_started",240);receipt.receipt.process_digest=None;
    assert!(f.domain.record_shared_budget_attempt_process(&f.owner.p,&f.policy,&d.command.command_id,receipt.clone()).await.is_err(),"must first be permanently fenced");
    let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET permanent_fence=true,activity_until_ms=0 WHERE id=$1").bind::<Text,_>(&d.command.command_id).execute(&mut db).await.unwrap();
    let cleanup=f.domain.poll_shared_budget_attempt(&f.owner.p,&f.policy,ExecutorPollInput::default()).await.unwrap().unwrap();
    assert!(cleanup.cleanup_allowed);assert!(!cleanup.execution_allowed);receipt.token=cleanup.token;
    let ack=f.domain.record_shared_budget_attempt_process(&f.owner.p,&f.policy,&d.command.command_id,receipt.clone()).await.unwrap();
    assert_eq!(ack.state,"stopped");assert!(ack.activity.is_none());
    assert_eq!(f.domain.record_shared_budget_attempt_process(&f.owner.p,&f.policy,&d.command.command_id,receipt.clone()).await.unwrap().receipt_digest,ack.receipt_digest);
    let mut fabricated=evidence(&d,"qualified",241);fabricated.token=receipt.token.clone();
    assert!(f.domain.record_shared_budget_attempt_process(&f.owner.p,&f.policy,&d.command.command_id,fabricated).await.is_err());
    i.start_intent_id="after_unstarted".into();i.predecessor.command_id=d.command.command_id.clone();i.predecessor.command_digest=d.command_digest.clone();i.predecessor.process_attempt=d.command.process_attempt;i.predecessor.process_digest=String::new();i.predecessor.stopped_receipt_digest=ack.receipt_digest;i.budget_observation.observed_ledger_revision=241;
    let next=f.domain.start_shared_budget_attempt(&f.owner.p,&f.policy,i,&m.gateway).await.unwrap();
    assert_eq!(next.command.process_attempt,d.command.process_attempt+1);assert!(next.execution_allowed);
    let q=f.domain.record_shared_budget_attempt_process(&f.owner.p,&f.policy,&next.command.command_id,evidence(&next,"qualified",250)).await.unwrap();assert!(q.activity.is_some());
    sql_query("UPDATE hagency_agent_v1.management_shared_runtimes SET permanent_fence=true,activity_until_ms=0 WHERE id=$1").bind::<Text,_>(&next.command.command_id).execute(&mut db).await.unwrap();
    let mut invalid=evidence(&next,"not_started",251);invalid.receipt.process_digest=None;
    assert!(f.domain.record_shared_budget_attempt_process(&f.owner.p,&f.policy,&next.command.command_id,invalid).await.is_err(),"a process that acquired activity cannot become never-started");
    let historical=f.domain.recover_shared_budget_attempt(&f.owner.p,&f.policy,&d.command.command_id,crate::management_agent_runtime_types::ManagedRuntimeRecoverInput{command_digest:d.command_digest,observation_id:None}).await.unwrap();
    assert_eq!(historical.recorded.unwrap().state,"not_started");assert!(!historical.execution_allowed);assert!(!historical.cleanup_allowed);
}

#[tokio::test]
#[ignore="dedicated PostgreSQL"]
async fn budget_execution_v3_long_reply_http_recovery_and_matrix_delivery(){
    for (index,body) in ["研".repeat(6000)+"Offline answer.","x".repeat(32*1024),"\n".repeat(20*1024),"## 调研结果\n\n**结论**\n\n| GPU | 显存 |\n|---|---|\n| RTX | 96GB |\n\n[来源](https://example.org)\n\n".to_owned()+&"正文\n\n".repeat(2000)].into_iter().enumerate() {
        let(f,r,_admin,m,i)=Box::pin(fixture()).await;
        let(d,a)=started(&f,&m,i).await;
        let w=queued(&f,&r,&m,&a,&format!("long_answer_{index}")).await;
        f.domain.acknowledge_shared_budget_work(&f.owner.p,&f.policy,input(&a,&w),&m.gateway).await.unwrap();
        f.domain.start_shared_budget_work(&f.owner.p,&f.policy,input(&a,&w),&m.gateway).await.unwrap();
        let mut upload=usage(&f,&d,&w,260,37,"completed").await;
        upload.receipt.body=Some(body.clone());upload.receipt.body_digest=Some(crate::hash(&body));
        let svc=service(&f,true);
        let ack=post(&svc,&format!("shared-execution/work/{}/result",w.id),&f.owner.token,json!(upload),StatusCode::OK).await;
        assert_eq!(ack["replyState"],"pending");
        let recovered=post(&svc,&format!("shared-execution/work/{}/recover",w.id),&f.owner.token,json!({"runtimeCommandDigest":d.command_digest,"workDigest":allocation_digest(&w).unwrap()}),StatusCode::OK).await;
        assert_eq!(recovered["ticket"]["recorded"]["body"],body);
        let sender=work::sender().await;
        f.domain.send_shared_replies(&f.policy,&m.gateway,&sender.matrix,&mut String::new()).await.unwrap();
        let calls=sender.calls.lock().unwrap();
        assert_eq!(calls.len(),1);
        assert_eq!(calls[0].1["body"],body);
        if index==3 {
            assert_eq!(calls[0].1["format"],"org.matrix.custom.html");
            let rich=calls[0].1["formatted_body"].as_str().unwrap();
            assert!(rich.contains("<h2>") && rich.contains("<table>") && rich.contains("<strong>"));
            assert_eq!(calls[0].1["m.relates_to"]["event_id"],w.thread_root);
        }
    }
}
