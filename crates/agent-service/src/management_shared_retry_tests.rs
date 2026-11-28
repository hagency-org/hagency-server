//! ADR-193 amendment: an event whose attempt provably never started runs again
//! in place, at most three times, and only on the executor's own evidence.
use super::*;
#[path = "management_shared_status_tests.rs"]
mod status_tests;

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
// The same start, from an executor that takes messages back.
fn reoffering(mut input: SharedDemandStartInput) -> SharedDemandStartInput {
    input.reoffers = true;
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
    // A start that does not say its executor takes messages back.
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
    forged.token = cleanup.token.clone();
    assert!(
        conflict(
            retry(&f, &lost, &issued, 1, forged).await,
            "shared_attempt_not_unstarted"
        ),
        "a process that may have run is never retried"
    );
    // Nor on its stop: the start did not say that its executor takes
    // messages back.
    let mut evidence = attempts::result(&issued, "stopped", 212);
    evidence.receipt.reason = Some(UNCERTAIN_TIMEOUT.into());
    evidence.token = cleanup.token;
    assert!(matches!(
        retry(&f, &lost, &issued, 1, evidence).await,
        Err(crate::Error::Invalid("invalid_shared_demand_retry"))
    ));
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
    f.domain.reset_management_test_clock().await.unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_silent_claimed_attempt_waits_for_its_stop_and_takes_no_forged_one() {
    for asks in [false, true] {
        let (f, r, _admin, _a, m) = Box::pin(setup()).await;
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        retain(&f, &r, &m, &format!("unknown_lost_{asks}")).await;
        retain(&f, &r, &m, &format!("unknown_next_{asks}")).await;
        let lost = demand(&f).await;
        // Its executor takes messages back.
        let input = reoffering(start_input(&lost, &d, &stopped));
        let issued = start_lapsed(&f, &m, &lost, input).await;
        // Claimed: the executor reported a process before going silent. It
        // was never given the event.
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
        // The time bound retries nothing and ends nothing: the event waits
        // for the executor's evidence that the process stopped.
        let waiting = recovered(&f, &lost).await;
        assert_eq!(
            (waiting.state.as_str(), waiting.retry),
            ("start_committed", None)
        );
        let cleanup = attempt_poll(&f).await;
        assert_eq!(cleanup.command.command_id, issued.command.command_id);
        let mut forged = attempts::result(&issued, "not_started", 211);
        forged.receipt.process_digest = None;
        forged.token = cleanup.token.clone();
        assert!(
            conflict(
                retry(&f, &lost, &issued, 1, forged).await,
                "shared_attempt_not_unstarted"
            ),
            "a process that reported is never called unstarted"
        );
        let mut evidence = attempts::result(&issued, "stopped", 212);
        evidence.receipt.reason = Some(UNCERTAIN_TIMEOUT.into());
        evidence.token = cleanup.token;
        if asks {
            // No turn began for the event in that process: the server never
            // gave it the event. The executor asks, and the event runs again.
            let reply = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
            assert_eq!(
                (reply.demand.state.as_str(), reply.demand.retry),
                ("retry_offered", Some(1))
            );
            assert_eq!(
                next_offer(&f).await.offer.event_id,
                format!("$unknown_lost_{asks}")
            );
            f.domain.reset_management_test_clock().await.unwrap();
            continue;
        }
        // An executor that only reports the stop ends the event, as before.
        record(&f, &issued, evidence).await.unwrap();
        assert_eq!(
            next_offer(&f).await.offer.event_id,
            format!("$unknown_next_{asks}")
        );
        let held = recovered(&f, &lost).await;
        assert_eq!((held.state.as_str(), held.retry), ("head_changed", None));
        let input = retried(&held, &issued, &"0".repeat(64), 213, 1);
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
        assert_eq!(
            work_items(&f, &format!("$unknown_lost_{asks}")).await,
            0,
            "never replayed"
        );
        f.domain.reset_management_test_clock().await.unwrap();
    }
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
    // Its sender reads which record ended it and may send it again; the
    // resource's owner, who did not send it, may not.
    let room = work::message(&f, &r, "scope").await.room_id;
    for (reader, again) in [(&f.outsider.p, true), (&f.owner.p, false)] {
        assert_eq!(
            status_of(&f, &room, reader, "$cap_lost").await,
            json!(["blocked", "head_changed", "retry_exhausted", again])
        );
    }
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
                reoffers: true,
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
                reoffers: false,
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
    let sql = include_str!(
        "../migrations/domain/2026-11-19-000146_management_shared_demand_retry/up.sql"
    );
    let own = &sql[sql
        .find("CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_demand()")
        .unwrap()..];
    db.batch_execute(own).await.unwrap();
    DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
}

// Time-limit rule 3: the line between what is redone and what is never
// guessed is the start of a turn. A process that was given a message and
// ended before the turn began leaves a message that runs in the next process.
async fn released(f: &Fixture, work: &str) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_shared_demand_released_bindings WHERE work_id=$1")
        .bind::<Text, _>(work)
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}
// Attempt 0 of the oldest waiting event: its process starts, reports, is
// given the event and acknowledges it. No turn has begun. `reoffers`: the
// start says that its executor takes messages back.
async fn given(
    f: &Fixture,
    m: &MatrixFixture,
    d: &SharedRuntimeDelivery,
    stopped: &SharedProcessReceipt,
    reoffers: bool,
) -> (
    SharedDemandStatus,
    SharedAttemptDelivery,
    SharedWorkItem,
    SharedWorkInput,
) {
    let lost = demand(f).await;
    let mut input = start_input(&lost, d, stopped);
    input.reoffers = reoffers;
    let issued = f
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
    let activity = record(f, &issued, attempts::result(&issued, "qualified", 210))
        .await
        .unwrap()
        .activity
        .unwrap();
    let work = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered(f, &lost).await.state, "bound");
    let input = SharedWorkInput {
        authority: activity.authority,
        work_id: work.id.clone(),
        work_digest: allocation_digest(&work).unwrap(),
    };
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    (lost, issued, work, input)
}
// The retry start that names a process which ran and stopped, from an
// executor that takes messages back.
fn after_stopped(
    o: &SharedDemandStatus,
    previous: &SharedAttemptDelivery,
    stop: &str,
    observed: i64,
    n: i64,
) -> SharedDemandStartInput {
    let mut input = after(o, previous, stop, &"d".repeat(64), observed);
    input.start.start_intent_id = format!("sdr{n}_{}", o.offer.demand_id);
    input.retry = Some(n);
    input.reoffers = true;
    input
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_work_that_never_began_runs_in_the_next_process() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "unbegun_first").await;
    retain(&f, &r, &m, "unbegun_next").await;
    let (lost, issued, first, input) = given(&f, &m, &d, &stopped, true).await;
    assert_eq!(first.event_id, "$unbegun_first");
    // The process ends before the turn began. The executor reports the stop
    // and asks for the event to run again.
    let evidence = attempts::result(&issued, "stopped", 220);
    let reply = retry(&f, &lost, &issued, 1, evidence.clone())
        .await
        .unwrap();
    assert_eq!(reply.acknowledgement.state, "stopped");
    assert_eq!(
        (
            reply.demand.state.as_str(),
            reply.demand.retry,
            reply.demand.binding.is_none()
        ),
        ("retry_offered", Some(1), true)
    );
    assert_eq!(released(&f, &first.id).await, 1);
    // The proof stands: the work that was given out can never start.
    assert!(
        f.domain
            .start_current_work(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .is_err()
    );
    // A lost reply is replayed exactly.
    let again = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
    assert_eq!(again.demand.state, "retry_offered");
    // The event keeps its place: the next one is not offered before it.
    let offered = next_offer(&f).await;
    assert_eq!(
        (offered.offer.demand_id.as_str(), offered.state.as_str()),
        (lost.offer.demand_id.as_str(), "retry_offered")
    );
    let started = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &lost.offer.demand_id,
            after_stopped(
                &offered,
                &issued,
                &reply.acknowledgement.receipt_digest,
                221,
                1,
            ),
            &m.gateway,
        )
        .await
        .unwrap();
    assert!(started.execution_allowed);
    let activity = record(&f, &started, attempts::result(&started, "qualified", 230))
        .await
        .unwrap()
        .activity
        .unwrap();
    let second = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.event_id, "$unbegun_first", "the same event runs");
    assert_ne!(second.id, first.id);
    assert_ne!(second.execution_id, first.execution_id);
    assert_eq!(second.body, first.body);
    assert_eq!(work_items(&f, "$unbegun_first").await, 2);
    let bound = recovered(&f, &lost).await;
    assert_eq!(bound.state, "bound");
    let binding = bound.binding.unwrap();
    assert_eq!(
        (binding.runtime_command_id, binding.work_id),
        (started.command.command_id.clone(), second.id.clone())
    );
    // This time the turn begins, and from here nothing is redone: when this
    // process ends, a retry that is asked for changes nothing.
    let input = SharedWorkInput {
        authority: activity.authority,
        work_id: second.id.clone(),
        work_digest: allocation_digest(&second).unwrap(),
    };
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    f.domain
        .start_current_work(&f.owner.p, &f.policy, input, &m.gateway)
        .await
        .unwrap();
    let ended = retry(
        &f,
        &lost,
        &started,
        2,
        attempts::result(&started, "stopped", 240),
    )
    .await
    .unwrap();
    assert_eq!(
        (ended.demand.state.as_str(), ended.demand.retry),
        ("bound", Some(1))
    );
    assert_eq!(released(&f, &second.id).await, 0);
    assert_eq!(work_items(&f, "$unbegun_first").await, 2);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_released_work_is_a_waiting_event_again_for_a_stop_and_a_plain_report() {
    for stop in [true, false] {
        let (f, r, _admin, a, m) = Box::pin(setup()).await;
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        retain(&f, &r, &m, &format!("released_{stop}")).await;
        let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
        let event = format!("$released_{stop}");
        // The executor reports on the plain result route that the process
        // ended before its turn: the message is given back.
        let evidence = attempts::result(&issued, "stopped", 220);
        record(&f, &issued, evidence.clone()).await.unwrap();
        assert_eq!(released(&f, &first.id).await, 1);
        assert_eq!(
            fence_says(&fence_facts(&f, &issued.command.command_id).await),
            ("computer".into(), 1, 0)
        );
        if stop {
            // A Stop cancels it with every other waiting message, and a
            // retry asked for afterwards opens nothing.
            f.domain
                .change_shared_run_control(
                    &f.owner.p,
                    &f.policy,
                    &a.id,
                    intake_tests::change("released_stop", 2, "paused"),
                    &m.gateway,
                )
                .await
                .unwrap();
            assert_eq!(recovered(&f, &lost).await.state, "cancelled");
            let reply = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
            assert_eq!(
                (reply.demand.state.as_str(), reply.demand.retry),
                ("cancelled", None)
            );
            assert_eq!(retries_opened(&f, &lost.offer.demand_id).await, 0);
            assert_eq!(
                status_of(&f, &first.room_id, &f.outsider.p, &event).await,
                json!(["cancelled", "explicit_stop", "explicit_stop", false])
            );
            continue;
        }
        // Without a request for the retry, it ends at the next poll.
        assert!(
            f.domain
                .poll_shared_demand(&f.owner.p, &f.policy, SharedDemandPollInput::default())
                .await
                .unwrap()
                .demand
                .is_none()
        );
        let ended = recovered(&f, &lost).await;
        assert_eq!((ended.state.as_str(), ended.retry), ("head_changed", None));
        assert_eq!(
            status_of(&f, &first.room_id, &f.outsider.p, &event).await,
            json!(["blocked", "head_changed", "retry_not_requested", true])
        );
    }
}

