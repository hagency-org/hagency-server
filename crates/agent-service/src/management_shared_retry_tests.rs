//! ADR-193 amendment: an event whose attempt provably never started runs again
//! in place, at most three times, and only on the executor's own evidence.
use super::*;

async fn retry(
    f: &Fixture,
    o: &SharedDemandStatus,
    attempt: &SharedAttemptDelivery,
    n: i64,
    result: SharedAttemptResultInput,
) -> crate::Result<SharedDemandRetryReply> {
    f.domain
        .retry_shared_demand(
            &f.owner.p,
            &f.policy,
            &o.offer.demand_id,
            SharedDemandRetryInput {
                demand_digest: o.demand_digest.clone(),
                expected_control_revision: o.offer.control_revision,
                retry: n,
                attempt_id: attempt.command.command_id.clone(),
                result,
            },
        )
        .await
}
// Issue with the attempt window already over in real time: its tickets lapse
// at once, so a cleanup ticket needs no 30 s wait. Only for attempts that
// are meant to end not_started.
async fn start_lapsed(
    f: &Fixture,
    m: &MatrixFixture,
    o: &SharedDemandStatus,
    input: SharedDemandStartInput,
) -> SharedAttemptDelivery {
    f.domain
        .set_management_test_clock(db_now(f).await - 31_000)
        .await
        .unwrap();
    let issued = f
        .domain
        .start_shared_demand(&f.owner.p, &f.policy, &o.offer.demand_id, input, &m.gateway)
        .await;
    f.domain.reset_management_test_clock().await.unwrap();
    let issued = issued.unwrap();
    assert!(issued.execution_allowed);
    issued
}
// The executor never claimed `issued`: its cleanup ticket and not_started evidence.
async fn unstarted(
    f: &Fixture,
    issued: &SharedAttemptDelivery,
    ledger: i64,
) -> SharedAttemptResultInput {
    let cleanup = attempt_poll(f).await;
    assert_eq!(cleanup.command.command_id, issued.command.command_id);
    assert!(cleanup.cleanup_allowed && !cleanup.execution_allowed);
    let mut v = attempts::result(issued, "not_started", ledger);
    v.receipt.process_digest = None;
    v.token = cleanup.token;
    v
}
fn retried(
    o: &SharedDemandStatus,
    previous: &SharedAttemptDelivery,
    stop: &str,
    observed: i64,
    n: i64,
) -> SharedDemandStartInput {
    let mut input = after(o, previous, stop, "", observed);
    input.start.start_intent_id = format!("sdr{n}_{}", o.offer.demand_id);
    input.retry = Some(n);
    input
}
async fn work_items(f: &Fixture, event: &str) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_shared_work WHERE event_id=$1")
        .bind::<Text, _>(event)
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_not_started_retries_same_event_once_in_place() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "retry_lost").await;
    retain(&f, &r, &m, "retry_next").await;
    let lost = demand(&f).await;
    assert_eq!(lost.offer.event_id, "$retry_lost");
    let issued = start_lapsed(&f, &m, &lost, start_input(&lost, &d, &stopped)).await;
    let evidence = unstarted(&f, &issued, 210).await;
    // Out of order or for another attempt: refused, nothing recorded.
    assert!(conflict(
        retry(&f, &lost, &issued, 2, evidence.clone()).await,
        "shared_demand_retry_changed"
    ));
    let reply = retry(&f, &lost, &issued, 1, evidence.clone())
        .await
        .unwrap();
    assert_eq!(reply.acknowledgement.state, "stopped");
    assert_eq!(
        (reply.demand.state.as_str(), reply.demand.retry),
        ("retry_offered", Some(1))
    );
    // A lost reply is replayed exactly.
    let again = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
    assert_eq!(
        again.acknowledgement.receipt_digest,
        reply.acknowledgement.receipt_digest
    );
    assert_eq!(again.demand.state, "retry_offered");
    // The retried event keeps its FIFO place: the next event is not offered.
    let offered = next_offer(&f).await;
    assert_eq!(offered.offer.demand_id, lost.offer.demand_id);
    assert_eq!(offered.state, "retry_offered");
    let mut original = start_input(&lost, &d, &stopped);
    original.start.start_intent_id = "replayed_original".into();
    assert!(
        f.domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &lost.offer.demand_id,
                original,
                &m.gateway
            )
            .await
            .is_err(),
        "the original start never runs again"
    );
    let input = retried(
        &offered,
        &issued,
        &reply.acknowledgement.receipt_digest,
        211,
        1,
    );
    let started = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &lost.offer.demand_id,
            input.clone(),
            &m.gateway,
        )
        .await
        .unwrap();
    assert!(started.execution_allowed);
    assert_eq!(
        started.command.process_attempt,
        issued.command.process_attempt + 1
    );
    assert!(started.command.predecessor.process_digest.is_empty());
    assert_eq!(
        started.command.predecessor.stopped_receipt_digest,
        reply.acknowledgement.receipt_digest
    );
    // A lost start reply never starts a second attempt.
    let replayed = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &lost.offer.demand_id,
            input,
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(replayed.command.command_id, started.command.command_id);
    assert!(!replayed.execution_allowed);
    assert_eq!(recovered(&f, &lost).await.retry, Some(1));
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
    assert_eq!(w.event_id, "$retry_lost", "the same event runs");
    assert_eq!(work_items(&f, "$retry_lost").await, 1, "exactly once");
    let bound = recovered(&f, &lost).await;
    assert_eq!(bound.state, "bound");
    assert_eq!(
        bound.binding.unwrap().runtime_command_id,
        started.command.command_id
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_time_bound_never_replays_but_later_not_started_retries_once() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "late_lost").await;
    retain(&f, &r, &m, "late_next").await;
    let lost = demand(&f).await;
    // Start accepted, reply lost, executor away past the time bound.
    let issued = start_lapsed(&f, &m, &lost, start_input(&lost, &d, &stopped)).await;
    f.domain
        .set_management_test_clock(issued.command.expires_at_ms + 30_000 + 180_000 + 1_000)
        .await
        .unwrap();
    for _ in 0..64 {
        if runtime_state(&f, &issued.command.command_id).await.0 == "stopped" {
            break;
        }
        let _ = f.domain.sweep_shared_attempt_timeouts().await;
    }
    assert_eq!(
        runtime_state(&f, &issued.command.command_id).await,
        ("stopped".into(), true)
    );
    // The server's own bound neither replays nor ends the event: it waits for
    // the executor's evidence and holds the FIFO place.
    let held = recovered(&f, &lost).await;
    assert_eq!((held.state.as_str(), held.retry), ("start_committed", None));
    assert_eq!(next_offer(&f).await.offer.demand_id, lost.offer.demand_id);
    assert_eq!(work_items(&f, "$late_lost").await, 0);
    // The executor returns and proves it never claimed the attempt.
    let evidence = unstarted(&f, &issued, 210).await;
    let reply = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
    assert_eq!(reply.demand.state, "retry_offered");
    let started = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &lost.offer.demand_id,
            retried(
                &reply.demand,
                &issued,
                &reply.acknowledgement.receipt_digest,
                211,
                1,
            ),
            &m.gateway,
        )
        .await
        .unwrap();
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
    assert_eq!(w.event_id, "$late_lost");
    assert_eq!(work_items(&f, "$late_lost").await, 1);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_unknowable_attempt_stays_terminal_and_is_never_retried() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "unknown_lost").await;
    retain(&f, &r, &m, "unknown_next").await;
    let lost = demand(&f).await;
    let issued = start_lapsed(&f, &m, &lost, start_input(&lost, &d, &stopped)).await;
    // Claimed: the executor reported a process before going silent.
    f.domain
        .set_management_test_clock(issued.command.issued_at_ms + 1_000)
        .await
        .unwrap();
    record(&f, &issued, attempts::result(&issued, "qualified", 210))
        .await
        .unwrap();
    f.domain
        .set_management_test_clock(issued.command.expires_at_ms + 30_000 + 180_000 + 1_000)
        .await
        .unwrap();
    for _ in 0..64 {
        if runtime_state(&f, &issued.command.command_id).await.0 == "stopped" {
            break;
        }
        let _ = f.domain.sweep_shared_attempt_timeouts().await;
    }
    assert_eq!(recovered(&f, &lost).await.state, "head_changed");
    let cleanup = attempt_poll(&f).await;
    assert_eq!(cleanup.command.command_id, issued.command.command_id);
    let mut forged = attempts::result(&issued, "not_started", 211);
    forged.receipt.process_digest = None;
    forged.token = cleanup.token;
    assert!(
        conflict(
            retry(&f, &lost, &issued, 1, forged).await,
            "shared_attempt_not_unstarted"
        ),
        "a process that may have run is never retried"
    );
    let held = recovered(&f, &lost).await;
    assert_eq!((held.state.as_str(), held.retry), ("head_changed", None));
    let input = retried(&held, &issued, &"0".repeat(64), 212, 1);
    assert!(
        f.domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &lost.offer.demand_id,
                input,
                &m.gateway
            )
            .await
            .is_err()
    );
    assert_eq!(work_items(&f, "$unknown_lost").await, 0, "never replayed");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_retry_cap_ends_the_event_and_the_queue_moves_on() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "cap_lost").await;
    retain(&f, &r, &m, "cap_next").await;
    let lost = demand(&f).await;
    let mut attempt = start_lapsed(&f, &m, &lost, start_input(&lost, &d, &stopped)).await;
    let mut ledger = 210;
    for n in 1..=MAX_DEMAND_RETRIES {
        let reply = retry(
            &f,
            &lost,
            &attempt,
            n,
            unstarted(&f, &attempt, ledger).await,
        )
        .await
        .unwrap();
        assert_eq!(
            (reply.demand.state.as_str(), reply.demand.retry),
            ("retry_offered", Some(n))
        );
        let input = retried(
            &reply.demand,
            &attempt,
            &reply.acknowledgement.receipt_digest,
            ledger + 1,
            n,
        );
        attempt = start_lapsed(&f, &m, &reply.demand, input).await;
        ledger += 10;
    }
    let evidence = unstarted(&f, &attempt, ledger).await;
    let reply = retry(&f, &lost, &attempt, MAX_DEMAND_RETRIES + 1, evidence)
        .await
        .unwrap();
    assert_eq!(reply.demand.state, "head_changed", "the cap ends the event");
    assert_eq!(work_items(&f, "$cap_lost").await, 0);
    let next = next_offer(&f).await;
    assert_eq!(next.offer.event_id, "$cap_next");
    assert_eq!(
        next.offer.predecessor_command_id,
        attempt.command.command_id
    );
    let started = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &next.offer.demand_id,
            after(
                &next,
                &attempt,
                &reply.acknowledgement.receipt_digest,
                "",
                ledger + 1,
            ),
            &m.gateway,
        )
        .await
        .unwrap();
    assert!(started.execution_allowed);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_stop_refuses_a_retry_and_ends_a_running_one() {
    for running in [false, true] {
        let (f, r, _admin, a, m) = Box::pin(setup()).await;
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        retain(&f, &r, &m, &format!("stop_lost_{running}")).await;
        let lost = demand(&f).await;
        let issued = start_lapsed(&f, &m, &lost, start_input(&lost, &d, &stopped)).await;
        let evidence = unstarted(&f, &issued, 210).await;
        let stop = || async {
            f.domain
                .change_shared_run_control(
                    &f.owner.p,
                    &f.policy,
                    &a.id,
                    intake_tests::change("retry_stop", 2, "paused"),
                    &m.gateway,
                )
                .await
                .unwrap();
        };
        if !running {
            stop().await;
            let reply = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
            assert_ne!(
                reply.demand.state, "retry_offered",
                "a stopped Run never reopens its event"
            );
            assert_eq!(reply.demand.retry, None);
            continue;
        }
        let reply = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
        let started = f
            .domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &lost.offer.demand_id,
                retried(
                    &reply.demand,
                    &issued,
                    &reply.acknowledgement.receipt_digest,
                    211,
                    1,
                ),
                &m.gateway,
            )
            .await
            .unwrap();
        assert!(started.execution_allowed);
        stop().await;
        let polled = attempt_poll(&f).await;
        assert_eq!(polled.command.command_id, started.command.command_id);
        assert!(
            polled.cleanup_allowed && !polled.execution_allowed,
            "the retried attempt loses its authority at once"
        );
        assert!(
            record(&f, &started, attempts::result(&started, "qualified", 220))
                .await
                .unwrap()
                .activity
                .is_none()
        );
        assert_eq!(work_items(&f, &format!("$stop_lost_{running}")).await, 0);
    }
}

