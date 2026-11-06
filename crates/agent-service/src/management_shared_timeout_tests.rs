//! Uncertain v2 shared execution is time-bounded. A lost start or an unknown
//! process holds its Agent only for the grace period; then the chain and the
//! queue continue and the interrupted event is never replayed.
use super::*;
use crate::management_shared_attempt_types::*;

async fn db_now(f: &Fixture) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS count")
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}
// Result-ticket deletion is protected by PostgreSQL's real clock, so lapsed
// one-shot leases are awaited for real before any test clock moves ahead.
async fn wait_until(f: &Fixture, at: i64) {
    loop {
        let now = db_now(f).await;
        if now > at {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis((at - now + 20) as u64)).await;
    }
}
fn conflict<T>(r: crate::Result<T>, code: &str) -> bool {
    matches!(r, Err(crate::Error::Conflict(c)) if c == code)
}
async fn record(
    f: &Fixture,
    d: &SharedAttemptDelivery,
    v: SharedAttemptResultInput,
) -> crate::Result<SharedAttemptAcknowledgement> {
    f.domain
        .record_shared_attempt_process(&f.owner.p, &f.policy, &d.command.command_id, v)
        .await
}
async fn attempt_poll(f: &Fixture) -> SharedAttemptDelivery {
    f.domain
        .poll_shared_attempt(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap()
}
// The poll that ends the interrupted demand offers nothing for that Agent.
async fn next_offer(f: &Fixture) -> SharedDemandStatus {
    for _ in 0..3 {
        if let Some(d) = f
            .domain
            .poll_shared_demand(&f.owner.p, &f.policy, SharedDemandPollInput { after: None, retries: true })
            .await
            .unwrap()
            .demand
        {
            return d;
        }
    }
    panic!("the next retained event is offered");
}
fn after(
    o: &SharedDemandStatus,
    d: &SharedAttemptDelivery,
    stop: &str,
    process: &str,
    observed: i64,
) -> SharedDemandStartInput {
    SharedDemandStartInput {
        demand_digest: o.demand_digest.clone(),
        expected_control_revision: o.offer.control_revision,
        start: SharedAttemptStartInput {
            web_search: false,
            tool_policy: None,
            start_intent_id: format!("start_after_{}", d.command.command_id),
            assignment_id: d.command.assignment_id.clone(),
            predecessor: SharedAttemptPredecessor {
                command_id: d.command.command_id.clone(),
                command_digest: d.command_digest.clone(),
                process_attempt: d.command.process_attempt,
                stopped_receipt_digest: stop.into(),
                process_digest: process.into(),
            },
            budget_observation: SharedBudgetObservation {
                observed_ledger_revision: observed,
                ..d.command.budget_observation.clone()
            },
        },
        retry: None,
    }
}
async fn runtime_state(f: &Fixture, id: &str) -> (String, bool) {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type=Text)]
        state: String,
        #[diesel(sql_type=Bool)]
        permanent_fence: bool,
    }
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let r = sql_query("SELECT state,permanent_fence FROM hagency_agent_v1.management_shared_runtimes WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result::<Row>(&mut db)
        .await
        .unwrap();
    (r.state, r.permanent_fence)
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_v2_unclaimed_not_started_continues_chain_and_ends_lost_demand() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "unstarted_lost").await;
    retain(&f, &r, &m, "unstarted_next").await;
    let lost = demand(&f).await;
    assert_eq!(lost.offer.event_id, "$unstarted_lost");
    let issued = f
        .domain
        .start_shared_demand(&f.owner.p, &f.policy, &lost.offer.demand_id, start_input(&lost, &d, &stopped), &m.gateway)
        .await
        .unwrap();
    assert!(issued.execution_allowed);
    // The executor stalls past the claim window: nothing claims the attempt.
    let mut unstarted = attempts::result(&issued, "not_started", 210);
    unstarted.receipt.process_digest = None;
    assert!(conflict(record(&f, &issued, unstarted.clone()).await, "shared_attempt_not_unstarted"), "an execute ticket cannot retire a live window");
    wait_until(&f, issued.command.expires_at_ms).await;
    let cleanup = attempt_poll(&f).await;
    assert_eq!(cleanup.command.command_id, issued.command.command_id);
    assert!(cleanup.cleanup_allowed && !cleanup.execution_allowed);
    unstarted.token = cleanup.token.clone();
    let ack = record(&f, &issued, unstarted.clone()).await.unwrap();
    assert_eq!(ack.state, "stopped");
    assert!(ack.activity.is_none());
    assert_eq!(record(&f, &issued, unstarted.clone()).await.unwrap().receipt_digest, ack.receipt_digest);
    let mut fabricated = attempts::result(&issued, "qualified", 211);
    fabricated.token = cleanup.token.clone();
    assert!(conflict(record(&f, &issued, fabricated).await, "shared_process_cannot_restart"), "no process evidence after non-issuance");
    assert_eq!(runtime_state(&f, &issued.command.command_id).await, ("stopped".into(), true));
    // The lost event ends and the next one is offered on top of the unstarted attempt.
    let next = next_offer(&f).await;
    assert_eq!(next.offer.event_id, "$unstarted_next");
    assert_eq!(next.offer.predecessor_command_id, issued.command.command_id);
    assert_eq!(recovered(&f, &lost).await.state, "head_changed");
    let mut forged = after(&next, &issued, &ack.receipt_digest, &"d".repeat(64), 212);
    assert!(conflict(f.domain.start_shared_demand(&f.owner.p, &f.policy, &next.offer.demand_id, forged.clone(), &m.gateway).await, "shared_predecessor_not_stopped"), "no process identity is invented");
    forged.start.predecessor.process_digest.clear();
    let started = f
        .domain
        .start_shared_demand(&f.owner.p, &f.policy, &next.offer.demand_id, forged, &m.gateway)
        .await
        .unwrap();
    assert!(started.execution_allowed);
    assert_eq!(started.command.process_attempt, issued.command.process_attempt + 1);
    assert!(started.command.predecessor.process_digest.is_empty());
    assert_eq!(started.command.predecessor.stopped_receipt_digest, ack.receipt_digest);
    let activity = record(&f, &started, attempts::result(&started, "qualified", 220))
        .await
        .unwrap()
        .activity
        .unwrap();
    let w = f
        .domain
        .poll_shared_work(&f.owner.p, &f.policy, activity.authority, &m.gateway)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w.event_id, "$unstarted_next", "the interrupted event is never replayed");
    let historical = f
        .domain
        .recover_shared_attempt(
            &f.owner.p,
            &f.policy,
            &issued.command.command_id,
            crate::management_agent_runtime_types::ManagedRuntimeRecoverInput {
                command_digest: issued.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(historical.recorded.unwrap().state, "not_started");
    assert!(!historical.execution_allowed && !historical.cleanup_allowed);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_attempt_v2_uncertain_process_blocks_only_until_grace_then_server_stops_it() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "uncertain_lost").await;
    retain(&f, &r, &m, "uncertain_next").await;
    let lost = demand(&f).await;
    let issued = f
        .domain
        .start_shared_demand(&f.owner.p, &f.policy, &lost.offer.demand_id, start_input(&lost, &d, &stopped), &m.gateway)
        .await
        .unwrap();
    // Claimed and qualified, then silent: no stop, renewal or work evidence.
    let activity = record(&f, &issued, attempts::result(&issued, "qualified", 203))
        .await
        .unwrap()
        .activity
        .unwrap();
    wait_until(&f, issued.command.expires_at_ms.max(activity.expires_at_ms)).await;
    // The first activity lease ends within 30 s of the attempt window; the
    // server then allows a 3-minute grace (UNCERTAIN_GRACE_MS).
    assert!(activity.expires_at_ms <= issued.command.expires_at_ms + 30_000);
    let deadline = issued.command.expires_at_ms + 30_000 + 180_000;
    let mut timeout = attempts::result(&issued, "stopped", 205);
    timeout.receipt.reason = Some(UNCERTAIN_TIMEOUT.into());
    let premature = SharedAttemptStartInput {
        web_search: false,
        tool_policy: None,
        start_intent_id: "start_during_grace".into(),
        ..after(&lost, &issued, &allocation_digest(&timeout.receipt).unwrap(), &"d".repeat(64), 206).start
    };
    // Within the grace period nothing changes: fenced, uncertain, and no
    // second process may start. A cleanup lease does not end the bound.
    f.domain.set_management_test_clock(deadline - 1000).await.unwrap();
    let fenced = attempt_poll(&f).await;
    assert_eq!(fenced.command.command_id, issued.command.command_id);
    assert!(fenced.cleanup_allowed && !fenced.execution_allowed);
    timeout.token = fenced.token.clone();
    assert!(conflict(record(&f, &issued, timeout.clone()).await, "shared_attempt_timeout_pending"), "the time bound has not passed");
    assert_eq!(runtime_state(&f, &issued.command.command_id).await, ("qualified".into(), true));
    assert_eq!(recovered(&f, &lost).await.state, "start_committed");
    assert!(conflict(f.domain.start_shared_attempt(&f.owner.p, &f.policy, premature.clone(), &m.gateway).await, "shared_predecessor_not_stopped"));
    // After it, the server stops the attempt itself, without any executor poll.
    f.domain.set_management_test_clock(deadline + 1000).await.unwrap();
    for _ in 0..64 {
        f.domain.sweep_shared_attempt_timeouts().await.unwrap();
        if runtime_state(&f, &issued.command.command_id).await.0 == "stopped" {
            break;
        }
    }
    assert_eq!(runtime_state(&f, &issued.command.command_id).await, ("stopped".into(), true));
    assert_eq!(recovered(&f, &lost).await.state, "head_changed");
    assert!(conflict(f.domain.start_shared_attempt(&f.owner.p, &f.policy, premature, &m.gateway).await, "shared_predecessor_evidence_changed"), "a stop needs explicit evidence");
    // The executor supplies that evidence: the known process, reason uncertain_timeout.
    let awaiting = attempt_poll(&f).await;
    assert_eq!(awaiting.command.command_id, issued.command.command_id);
    assert!(awaiting.cleanup_allowed && !awaiting.execution_allowed);
    timeout.token = awaiting.token.clone();
    let ack = record(&f, &issued, timeout.clone()).await.unwrap();
    assert_eq!(ack.state, "stopped");
    assert_eq!(record(&f, &issued, timeout).await.unwrap().receipt_digest, ack.receipt_digest);
    let next = next_offer(&f).await;
    assert_eq!(next.offer.event_id, "$uncertain_next");
    let started = f
        .domain
        .start_shared_demand(&f.owner.p, &f.policy, &next.offer.demand_id, after(&next, &issued, &ack.receipt_digest, &"d".repeat(64), 206), &m.gateway)
        .await
        .unwrap();
    assert!(started.execution_allowed);
    assert_eq!(started.command.predecessor.process_digest, "d".repeat(64));
    let activity = record(&f, &started, attempts::result(&started, "qualified", 220))
        .await
        .unwrap()
        .activity
        .unwrap();
    let w = f
        .domain
        .poll_shared_work(&f.owner.p, &f.policy, activity.authority, &m.gateway)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w.event_id, "$uncertain_next", "the interrupted event is never replayed");
    f.domain.reset_management_test_clock().await.unwrap();
}

#[path = "management_shared_retry_tests.rs"]
mod retry_tests;