/// PostgreSQL runs the triggers of one row event in the order of their
/// names. At a process's fence the release must see the work before
/// `fence_shared_work` cancels it, and the fence's record counts what both
/// did (migration 2026-11-28-000161): renamed out of that order, the release
/// would find nothing to give back, silently.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_fence_triggers_run_in_the_order_the_release_needs() {
    let (f, _r, _admin, _a, _m) = Box::pin(setup()).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // The row triggers that run after an update of `permanent_fence`, in the
    // order PostgreSQL runs them.
    let fired = sql_query(
        "SELECT t.tgname::text AS value FROM pg_trigger t
         WHERE t.tgrelid='hagency_agent_v1.management_shared_runtimes'::regclass AND NOT t.tgisinternal
           AND t.tgtype & 19 = 17
           AND (t.tgattr::int2[] = '{}' OR (SELECT attnum FROM pg_attribute
                WHERE attrelid=t.tgrelid AND attname='permanent_fence') = ANY(t.tgattr::int2[]))
         ORDER BY t.tgname",
    )
    .load::<Text1>(&mut db)
    .await
    .unwrap()
    .into_iter()
    .map(|row| row.value)
    .collect::<Vec<_>>();
    let at = |name: &str| {
        fired
            .iter()
            .position(|fired| fired == name)
            .unwrap_or_else(|| panic!("{name} runs at the fence: {fired:?}"))
    };
    assert!(
        at("fence_shared_release") < at("fence_shared_work")
            && at("fence_shared_work") < at("record_shared_runtime_fence"),
        "{fired:?}"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_binding_and_its_event_are_released_only_with_the_proof() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "guarded").await;
    let (lost, _issued, work, _) = given(&f, &m, &d, &stopped, true).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // The process lives: nothing may be released by hand.
    for statement in [
        "DELETE FROM hagency_agent_v1.management_shared_demand_bindings WHERE work_id=$1",
        "DELETE FROM hagency_agent_v1.management_shared_event_pins WHERE work_id=$1",
        "INSERT INTO hagency_agent_v1.management_shared_demand_released_bindings SELECT work_id,demand_id,runtime_id,payload,digest,1 FROM hagency_agent_v1.management_shared_demand_bindings WHERE work_id=$1",
        "UPDATE hagency_agent_v1.management_shared_intake SET state='pending',reason=NULL WHERE id=(SELECT d.intake_id FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_demand_bindings b ON b.demand_id=d.id WHERE b.work_id=$1)",
    ] {
        assert!(
            sql_query(statement)
                .bind::<Text, _>(&work.id)
                .execute(&mut db)
                .await
                .is_err(),
            "{statement}"
        );
    }
    assert_eq!(recovered(&f, &lost).await.state, "bound");
    assert_eq!(released(&f, &work.id).await, 0);
}

#[derive(QueryableByName)]
struct Text1 {
    #[diesel(sql_type=Text)]
    value: String,
}
async fn text(f: &Fixture, query: &str, id: &str) -> Option<String> {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(query)
        .bind::<Text, _>(id)
        .load::<Text1>(&mut db)
        .await
        .unwrap()
        .pop()
        .map(|row| row.value)
}
async fn work_state(f: &Fixture, work: &str) -> String {
    text(
        f,
        "SELECT state AS value FROM hagency_agent_v1.management_shared_work WHERE id=$1",
        work,
    )
    .await
    .unwrap()
}
async fn block_payload(f: &Fixture, demand: &str) -> Option<String> {
    text(f, "SELECT payload AS value FROM hagency_agent_v1.management_shared_demand_blocks WHERE demand_id=$1", demand).await
}
async fn still_bound(f: &Fixture, work: &str) -> bool {
    text(f, "SELECT demand_id AS value FROM hagency_agent_v1.management_shared_demand_bindings WHERE work_id=$1", work)
        .await
        .is_some()
}
// The end of the last result ticket handed out for `runtime`.
async fn tickets_end(f: &Fixture, runtime: &str) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("SELECT coalesce(max(expires_at_ms),0) AS count FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1")
        .bind::<Text, _>(runtime)
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}
// The server's time bound stops `issued`, whose process went silent.
async fn time_bound(f: &Fixture, issued: &SharedAttemptDelivery) -> i64 {
    let at = issued.command.expires_at_ms + 30_000 + 180_000 + 1_000;
    f.domain.set_management_test_clock(at).await.unwrap();
    for _ in 0..64 {
        if runtime_state(f, &issued.command.command_id).await.0 == "stopped" {
            break;
        }
        let _ = f.domain.sweep_shared_attempt_timeouts().await;
    }
    assert_eq!(
        runtime_state(f, &issued.command.command_id).await,
        ("stopped".into(), true)
    );
    at
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_start_that_does_not_take_messages_back_keeps_them_as_before() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "kept_first").await;
    retain(&f, &r, &m, "kept_next").await;
    let (lost, issued, first, input) = given(&f, &m, &d, &stopped, false).await;
    // Its executor cannot ask for a retry of a process that ran: refused,
    // and nothing is recorded.
    let evidence = attempts::result(&issued, "stopped", 220);
    assert!(matches!(
        retry(&f, &lost, &issued, 1, evidence.clone()).await,
        Err(crate::Error::Invalid("invalid_shared_demand_retry"))
    ));
    assert_eq!(
        runtime_state(&f, &issued.command.command_id).await,
        ("qualified".into(), false)
    );
    // The process ends before the turn: the message stays with it and ends.
    record(&f, &issued, evidence).await.unwrap();
    assert_eq!(released(&f, &first.id).await, 0);
    assert_eq!(
        fence_says(&fence_facts(&f, &issued.command.command_id).await),
        ("computer".into(), 0, 0)
    );
    assert!(still_bound(&f, &first.id).await);
    assert_eq!(work_state(&f, &first.id).await, "cancelled");
    assert_eq!(recovered(&f, &lost).await.state, "bound");
    assert_eq!(
        status_of(&f, &first.room_id, &f.outsider.p, "$kept_first").await,
        json!(["cancelled", "never_started", "never_started", true])
    );
    assert!(
        f.domain
            .start_current_work(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .is_err()
    );
    assert_eq!(next_offer(&f).await.offer.event_id, "$kept_next");
}