async fn advertised(f: &Fixture, m: &MatrixFixture) -> bool {
    let mut response = TestClient::get("https://example.test/hagency/api/v1/discovery")
        .add_header("host", "example.test", true)
        .send(&svc(f, m.gateway.clone()))
        .await;
    let value = response.take_json::<Value>().await.unwrap();
    value["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "shared-demand-retry-v1")
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_retry_capability_follows_its_route_and_evidence_still_records() {
    let (mut f, r, _admin, _a, m) = Box::pin(setup()).await;
    assert!(advertised(&f, &m).await);
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "gate_lost").await;
    let lost = demand(&f).await;
    let issued = start_lapsed(&f, &m, &lost, start_input(&lost, &d, &stopped)).await;
    let evidence = unstarted(&f, &issued, 210).await;
    // Any one gate of the demand route off: no capability, no route.
    f.policy = f.policy.clone().with_project_reservations(false);
    assert!(!advertised(&f, &m).await);
    assert!(matches!(
        retry(&f, &lost, &issued, 1, evidence.clone()).await,
        Err(crate::Error::Unavailable("shared_demand_retry_unavailable"))
    ));
    // The plain result route still records the never-started evidence.
    assert_eq!(
        record(&f, &issued, evidence).await.unwrap().state,
        "stopped"
    );
    assert_eq!(
        runtime_state(&f, &issued.command.command_id).await,
        ("stopped".into(), true)
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_executor_without_retries_never_sees_them_and_ends_an_open_one() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "old_lost").await;
    retain(&f, &r, &m, "old_next").await;
    let lost = demand(&f).await;
    let issued = start_lapsed(&f, &m, &lost, start_input(&lost, &d, &stopped)).await;
    let reply = retry(&f, &lost, &issued, 1, unstarted(&f, &issued, 210).await)
        .await
        .unwrap();
    assert_eq!(reply.demand.state, "retry_offered");
    let capable = f
        .domain
        .poll_shared_demand(
            &f.owner.p,
            &f.policy,
            SharedDemandPollInput {
                after: None,
                retries: true,
            },
        )
        .await
        .unwrap()
        .demand
        .unwrap();
    assert_eq!(
        (capable.state.as_str(), capable.retry),
        ("retry_offered", Some(1))
    );
    // An executor without retry support (a downgrade) reads no retry fields;
    // the retry it cannot run ends the event as before and the queue moves on.
    let old = f
        .domain
        .recover_shared_demand(
            &f.owner.p,
            &lost.offer.demand_id,
            SharedDemandRecoverInput {
                demand_digest: lost.demand_digest.clone(),
                retries: false,
            },
        )
        .await
        .unwrap();
    assert_eq!((old.state.as_str(), old.retry), ("head_changed", None));
    let value = serde_json::to_value(&old).unwrap();
    assert!(
        value.get("retry").is_none(),
        "an old executor's strict status parser sees no new field"
    );
    let next = next_offer(&f).await;
    assert_eq!(next.offer.event_id, "$old_next");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_validator_owner_refuses_to_undo_a_later_change() {
    let (f, _r, _admin, _a, _m) = Box::pin(setup()).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // A later versioned migration changed the validator in place.
    db.batch_execute("CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_demand() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END; $$")
        .await
        .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err(),
        "startup refuses to silently undo it"
    );
    // Back to this file's own body: startup accepts and keeps it.
    let sql = include_str!("management_shared_demand_retry_schema.sql");
    let own = &sql[sql
        .find("CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_demand()")
        .unwrap()..];
    db.batch_execute(own).await.unwrap();
    DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
}