/// The SQL query of the operations guide (`docs/OPERATIONS.md`) that
/// contains `marker`, as the guide gives it; the Chinese guide gives the
/// same queries.
fn guide_query(marker: &str) -> String {
    fn queries(guide: &str) -> Vec<&str> {
        guide
            .split("```sql\n")
            .skip(1)
            .map(|block| &block[..block.find("```").unwrap()])
            .collect()
    }
    let english = queries(include_str!("../../../docs/OPERATIONS.md"));
    assert_eq!(
        english,
        queries(include_str!("../../../docs/OPERATIONS.zh-CN.md")),
        "both guides give the same queries"
    );
    let mut found = english.into_iter().filter(|query| query.contains(marker));
    let query = found
        .next()
        .unwrap()
        .trim()
        .trim_end_matches(';')
        .to_string();
    assert!(found.next().is_none(), "one query contains {marker}");
    query
}

/// A message whose release fails ends as it did before, and the operator
/// learns of it: the service logs a warning with its ids and the SQLSTATE,
/// the fence record counts it, and the query of the operations guide finds
/// it.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_message_that_cannot_be_given_back_is_logged_and_found() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "not_given_back").await;
    let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
    // A fault in the release of this one message only.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!(
        "CREATE FUNCTION hagency_agent_v1.test_refuse_release() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN IF NEW.work_id='{}' THEN RAISE EXCEPTION 'refused by the test' USING ERRCODE='P0001'; END IF; RETURN NEW; END $$;
         CREATE TRIGGER test_refuse_release BEFORE INSERT ON hagency_agent_v1.management_shared_demand_released_bindings
          FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.test_refuse_release();",
        first.id
    ))
    .await
    .unwrap();
    let recorded = record(&f, &issued, attempts::result(&issued, "stopped", 220)).await;
    db.batch_execute(
        "DROP TRIGGER test_refuse_release ON hagency_agent_v1.management_shared_demand_released_bindings;
         DROP FUNCTION hagency_agent_v1.test_refuse_release();",
    )
    .await
    .unwrap();
    recorded.unwrap();
    // It ends as before: still bound to the process that ended.
    assert_eq!(released(&f, &first.id).await, 0);
    assert!(still_bound(&f, &first.id).await);
    assert_eq!(work_state(&f, &first.id).await, "cancelled");
    assert_eq!(
        fence_says(&fence_facts(&f, &issued.command.command_id).await),
        ("computer".into(), 0, 1)
    );
    // The service log names it by its ids and the SQLSTATE, nothing else.
    let notes = crate::database::emitted(&lost.offer.demand_id);
    assert_eq!(
        notes,
        vec![crate::database::ReofferNote {
            kind: "release_failed".into(),
            demand: Some(lost.offer.demand_id.clone()),
            work: Some(first.id.clone()),
            runtime: Some(issued.command.command_id.clone()),
            reason: Some("P0001".into()),
        }]
    );
    // The guide's query finds it.
    #[derive(QueryableByName)]
    struct Found {
        #[diesel(sql_type = Text)]
        runtime_id: String,
        #[diesel(sql_type = Text)]
        demand_id: String,
        #[diesel(sql_type = Text)]
        work_id: String,
        #[diesel(sql_type = Text)]
        event_id: String,
    }
    let found = sql_query(guide_query("releaseFailed"))
        .load::<Found>(&mut db)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.runtime_id == issued.command.command_id)
        .map(|row| (row.demand_id, row.work_id, row.event_id))
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        vec![(
            lost.offer.demand_id.clone(),
            first.id.clone(),
            "$not_given_back".to_string()
        )]
    );
}

/// After the re-offer is switched off, an operator waits for the query of the
/// operations guide to return 0 before moving the binary back. It counts
/// every message whose process said it takes messages back, stopped, and
/// left the message waiting for its executor, given back or never given to
/// the process at all, until the bounded wait ends it. An older binary would
/// wait for such a message's evidence until its own time ends.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_guide_counts_a_wait_for_the_executor_until_it_lapses() {
    let (mut f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "settle").await;
    let waiting = demand(&f).await;
    let issued = start_lapsed(
        &f,
        &m,
        &waiting,
        reoffering(start_input(&waiting, &d, &stopped)),
    )
    .await;
    // The process reports and goes silent before it is given the message;
    // the time bound stops it, and the message waits for the executor.
    f.domain
        .set_management_test_clock(issued.command.issued_at_ms + 1_000)
        .await
        .unwrap();
    record(&f, &issued, attempts::result(&issued, "qualified", 210))
        .await
        .unwrap();
    let at = time_bound(&f, &issued).await;
    f.policy = f.policy.clone().with_shared_demand_reoffers(false);
    assert_eq!(recovered(&f, &waiting).await.state, "start_committed");
    // The guide's count, by message.
    let counted =
        guide_query("shared_start_reoffers").replacen("SELECT count(*)", "SELECT d.id AS value", 1);
    let settle = |f: &Fixture| {
        let (url, counted, demand) = (
            f.url.clone(),
            counted.clone(),
            waiting.offer.demand_id.clone(),
        );
        async move {
            let mut db = AsyncPgConnection::establish(&url).await.unwrap();
            sql_query(&counted)
                .load::<Text1>(&mut db)
                .await
                .unwrap()
                .iter()
                .filter(|row| row.value == demand)
                .count()
        }
    };
    assert_eq!(settle(&f).await, 1);
    // Nothing was given back: a count of messages given back misses it.
    assert_eq!(
        count(
            &f,
            "SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_shared_demand_released_bindings WHERE demand_id=$1",
            &waiting.offer.demand_id
        )
        .await,
        0
    );
    // The bounded wait ends it, with the re-offer off, and the count is 0.
    f.domain
        .set_management_test_clock(at + 180_000)
        .await
        .unwrap();
    f.domain.sweep_shared_attempt_timeouts().await.unwrap();
    f.domain.reset_management_test_clock().await.unwrap();
    assert_eq!(
        block_payload(&f, &waiting.offer.demand_id).await.as_deref(),
        Some("\"retry_lapsed\"")
    );
    assert_eq!(settle(&f).await, 0);
}

/// A computer blocks a message in answer to an offer: one not yet started
/// (`offered`), or a retry (`retry_offered`). A message given back is offered
/// again once its computer asks for the retry; until then it waits for that
/// computer's evidence (`start_committed`), and a block is refused as for any
/// started message.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_message_given_back_is_blocked_once_it_is_offered_again() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "blocked_again").await;
    let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
    // The stop comes on the plain route: the message is given back and
    // waits for the computer.
    let evidence = attempts::result(&issued, "stopped", 220);
    record(&f, &issued, evidence.clone()).await.unwrap();
    assert_eq!(released(&f, &first.id).await, 1);
    let waiting = recovered(&f, &lost).await;
    assert_eq!(waiting.state, "start_committed");
    let block = |o: &SharedDemandStatus| SharedDemandBlockInput {
        demand_digest: o.demand_digest.clone(),
        expected_control_revision: o.offer.control_revision,
        reason: "usage_unrepresentable".into(),
        budget: None,
        retries: true,
        reoffers: true,
    };
    assert!(conflict(
        f.domain
            .block_shared_demand(&f.owner.p, &lost.offer.demand_id, block(&waiting))
            .await,
        "shared_demand_changed"
    ));
    // The computer asks for the retry with that receipt: offered again, the
    // message can be blocked, and it ends so.
    let reply = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
    assert_eq!(reply.demand.state, "retry_offered");
    let blocked = f
        .domain
        .block_shared_demand(&f.owner.p, &lost.offer.demand_id, block(&reply.demand))
        .await
        .unwrap();
    assert_eq!(blocked.state, "usage_unrepresentable");
    assert_eq!(
        status_of(&f, &first.room_id, &f.outsider.p, "$blocked_again").await[2],
        "usage_unrepresentable"
    );
}

// Whether the Agent of `assignment` serves in `room` now, by the rule the
// chat status reads; the status also makes the service's check of the
// reservation, which agrees.
async fn serves(f: &Fixture, assignment: &str, room: &str) -> bool {
    let (serves, stands) = f
        .domain
        .shared_agent_serves_at(assignment, room, db_now(f).await)
        .await
        .unwrap();
    assert!(stands || !serves, "the rule serves where the check refuses");
    serves
}

/// One rule decides whether a message is given back at its process's fence
/// and whether its sender is offered to send it again: the Agent serves
/// (`shared_agent_serves()`, which the chat status calls). In each case the
/// message is given back exactly when the Agent serves by that rule and no
/// budget was applied to it, and the status reads the same rule.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_agent_that_no_longer_serves_gets_nothing_back() {
    // (case, the Agent serves, the message is given back)
    for (case, serving, back) in [
        ("serving", true, true),
        ("stopped", false, false),
        ("room", false, false),
        ("time", false, false),
        ("withdrawn", false, false),
        ("ledger", false, false),
        ("budget", true, false),
    ] {
        let (mut f, r, admin, a, m) = Box::pin(setup()).await;
        f.policy = f.policy.clone().with_agent_budget_deltas(true);
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        let event = format!("$served_{case}");
        retain(&f, &r, &m, &event[1..]).await;
        let (_lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
        let report = attempts::result(&issued, "stopped", 220);
        match case {
            "serving" => {
                // Its computer reports that the process ended before the turn.
                record(&f, &issued, report).await.unwrap();
            }
            "stopped" => {
                // Its owner stops it. The Stop changes the Agent's control,
                // then ends its processes: a stopped Agent is given nothing
                // back.
                f.domain
                    .change_shared_run_control(
                        &f.owner.p,
                        &f.policy,
                        &a.id,
                        intake_tests::change("served_stop", 2, "paused"),
                        &m.gateway,
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    fence_says(&fence_facts(&f, &issued.command.command_id).await),
                    ("server".into(), 0, 0)
                );
            }
            "room" => {
                // Its membership of the room is no longer confirmed (as the
                // Matrix check records it); then its process ends.
                let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
                sql_query("UPDATE hagency_agent_v1.management_agent_matrix SET state='waiting_native' WHERE assignment_id=$1")
                    .bind::<Text, _>(&a.id)
                    .execute(&mut db)
                    .await
                    .unwrap();
                record(&f, &issued, report).await.unwrap();
            }
            "time" => {
                // Its reservation's time is over: the allocation it stands
                // on has ended. Then its process ends. Nothing reads its
                // Matrix readiness in between, which would withdraw the
                // reservation for good.
                status_tests::allocation_ends(&f, &r, db_now(&f).await - 1).await;
                record(&f, &issued, report).await.unwrap();
            }
            "withdrawn" => {
                // Its manager withdraws the request: the Agent's processes
                // are fenced with it.
                f.domain
                    .cancel_managed_agent_request(
                        &f.outsider.p,
                        &f.policy,
                        &r.id,
                        crate::management_types::RevokeInput {
                            command_id: "served_cancel".into(),
                            expected_revision: r.revision,
                        },
                    )
                    .await
                    .unwrap();
                assert!(runtime_state(&f, &issued.command.command_id).await.1);
            }
            "ledger" => {
                // The executor's device replaces its ledger database: the
                // Agent is fenced, and so are its processes.
                fresh_ledger(&f).await;
                assert_eq!(
                    runtime_state(&f, &issued.command.command_id).await,
                    ("stopped".into(), true)
                );
            }
            _ => {
                // A budget change is applied to the Agent; then its process
                // ends.
                budget_tests::apply_delta(&f, &r, &admin, 25, 230).await;
                time_bound(&f, &issued).await;
                f.domain.reset_management_test_clock().await.unwrap();
            }
        }
        assert_eq!(serves(&f, &a.id, &first.room_id).await, serving, "{case}");
        assert_eq!(released(&f, &first.id).await, i64::from(back), "{case}");
        let status = status_of(&f, &first.room_id, &f.outsider.p, &event).await;
        if back {
            // A waiting message again: nothing ended, nothing to send again.
            assert_eq!(status, json!(["pending", null, null, false]), "{case}");
            continue;
        }
        assert!(still_bound(&f, &first.id).await, "{case}");
        assert_eq!(work_state(&f, &first.id).await, "cancelled", "{case}");
        // It ends as before, and its sender is offered to send it again
        // exactly when the Agent serves.
        assert_eq!(
            status,
            json!(["cancelled", "never_started", "never_started", serving]),
            "{case}"
        );
    }
}

/// The rule reads a project Agent's reservation as the service's check does,
/// at the time it is given: the Agent serves until its allocation ends, and
/// from that instant on it does not. The end is moved to a minute from now,
/// before the reservation's other ends (its reserve command's and its
/// grant's), so that the allocation's end is the one read.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_agent_serves_reads_a_project_allocation_end_as_the_service_does() {
    let (f, r, _admin, a, _m) = Box::pin(setup()).await;
    let room = work::message(&f, &r, "allocation_end").await.room_id;
    let now = db_now(&f).await;
    let end = now + 60_000;
    status_tests::allocation_ends(&f, &r, end).await;
    for (at, serving) in [(now, true), (end - 1, true), (end, false)] {
        assert_eq!(
            f.domain
                .shared_agent_serves_at(&a.id, &room, at)
                .await
                .unwrap(),
            (serving, serving),
            "{} ms after now",
            at - now
        );
    }
}

/// The rule reads a pool Agent's reservation as the service's check does: it
/// serves up to the turn of its period; at the turn its reservation's time
/// is over, until its pool's grant for the next period is in place, and then
/// it serves again.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_agent_serves_reads_a_pool_reservation_across_the_turn_as_the_service_does() {
    let (f, request, assignment, _m) = Box::pin(prepared_pool_calendar(true, true)).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let now = now_ms();
    let turn = period_end(&mut db, &assignment.id, now).await;
    let at = async |at: i64| {
        f.domain
            .shared_agent_serves_at(&assignment.id, &request.room_id, at)
            .await
            .unwrap()
    };
    assert_eq!(at(now).await, (true, true), "before the turn");
    assert_eq!(at(turn - 1).await, (true, true), "1 ms before the turn");
    assert_eq!(at(turn).await, (false, false), "at the turn");
    renew_pool_grant(&f, &request, turn).await;
    assert_eq!(at(turn).await, (true, true), "at the turn, renewed");
    assert_eq!(at(turn - 1).await, (true, true), "before the turn, renewed");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_message_given_back_waits_for_its_executor_a_bounded_time() {
    for comes_back in [false, true] {
        let (f, r, _admin, a, m) = Box::pin(setup()).await;
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        let event = format!("bounded_{comes_back}");
        retain(&f, &r, &m, &event).await;
        retain(&f, &r, &m, &format!("bounded_next_{comes_back}")).await;
        let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
        if comes_back {
            // The cleanup ticket asked for below replaces this process's
            // result tickets, whose deletion PostgreSQL guards by its real
            // clock: they must have lapsed for real before the clock moves.
            wait_until(&f, tickets_end(&f, &issued.command.command_id).await).await;
        }
        // The process goes silent. The time bound stops it, and the message
        // is given back: it waits for the executor's evidence.
        let at = time_bound(&f, &issued).await;
        assert_eq!(released(&f, &first.id).await, 1);
        assert_eq!(
            fence_says(&fence_facts(&f, &issued.command.command_id).await),
            ("time_bound".into(), 1, 0)
        );
        assert_eq!(recovered(&f, &lost).await.state, "start_committed");
        assert_eq!(
            status_of(&f, &first.room_id, &f.outsider.p, &format!("${event}")).await,
            json!(["pending", null, null, false])
        );
        // Nothing has ended: no reader is offered to send it again, nor
        // told why not.
        for reader in [&f.outsider.p, &f.owner.p] {
            assert_eq!(
                sending_again(&f, &first.room_id, reader, &format!("${event}")).await,
                json!([false, null])
            );
        }
        if comes_back {
            // The executor comes back in time, proves the stop and starts the
            // retry.
            let cleanup = attempt_poll(&f).await;
            assert_eq!(cleanup.command.command_id, issued.command.command_id);
            let mut evidence = attempts::result(&issued, "stopped", 220);
            evidence.receipt.reason = Some(UNCERTAIN_TIMEOUT.into());
            evidence.token = cleanup.token;
            let reply = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
            assert_eq!(reply.demand.state, "retry_offered");
            // Offered again, it reads as waiting: not as a message that
            // never began, and its sender is not offered to send it again.
            assert_eq!(
                status_of(&f, &first.room_id, &f.outsider.p, &format!("${event}")).await,
                json!(["pending", null, null, false])
            );
            assert_eq!(
                sending_again(&f, &first.room_id, &f.outsider.p, &format!("${event}")).await,
                json!([false, null])
            );
            let started = f
                .domain
                .start_shared_demand(
                    &f.owner.p,
                    &f.policy,
                    &lost.offer.demand_id,
                    after_stopped(
                        &reply.demand,
                        &issued,
                        &reply.acknowledgement.receipt_digest,
                        221,
                        1,
                    ),
                    &m.gateway,
                )
                .await
                .unwrap();
            assert!(started.execution_allowed);
            // So it reads once the retry has a process.
            assert_eq!(
                status_of(&f, &first.room_id, &f.outsider.p, &format!("${event}")).await,
                json!(["pending", null, null, false])
            );
        }
        // A grace later the wait is over, unless the retry has a process.
        f.domain
            .set_management_test_clock(at + 180_000)
            .await
            .unwrap();
        f.domain.sweep_shared_attempt_timeouts().await.unwrap();
        let after_wait = recovered(&f, &lost).await;
        if comes_back {
            assert_eq!(
                (after_wait.state.as_str(), after_wait.retry),
                ("start_committed", Some(1)),
                "a retry with a process is not lapsed"
            );
            assert_eq!(block_payload(&f, &lost.offer.demand_id).await, None);
            f.domain.reset_management_test_clock().await.unwrap();
            continue;
        }
        assert_eq!(after_wait.state, "head_changed");
        assert_eq!(
            block_payload(&f, &lost.offer.demand_id).await.as_deref(),
            Some("\"retry_lapsed\"")
        );
        // No turn began for it anywhere, and its sender can send it again.
        // The reason stays `head_changed`, as for any end of a waiting
        // message, and the end is the block's.
        assert_eq!(
            status_of(&f, &first.room_id, &f.outsider.p, &format!("${event}")).await,
            json!(["blocked", "head_changed", "retry_lapsed", true])
        );
        // Its sender is offered it, and nobody else is told anything.
        assert_eq!(
            sending_again(&f, &first.room_id, &f.outsider.p, &format!("${event}")).await,
            json!([true, null])
        );
        assert_eq!(
            sending_again(&f, &first.room_id, &f.owner.p, &format!("${event}")).await,
            json!([false, null])
        );
        f.domain.reset_management_test_clock().await.unwrap();
        assert_eq!(
            logged(&lost.offer.demand_id),
            vec![
                ("released".into(), None),
                ("ended".into(), Some("retry_lapsed".into()))
            ]
        );
        assert_eq!(
            next_offer(&f).await.offer.event_id,
            format!("$bounded_next_{comes_back}")
        );
        // Once only its Agent stands in the way, its sender is told why.
        f.domain
            .change_shared_run_control(
                &f.owner.p,
                &f.policy,
                &a.id,
                intake_tests::change("bounded_stop", 2, "paused"),
                &m.gateway,
            )
            .await
            .unwrap();
        assert_eq!(
            sending_again(&f, &first.room_id, &f.outsider.p, &format!("${event}")).await,
            json!([false, "agent_stopped"])
        );
        assert_eq!(
            sending_again(&f, &first.room_id, &f.owner.p, &format!("${event}")).await,
            json!([false, null])
        );
    }
}
/// Whether `reader` may send `event` in `room` again, and why not when only
/// its Agent stands in the way.
async fn sending_again(f: &Fixture, room: &str, reader: &Principal, event: &str) -> Value {
    let query = serde_json::from_value(json!({"roomId": room})).unwrap();
    let read = f.domain.shared_approvals(reader, query).await.unwrap();
    let row = read["executions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["eventId"] == event)
        .cloned()
        .unwrap_or_else(|| panic!("{event} is among the reader's newest messages"));
    json!([row["canSendAgain"], row["notOfferedBecause"]])
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_executor_that_does_not_take_messages_back_never_sees_one() {
    for asked in [false, true] {
        let (f, r, _admin, _a, m) = Box::pin(setup()).await;
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        let event = format!("downgraded_{asked}");
        retain(&f, &r, &m, &event).await;
        retain(&f, &r, &m, &format!("downgraded_next_{asked}")).await;
        let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
        if asked {
            // The process ends and the executor asks for the retry.
            let evidence = attempts::result(&issued, "stopped", 220);
            let reply = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
            assert_eq!(reply.demand.state, "retry_offered");
        } else {
            // The time bound stops the silent process: the message waits for
            // the executor that started it.
            time_bound(&f, &issued).await;
            f.domain.reset_management_test_clock().await.unwrap();
            let waiting = recovered(&f, &lost).await;
            assert_eq!(
                (waiting.state.as_str(), waiting.retry),
                ("start_committed", None)
            );
        }
        assert_eq!(released(&f, &first.id).await, 1);
        // That executor is replaced by one that runs retries but does not
        // take messages back (a downgrade): it reads the message as ended,
        // as before, and the queue moves on.
        let old = f
            .domain
            .recover_shared_demand(
                &f.owner.p,
                &lost.offer.demand_id,
                SharedDemandRecoverInput {
                    demand_digest: lost.demand_digest.clone(),
                    retries: true,
                    reoffers: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(old.state, "head_changed");
        assert_eq!(
            block_payload(&f, &lost.offer.demand_id).await.as_deref(),
            Some("\"reoffer_unsupported\"")
        );
        assert_eq!(
            status_of(&f, &first.room_id, &f.outsider.p, &format!("${event}")).await,
            json!(["blocked", "head_changed", "reoffer_unsupported", true])
        );
        assert_eq!(
            next_offer(&f).await.offer.event_id,
            format!("$downgraded_next_{asked}")
        );
    }
}

// Retry `n` of the message of `o` after `previous` ran and stopped (`stop`
// is its stop receipt's digest): the retry's process starts and reports, is
// given the message and acknowledges it. No turn has begun.
async fn rerun(
    f: &Fixture,
    m: &MatrixFixture,
    o: &SharedDemandStatus,
    previous: &SharedAttemptDelivery,
    stop: &str,
    n: i64,
    observed: i64,
) -> (SharedAttemptDelivery, SharedWorkItem, SharedWorkInput) {
    let started = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &o.offer.demand_id,
            after_stopped(o, previous, stop, observed, n),
            &m.gateway,
        )
        .await
        .unwrap();
    assert!(started.execution_allowed);
    let activity = record(
        f,
        &started,
        attempts::result(&started, "qualified", observed + 9),
    )
    .await
    .unwrap()
    .activity
    .unwrap();
    let work = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    let input = SharedWorkInput {
        authority: activity.authority,
        work_id: work.id.clone(),
        work_digest: allocation_digest(&work).unwrap(),
    };
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    (started, work, input)
}
async fn count(f: &Fixture, query: &str, id: &str) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(query)
        .bind::<Text, _>(id)
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count
}
// Demands made for `event`.
async fn demands_for(f: &Fixture, event: &str) -> i64 {
    count(f, "SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_intake q ON q.id=d.intake_id WHERE q.event_id=$1", event).await
}
async fn retries_opened(f: &Fixture, demand: &str) -> i64 {
    count(f, "SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_shared_demand_retries WHERE demand_id=$1", demand).await
}
async fn retry_opened_at(f: &Fixture, demand: &str) -> i64 {
    count(f, "SELECT created_at_ms AS count FROM hagency_agent_v1.management_shared_demand_retries WHERE demand_id=$1 AND retry=1", demand).await
}
async fn stop_receipts(f: &Fixture, runtime: &str) -> i64 {
    count(f, "SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_shared_process_receipts WHERE runtime_id=$1 AND payload::jsonb->>'state'='stopped'", runtime).await
}
// The facts of the record of `runtime`'s fence.
async fn fence_facts(f: &Fixture, runtime: &str) -> Value {
    serde_json::from_str(
        &text(
            f,
            "SELECT facts AS value FROM hagency_agent_v1.management_shared_runtime_fences WHERE runtime_id=$1",
            runtime,
        )
        .await
        .expect("the fence is recorded"),
    )
    .unwrap()
}
fn fence_says(facts: &Value) -> (String, i64, i64) {
    (
        facts["stopSource"].as_str().unwrap().into(),
        facts["released"].as_i64().unwrap(),
        facts["releaseFailed"].as_i64().unwrap(),
    )
}
// What the service log says about `demand`: the kind and reason of each line.
fn logged(demand: &str) -> Vec<(String, Option<String>)> {
    crate::database::emitted(demand)
        .into_iter()
        .map(|n| (n.kind, n.reason))
        .collect()
}
async fn capabilities(f: &Fixture, m: &MatrixFixture) -> Vec<String> {
    let mut response = TestClient::get("https://example.test/hagency/api/v1/discovery")
        .add_header("host", "example.test", true)
        .send(&svc(f, m.gateway.clone()))
        .await;
    let value = response.take_json::<Value>().await.unwrap();
    value["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}
// The executor's declaration is trusted 300 s by the server's clock. Tests
// that move that clock further keep it current.
async fn keep_declared(f: &Fixture) {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_shared_executor_protocols SET expires_at_ms=expires_at_ms+3600000 WHERE device_id=$1")
        .bind::<Text, _>(f.owner.p.device_id.as_deref().unwrap())
        .execute(&mut db)
        .await
        .unwrap();
}
async fn poll_for(f: &Fixture, reoffers: bool) -> Option<SharedDemandStatus> {
    f.domain
        .poll_shared_demand(
            &f.owner.p,
            &f.policy,
            SharedDemandPollInput {
                after: None,
                retries: true,
                reoffers,
            },
        )
        .await
        .unwrap()
        .demand
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_next_message_waits_while_the_first_has_not_begun() {
    for says in [true, false] {
        let (f, r, _admin, _a, m) = Box::pin(setup()).await;
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        let first_event = format!("$waits_first_{says}");
        let next_event = format!("$waits_next_{says}");
        retain(&f, &r, &m, &first_event[1..]).await;
        retain(&f, &r, &m, &next_event[1..]).await;
        let (lost, issued, _first, _) = given(&f, &m, &d, &stopped, true).await;
        let polled = poll_for(&f, says).await;
        if !says {
            // An executor that does not take messages back is offered the
            // next message at once, as before: after the process that holds
            // the first one.
            let next = polled.expect("offered as before");
            assert_eq!(
                (
                    next.offer.event_id.as_str(),
                    next.offer.predecessor_command_id.as_str(),
                    next.state.as_str()
                ),
                (
                    next_event.as_str(),
                    issued.command.command_id.as_str(),
                    "offered"
                )
            );
            continue;
        }
        // While the first message waits for its turn in that process, the
        // next one is not offered, and no demand names that process.
        assert!(polled.is_none());
        assert_eq!(demands_for(&f, &next_event).await, 0);
        // The process ends before the turn. The first message is taken back
        // and runs first, in the next process.
        let reply = retry(
            &f,
            &lost,
            &issued,
            1,
            attempts::result(&issued, "stopped", 220),
        )
        .await
        .unwrap();
        assert_eq!(reply.demand.state, "retry_offered");
        assert_eq!(next_offer(&f).await.offer.demand_id, lost.offer.demand_id);
        assert_eq!(
            fence_says(&fence_facts(&f, &issued.command.command_id).await),
            ("computer".into(), 1, 0)
        );
        let (started, second, input) = rerun(
            &f,
            &m,
            &reply.demand,
            &issued,
            &reply.acknowledgement.receipt_digest,
            1,
            221,
        )
        .await;
        assert_eq!(second.event_id, first_event);
        // Given again, not begun: the next message still waits.
        assert!(poll_for(&f, true).await.is_none());
        assert_eq!(demands_for(&f, &next_event).await, 0);
        // The turn begins. Now the next message is offered, after the
        // process that runs the first one.
        f.domain
            .start_current_work(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .unwrap();
        let next = next_offer(&f).await;
        assert_eq!(
            (
                next.offer.event_id.as_str(),
                next.offer.predecessor_command_id.as_str(),
                next.state.as_str()
            ),
            (
                next_event.as_str(),
                started.command.command_id.as_str(),
                "offered"
            )
        );
        // That process ends, and the next message runs in the one after it.
        let ended = record(&f, &started, attempts::result(&started, "stopped", 240))
            .await
            .unwrap();
        let mut start = after(&next, &started, &ended.receipt_digest, &"d".repeat(64), 241);
        start.reoffers = true;
        let third = f
            .domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &next.offer.demand_id,
                start,
                &m.gateway,
            )
            .await
            .unwrap();
        assert!(third.execution_allowed);
        let activity = record(&f, &third, attempts::result(&third, "qualified", 250))
            .await
            .unwrap()
            .activity
            .unwrap();
        let work = f
            .domain
            .poll_shared_work(&f.owner.p, &f.policy, activity.authority, &m.gateway)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(work.event_id, next_event);
    }
}

/// The hold does not ask whether the Agent serves: the release asks that at
/// the fence. An Agent that does not serve at the poll but serves again by
/// the fence has its first message given back, and the next one, held
/// meanwhile, runs after it instead of ending.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_next_message_is_held_while_its_agent_briefly_does_not_serve() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "brief_first").await;
    retain(&f, &r, &m, "brief_next").await;
    let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
    // At the poll the Agent's membership of the room is not confirmed (as
    // the Matrix check records it), which the next message's own checks do
    // not read.
    let verified = text(
        &f,
        "SELECT state AS value FROM hagency_agent_v1.management_agent_matrix WHERE assignment_id=$1",
        &a.id,
    )
    .await
    .unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_agent_matrix SET state='waiting_native' WHERE assignment_id=$1")
        .bind::<Text, _>(&a.id)
        .execute(&mut db)
        .await
        .unwrap();
    assert!(!serves(&f, &a.id, &first.room_id).await);
    let early = poll_for(&f, true).await;
    // It is confirmed again before the process ends: the first message is
    // given back, and runs in the next process.
    sql_query(
        "UPDATE hagency_agent_v1.management_agent_matrix SET state=$2 WHERE assignment_id=$1",
    )
    .bind::<Text, _>(&a.id)
    .bind::<Text, _>(&verified)
    .execute(&mut db)
    .await
    .unwrap();
    assert!(serves(&f, &a.id, &first.room_id).await);
    let reply = retry(
        &f,
        &lost,
        &issued,
        1,
        attempts::result(&issued, "stopped", 220),
    )
    .await
    .unwrap();
    assert_eq!(reply.demand.state, "retry_offered");
    assert_eq!(released(&f, &first.id).await, 1);
    let (started, second, input) = rerun(
        &f,
        &m,
        &reply.demand,
        &issued,
        &reply.acknowledgement.receipt_digest,
        1,
        221,
    )
    .await;
    assert_eq!(second.event_id, "$brief_first");
    f.domain
        .start_current_work(&f.owner.p, &f.policy, input, &m.gateway)
        .await
        .unwrap();
    // The next message was not offered while the first one waited, so it
    // names the process that runs the first one, and is offered.
    if let Some(early) = early {
        // As the next poll finds it.
        poll_for(&f, true).await;
        panic!(
            "offered behind the process that ended, it ends {}",
            recovered(&f, &early).await.state
        );
    }
    let next = next_offer(&f).await;
    assert_eq!(
        (
            next.offer.event_id.as_str(),
            next.offer.predecessor_command_id.as_str(),
            next.state.as_str()
        ),
        (
            "$brief_next",
            started.command.command_id.as_str(),
            "offered"
        )
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_message_given_back_waits_once_counted_from_its_first_process() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "counted_once").await;
    let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
    // The first process ends before the turn; the message is taken back.
    let reply = retry(
        &f,
        &lost,
        &issued,
        1,
        attempts::result(&issued, "stopped", 220),
    )
    .await
    .unwrap();
    // Its retry runs in time, is given the message, and ends before its
    // turn too: taken back again.
    let (started, second, _) = rerun(
        &f,
        &m,
        &reply.demand,
        &issued,
        &reply.acknowledgement.receipt_digest,
        1,
        221,
    )
    .await;
    let again = retry(
        &f,
        &lost,
        &started,
        2,
        attempts::result(&started, "stopped", 240),
    )
    .await
    .unwrap();
    assert_eq!(
        (again.demand.state.as_str(), again.demand.retry),
        ("retry_offered", Some(2))
    );
    assert_eq!(
        (
            released(&f, &first.id).await,
            released(&f, &second.id).await
        ),
        (1, 1)
    );
    // One bound for the message: 180 s after the later of the end of its
    // first process's grace (the start window, the first lease and 180 s)
    // and the first request for a retry.
    let opened = retry_opened_at(&f, &lost.offer.demand_id).await;
    let bound = (issued.command.expires_at_ms + 30_000 + 180_000).max(opened) + 180_000;
    // Counted from the latest attempt, it would end later.
    assert!(bound < started.command.expires_at_ms + 30_000 + 180_000 + 180_000);
    keep_declared(&f).await;
    // Just before the bound the message still waits.
    f.domain.set_management_test_clock(bound - 1).await.unwrap();
    f.domain.sweep_shared_attempt_timeouts().await.unwrap();
    assert_eq!(recovered(&f, &lost).await.state, "retry_offered");
    assert_eq!(block_payload(&f, &lost.offer.demand_id).await, None);
    // At the bound no attempt starts for it any more, and it ends.
    f.domain.set_management_test_clock(bound).await.unwrap();
    assert!(conflict(
        f.domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &lost.offer.demand_id,
                after_stopped(
                    &again.demand,
                    &started,
                    &again.acknowledgement.receipt_digest,
                    241,
                    2,
                ),
                &m.gateway,
            )
            .await,
        "shared_demand_not_current"
    ));
    f.domain.sweep_shared_attempt_timeouts().await.unwrap();
    assert_eq!(recovered(&f, &lost).await.state, "head_changed");
    assert_eq!(
        block_payload(&f, &lost.offer.demand_id).await.as_deref(),
        Some("\"retry_lapsed\"")
    );
    f.domain.reset_management_test_clock().await.unwrap();
    // The service log has a line for each release and one for the end.
    assert_eq!(
        logged(&lost.offer.demand_id),
        vec![
            ("released".into(), None),
            ("released".into(), None),
            ("ended".into(), Some("retry_lapsed".into()))
        ]
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_reoffer_switched_off_keeps_v1_retries_and_ends_messages_given_back() {
    let (mut f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "off_given_back").await;
    retain(&f, &r, &m, "off_kept").await;
    retain(&f, &r, &m, "off_retried").await;
    let on = capabilities(&f, &m).await;
    assert!(on.iter().any(|c| c == "shared-demand-retry-v1"));
    assert!(on.iter().any(|c| c == "shared-demand-retry-v2"));
    let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
    // While it is on, the process ends before its turn and the message is
    // taken back.
    let reply = retry(
        &f,
        &lost,
        &issued,
        1,
        attempts::result(&issued, "stopped", 220),
    )
    .await
    .unwrap();
    assert_eq!(reply.demand.state, "retry_offered");
    assert_eq!(released(&f, &first.id).await, 1);
    // The operator switches it off; demand retries stay.
    f.policy = f.policy.clone().with_shared_demand_reoffers(false);
    let off = capabilities(&f, &m).await;
    assert!(off.iter().any(|c| c == "shared-demand-retry-v1"));
    assert!(!off.iter().any(|c| c == "shared-demand-retry-v2"));
    // The message taken back ends at its bound, visibly.
    let bound = (issued.command.expires_at_ms + 30_000 + 180_000)
        .max(retry_opened_at(&f, &lost.offer.demand_id).await)
        + 180_000;
    f.domain.set_management_test_clock(bound).await.unwrap();
    f.domain.sweep_shared_attempt_timeouts().await.unwrap();
    f.domain.reset_management_test_clock().await.unwrap();
    assert_eq!(recovered(&f, &lost).await.state, "head_changed");
    assert_eq!(
        block_payload(&f, &lost.offer.demand_id).await.as_deref(),
        Some("\"retry_lapsed\"")
    );
    assert_eq!(
        status_of(&f, &first.room_id, &f.outsider.p, "$off_given_back").await,
        json!(["blocked", "head_changed", "retry_lapsed", true])
    );
    // A start that says it takes messages back is kept as before: its
    // process ends before the turn, the retry route refuses its stop, and
    // nothing is given back.
    let kept = next_offer(&f).await;
    assert_eq!(kept.offer.event_id, "$off_kept");
    let mut start = after(
        &kept,
        &issued,
        &reply.acknowledgement.receipt_digest,
        &"d".repeat(64),
        221,
    );
    start.reoffers = true;
    let next = f
        .domain
        .start_shared_demand(
            &f.owner.p,
            &f.policy,
            &kept.offer.demand_id,
            start,
            &m.gateway,
        )
        .await
        .unwrap();
    let activity = record(&f, &next, attempts::result(&next, "qualified", 230))
        .await
        .unwrap()
        .activity
        .unwrap();
    let work = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &m.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    f.domain
        .acknowledge_shared_work(
            &f.owner.p,
            &f.policy,
            SharedWorkInput {
                authority: activity.authority,
                work_id: work.id.clone(),
                work_digest: allocation_digest(&work).unwrap(),
            },
            &m.gateway,
        )
        .await
        .unwrap();
    let evidence = attempts::result(&next, "stopped", 240);
    assert!(matches!(
        retry(&f, &kept, &next, 1, evidence.clone()).await,
        Err(crate::Error::Invalid("invalid_shared_demand_retry"))
    ));
    assert_eq!(stop_receipts(&f, &next.command.command_id).await, 0);
    let ended = record(&f, &next, evidence).await.unwrap();
    assert_eq!(released(&f, &work.id).await, 0);
    assert_eq!(work_state(&f, &work.id).await, "cancelled");
    assert_eq!(
        fence_says(&fence_facts(&f, &next.command.command_id).await),
        ("computer".into(), 0, 0)
    );
    assert_eq!(
        status_of(&f, &first.room_id, &f.outsider.p, "$off_kept").await,
        json!(["cancelled", "never_started", "never_started", true])
    );
    // A start that never happened is still run again.
    let lost = next_offer(&f).await;
    assert_eq!(lost.offer.event_id, "$off_retried");
    let issued = start_lapsed(
        &f,
        &m,
        &lost,
        after(&lost, &next, &ended.receipt_digest, &"d".repeat(64), 241),
    )
    .await;
    let reply = retry(&f, &lost, &issued, 1, unstarted(&f, &issued, 250).await)
        .await
        .unwrap();
    assert_eq!(
        (reply.demand.state.as_str(), reply.demand.retry),
        ("retry_offered", Some(1))
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_budget_applied_after_a_release_opens_no_retry_and_the_message_ends() {
    let (mut f, r, admin, _a, m) = Box::pin(setup()).await;
    f.policy = f
        .policy
        .clone()
        .with_agent_budget_deltas(true)
        .with_shared_budget_execution(true);
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "budget_after_release").await;
    retain(&f, &r, &m, "budget_after_release_next").await;
    let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
    // The process ends before its turn and its executor reports it; the
    // message is taken back.
    let evidence = attempts::result(&issued, "stopped", 220);
    record(&f, &issued, evidence.clone()).await.unwrap();
    assert_eq!(released(&f, &first.id).await, 1);
    // A budget change is applied to the Agent before the executor asks for
    // the retry. From then on the message reads as ended, as every message
    // the earlier protocol had started when the budget was applied, and it
    // keeps that one reading whichever record ends it below.
    budget_tests::apply_delta(&f, &r, &admin, 25, 230).await;
    let ended = json!(["blocked", "head_changed", "budget_applied", true]);
    let reading =
        async || status_of(&f, &first.room_id, &f.outsider.p, "$budget_after_release").await;
    assert_eq!(reading().await, ended);
    // The stop is already recorded, and no retry is opened.
    let reply = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
    assert_eq!(
        (reply.demand.state.as_str(), reply.demand.retry),
        ("start_committed", None)
    );
    assert_eq!(retries_opened(&f, &lost.offer.demand_id).await, 0);
    assert!(conflict(
        f.domain
            .start_shared_demand(
                &f.owner.p,
                &f.policy,
                &lost.offer.demand_id,
                after_stopped(
                    &reply.demand,
                    &issued,
                    &reply.acknowledgement.receipt_digest,
                    221,
                    1
                ),
                &m.gateway,
            )
            .await,
        "shared_budget_execution_required"
    ));
    // The budget's queue does not wait behind it: it records that end and
    // offers the next message.
    let offered = f
        .domain
        .poll_shared_budget_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .demand
        .expect("the next message is offered");
    assert_eq!(offered.offer.event_id, "$budget_after_release_next");
    assert_eq!(reading().await, ended);
    // For its executor the message ends at its bound, with the same reason.
    let bound = issued.command.expires_at_ms + 30_000 + 180_000 + 180_000;
    f.domain.set_management_test_clock(bound - 1).await.unwrap();
    f.domain.sweep_shared_attempt_timeouts().await.unwrap();
    assert_eq!(block_payload(&f, &lost.offer.demand_id).await, None);
    f.domain.set_management_test_clock(bound).await.unwrap();
    f.domain.sweep_shared_attempt_timeouts().await.unwrap();
    f.domain.reset_management_test_clock().await.unwrap();
    assert_eq!(recovered(&f, &lost).await.state, "head_changed");
    assert_eq!(
        block_payload(&f, &lost.offer.demand_id).await.as_deref(),
        Some("\"budget_applied\"")
    );
    assert_eq!(reading().await, ended);
    assert_eq!(
        logged(&lost.offer.demand_id),
        vec![
            ("released".into(), None),
            ("ended".into(), Some("budget_applied".into()))
        ]
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_period_turn_ends_a_message_given_back() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "period_given_back").await;
    let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
    let reply = retry(
        &f,
        &lost,
        &issued,
        1,
        attempts::result(&issued, "stopped", 220),
    )
    .await
    .unwrap();
    assert_eq!(reply.demand.state, "retry_offered");
    assert_eq!(released(&f, &first.id).await, 1);
    // The Agent's allowance runs by day, and the day turns while the
    // message waits to run again.
    let calendar = crate::recurring_quota::Calendar {
        cadence: crate::recurring_quota::Cadence::Daily,
        time_zone: "UTC".into(),
    };
    let period = calendar.period_at(db_now(&f).await as u64).unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_agent_recurring_terms(request_id,calendar,renewal_mode) VALUES($1,$2,'owner')")
        .bind::<Text, _>(&r.id)
        .bind::<Text, _>(serde_json::to_string(&calendar).unwrap())
        .execute(&mut db)
        .await
        .unwrap();
    f.domain
        .set_management_test_clock(period.end_ms as i64 + 1_000)
        .await
        .unwrap();
    // It ends with its period, as every message of that period does.
    assert_eq!(recovered(&f, &lost).await.state, "expired");
    // Not by its wait: the sweep finds nothing left to end.
    f.domain.sweep_shared_attempt_timeouts().await.unwrap();
    assert_eq!(block_payload(&f, &lost.offer.demand_id).await, None);
    assert_eq!(
        status_of(&f, &first.room_id, &f.outsider.p, "$period_given_back").await,
        json!(["expired", "event_expired", "event_expired", true])
    );
    f.domain.reset_management_test_clock().await.unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_message_taken_back_after_each_stop_ends_at_the_retry_cap() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "capped").await;
    retain(&f, &r, &m, "capped_next").await;
    let (lost, mut previous, first, _) = given(&f, &m, &d, &stopped, true).await;
    let mut work = vec![first];
    let mut stop = 220;
    // The first attempt and three more, each ending before its turn.
    for n in 1..=3 {
        let reply = retry(
            &f,
            &lost,
            &previous,
            n,
            attempts::result(&previous, "stopped", stop),
        )
        .await
        .unwrap();
        assert_eq!(
            (reply.demand.state.as_str(), reply.demand.retry),
            ("retry_offered", Some(n)),
            "retry {n}"
        );
        let (started, given, _) = rerun(
            &f,
            &m,
            &reply.demand,
            &previous,
            &reply.acknowledgement.receipt_digest,
            n,
            stop + 1,
        )
        .await;
        assert_eq!(given.event_id, "$capped");
        previous = started;
        work.push(given);
        stop += 20;
    }
    // The fourth stop ends the message: no fifth attempt.
    let reply = retry(
        &f,
        &lost,
        &previous,
        4,
        attempts::result(&previous, "stopped", stop),
    )
    .await
    .unwrap();
    assert_eq!(reply.demand.state, "head_changed");
    assert_eq!(
        block_payload(&f, &lost.offer.demand_id).await.as_deref(),
        Some("\"retry_exhausted\"")
    );
    assert_eq!(retries_opened(&f, &lost.offer.demand_id).await, 3);
    for w in &work {
        assert_eq!(released(&f, &w.id).await, 1);
    }
    assert_eq!(work_items(&f, "$capped").await, 4);
    assert_eq!(
        status_of(&f, &work[0].room_id, &f.outsider.p, "$capped").await,
        json!(["blocked", "head_changed", "retry_exhausted", true])
    );
    assert_eq!(next_offer(&f).await.offer.event_id, "$capped_next");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_demand_v2_stop_after_the_turn_began_is_recorded_and_offers_nothing_again() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "began_then_stopped").await;
    let (lost, issued, first, input) = given(&f, &m, &d, &stopped, true).await;
    f.domain
        .start_current_work(&f.owner.p, &f.policy, input, &m.gateway)
        .await
        .unwrap();
    // The process stops after its turn began, and its executor asks for a
    // retry with the stop: the stop is recorded, nothing is offered again.
    let evidence = attempts::result(&issued, "stopped", 220);
    let reply = retry(&f, &lost, &issued, 1, evidence.clone())
        .await
        .unwrap();
    assert_eq!(reply.acknowledgement.state, "stopped");
    assert_eq!(
        (reply.demand.state.as_str(), reply.demand.retry),
        ("bound", None)
    );
    assert_eq!(
        runtime_state(&f, &issued.command.command_id).await,
        ("stopped".into(), true)
    );
    assert_eq!(stop_receipts(&f, &issued.command.command_id).await, 1);
    assert_eq!(retries_opened(&f, &lost.offer.demand_id).await, 0);
    assert_eq!(released(&f, &first.id).await, 0);
    assert_eq!(work_items(&f, "$began_then_stopped").await, 1);
    assert_eq!(
        fence_says(&fence_facts(&f, &issued.command.command_id).await),
        ("computer".into(), 0, 0)
    );
    // A lost reply is replayed exactly.
    let again = retry(&f, &lost, &issued, 1, evidence).await.unwrap();
    assert_eq!(
        serde_json::to_value(&again).unwrap(),
        serde_json::to_value(&reply).unwrap()
    );
    assert!(logged(&lost.offer.demand_id).is_empty());
}
