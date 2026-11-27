//! What a message's status says once it ended without a turn, read from every
//! record that can end it; whether its Agent serves, by the check that the
//! polls make; the polls past what they cannot offer; and the time bound past
//! an attempt it cannot stop.
use super::*;

/// How v2 ended a message before a budget was applied to its Agent.
#[derive(Clone, Copy, Debug)]
enum V2End {
    /// Its start reported a process, which the time bound then gave up.
    Interrupted,
    /// Its start never reported one: the time bound stopped it, and the
    /// computer asked for no retry.
    NeverReported,
    /// Its usage could not be represented.
    Unrepresentable,
}

/// A budget applied after v2 started or ended the oldest message retires v2
/// for that Agent: the message can be neither bound there nor started under
/// the budget. It ends (one that v2 ended keeps that end), its sender may
/// send it again where a copy could run, and the budget queue offers the
/// next message instead of waiting behind it.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn budget_queue_moves_past_a_message_v2_ended_before_the_budget() {
    for end in [
        V2End::Interrupted,
        V2End::NeverReported,
        V2End::Unrepresentable,
    ] {
        let (mut f, r, admin, _a, m) = Box::pin(setup()).await;
        f.policy = f
            .policy
            .clone()
            .with_agent_budget_deltas(true)
            .with_shared_budget_execution(true);
        let (d, _) = qualify(&f).await;
        let stopped = attempts::stop(&f, &d, 201).await;
        // Event IDs are unique across the cases: they share one database.
        let (ended, next) = (format!("v2_ended_{end:?}"), format!("v2_next_{end:?}"));
        retain(&f, &r, &m, &ended).await;
        retain(&f, &r, &m, &next).await;
        let (ended, next) = (format!("${ended}"), format!("${next}"));
        let lost = demand(&f).await;
        assert_eq!(lost.offer.event_id, ended);
        if let V2End::Unrepresentable = end {
            f.domain
                .block_shared_demand(
                    &f.owner.p,
                    &lost.offer.demand_id,
                    SharedDemandBlockInput {
                        demand_digest: lost.demand_digest.clone(),
                        expected_control_revision: lost.offer.control_revision,
                        reason: "usage_unrepresentable".into(),
                        budget: None,
                        retries: false,
                        reoffers: false,
                    },
                )
                .await
                .unwrap();
        } else {
            let issued = start_lapsed(&f, &m, &lost, start_input(&lost, &d, &stopped)).await;
            if let V2End::Interrupted = end {
                f.domain
                    .set_management_test_clock(issued.command.issued_at_ms + 1_000)
                    .await
                    .unwrap();
                record(&f, &issued, attempts::result(&issued, "qualified", 210))
                    .await
                    .unwrap();
            }
            f.domain
                .set_management_test_clock(issued.command.expires_at_ms + 30_000 + 180_000 + 1_000)
                .await
                .unwrap();
            for _ in 0..64 {
                if runtime_state(&f, &issued.command.command_id).await.0 == "stopped" {
                    break;
                }
                f.domain.sweep_shared_attempt_timeouts().await.unwrap();
            }
            f.domain.reset_management_test_clock().await.unwrap();
            assert_eq!(
                runtime_state(&f, &issued.command.command_id).await.0,
                "stopped",
                "{end:?}"
            );
        }
        let room = work::message(&f, &r, "scope").await.room_id;
        // How the message reads once it ended, and whether its sender may
        // send it again. A block on usage without a budget ends it at once
        // and keeps its reason; a copy would be blocked the same way, since
        // this Agent has no periods to turn. Reading writes nothing.
        let (read_end, again) = match end {
            V2End::Unrepresentable => (
                json!(["blocked", "usage_unrepresentable", "usage_unrepresentable"]),
                false,
            ),
            _ => (json!(["blocked", "head_changed", "head_changed"]), true),
        };
        let reads = |again: bool| {
            let mut read = read_end.clone();
            read.as_array_mut().unwrap().push(json!(again));
            read
        };
        if let V2End::Unrepresentable = end {
            assert_eq!(
                status_of(&f, &room, &f.outsider.p, &ended).await,
                reads(false)
            );
            // Its time running out changes nothing.
            f.domain
                .set_management_test_clock(db_now(&f).await + 86_400_000 + 1_000)
                .await
                .unwrap();
            let late = status_of(&f, &room, &f.outsider.p, &ended).await;
            f.domain.reset_management_test_clock().await.unwrap();
            assert_eq!(late, reads(false));
        }
        budget_tests::apply_delta(&f, &r, &admin, 25, 230).await;
        // Read before any budget poll, the message the budget retired reads
        // as ended already, as the poll then records it; one v2 had ended
        // keeps its end.
        assert_eq!(
            status_of(&f, &room, &f.outsider.p, &ended).await,
            reads(again),
            "{end:?}"
        );
        let offered = f
            .domain
            .poll_shared_budget_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .demand
            .unwrap_or_else(|| panic!("{end:?}: the next message is offered"));
        assert_eq!(offered.offer.event_id, next, "{end:?}");
        for (reader, again) in [(&f.outsider.p, again), (&f.owner.p, false)] {
            assert_eq!(
                status_of(&f, &room, reader, &ended).await,
                reads(again),
                "{end:?}"
            );
        }
        assert_eq!(
            status_of(&f, &room, &f.outsider.p, &next).await,
            json!(["pending", null, null, false]),
            "{end:?}"
        );
    }
}

/// The status lists the reader's 16 newest messages in the room, newest
/// first: their own, and every message to an Agent they own.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn chat_status_lists_the_readers_sixteen_newest_messages() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    for n in 0..17 {
        retain(&f, &r, &m, &format!("cut_{n:02}")).await;
    }
    let room = work::message(&f, &r, "scope").await.room_id;
    let newest: Vec<String> = (1..17).rev().map(|n| format!("$cut_{n:02}")).collect();
    for reader in [&f.outsider.p, &f.owner.p] {
        let query = serde_json::from_value(json!({"roomId": room})).unwrap();
        let read = f.domain.shared_approvals(reader, query).await.unwrap();
        let shown: Vec<String> = read["executions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["eventId"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(shown, newest);
    }
}

/// Moves the end of the allocation that `r`'s Agent is reserved under to
/// `at`, and returns the end it had.
async fn allocation_ends(f: &Fixture, r: &ManagedAgentRequest, at: i64) -> i64 {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let before = sql_query("SELECT al.expires_at_ms AS count FROM hagency_agent_v1.management_allocations al JOIN hagency_agent_v1.management_agent_requests r ON r.allocation_id=al.id WHERE r.id=$1")
        .bind::<Text, _>(&r.id)
        .get_result::<Count>(&mut db)
        .await
        .unwrap()
        .count;
    sql_query("UPDATE hagency_agent_v1.management_allocations SET expires_at_ms=$2 WHERE id=(SELECT allocation_id FROM hagency_agent_v1.management_agent_requests WHERE id=$1)")
        .bind::<Text, _>(&r.id)
        .bind::<BigInt, _>(at)
        .execute(&mut db)
        .await
        .unwrap();
    before
}

/// Points the intake of `event` at a room binding of its Agent that was left,
/// as leaving or pausing an additional room leaves a message received there.
/// This fixture's Agent has no additional room, so the binding is written
/// here, with the rules that guard both rows off for these two writes.
async fn binding_left(f: &Fixture, a: &PendingManagedAssignment, event: &str) {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!(
        "ALTER TABLE hagency_agent_v1.shared_agent_rooms DISABLE TRIGGER USER;\
         INSERT INTO hagency_agent_v1.shared_agent_rooms(id,assignment_id,agent_id,project_id,room_id,state) SELECT 'srb_left_'||a.id,a.id,a.agent_id,r.domain_project_id,r.room_id,'left' FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_requests r ON r.id=a.request_id WHERE a.id='{id}';\
         ALTER TABLE hagency_agent_v1.shared_agent_rooms ENABLE TRIGGER USER;\
         ALTER TABLE hagency_agent_v1.management_shared_intake DISABLE TRIGGER USER;\
         UPDATE hagency_agent_v1.management_shared_intake SET room_binding_id='srb_left_'||assignment_id,room_binding_generation=1 WHERE assignment_id='{id}' AND event_id='{event}';\
         ALTER TABLE hagency_agent_v1.management_shared_intake ENABLE TRIGGER USER",
        id = a.id
    ))
    .await
    .unwrap();
}

/// An Agent whose reservation's time is over serves no new message, by the
/// one check that a new message meets when it arrives and that the polls
/// make: its sender is not offered to send a message again, a new message is
/// blocked when it arrives, and the poll offers none of its messages. Each
/// reads the time now: with the end moved back, the Agent serves again (no
/// read of its room membership comes between, as that read withdraws a
/// reservation whose time is over for good). A replaced ledger withdraws it.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn an_agent_whose_reservation_time_is_over_is_offered_nothing() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    attempts::stop(&f, &d, 201).await;
    let room = work::message(&f, &r, "scope").await.room_id;
    // A message stopped while it waited has ended, and its sender may send
    // it again while the Agent serves.
    retain(&f, &r, &m, "over_stopped").await;
    let revision = f
        .domain
        .shared_run_control(&f.outsider.p, &a.id)
        .await
        .unwrap()
        .revision;
    for (command, revision, mode) in [
        ("over_pause", revision, "paused"),
        ("over_resume", revision + 1, "running"),
    ] {
        f.domain
            .change_shared_run_control(
                &f.outsider.p,
                &f.policy,
                &a.id,
                intake_tests::change(command, revision, mode),
                &m.gateway,
            )
            .await
            .unwrap();
    }
    let stopped = |again: bool| json!(["cancelled", "explicit_stop", "explicit_stop", again]);
    let read = async || status_of(&f, &room, &f.outsider.p, "$over_stopped").await;
    assert_eq!(read().await, stopped(true));
    retain(&f, &r, &m, "over_waits").await;
    let late = work::message(&f, &r, "over_arrived").await;
    // The allocation that the reservation stands on ends.
    let end = allocation_ends(&f, &r, db_now(&f).await - 1).await;
    assert_eq!(read().await, stopped(false));
    f.domain
        .route_shared_event(&f.policy, &late, received(&f).await, &m.gateway)
        .await
        .unwrap();
    assert_eq!(
        status_of(&f, &room, &f.outsider.p, "$over_arrived").await,
        json!([
            "blocked",
            "assignment_authority_changed",
            "assignment_authority_changed",
            false
        ])
    );
    assert!(
        f.domain
            .poll_shared_demand(&f.owner.p, &f.policy, SharedDemandPollInput::default())
            .await
            .unwrap()
            .demand
            .is_none(),
        "the poll passes over the Agent"
    );
    allocation_ends(&f, &r, end).await;
    assert_eq!(read().await, stopped(true));
    assert_eq!(demand(&f).await.offer.event_id, "$over_waits");
    // A computer that says its ledger is new ends what the old one held.
    f.domain
        .declare_executor_ledger(
            &f.owner.p,
            &f.policy,
            crate::management_agent_executor_types::ExecutorLedgerInput {
                ledger_id: "ab".repeat(32),
                fresh: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(read().await, stopped(false));
}

/// An Agent that the budget queue cannot offer a message for now does not
/// hold back the other Agents: the poll passes over it, as the v2 poll does,
/// and offers the next Agent's message.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn budget_poll_passes_over_an_agent_it_cannot_offer_for() {
    let (mut f, r, admin, a, m) = Box::pin(setup()).await;
    f.policy = f
        .policy
        .clone()
        .with_agent_budget_deltas(true)
        .with_shared_budget_execution(true);
    let (d, _) = qualify(&f).await;
    attempts::stop(&f, &d, 201).await;
    let (second, b) = Box::pin(work::another_assignment(&f, &r, &admin, &m, 2)).await;
    let runtime = shared_poll(&f).await;
    assert_eq!(runtime.command.assignment_id, b.id);
    save(&f, &runtime, "qualified", 310).await.unwrap();
    attempts::stop(&f, &runtime, 311).await;
    retain(&f, &r, &m, "passed_a").await;
    retain(&f, &second, &m, "passed_b").await;
    budget_tests::apply_delta(&f, &r, &admin, 25, 330).await;
    budget_tests::apply_delta(&f, &second, &admin, 25, 340).await;
    // The poll goes by assignment id. The first Agent's reservation is
    // withdrawn, so no message can be offered for it.
    let (first, other) = if a.id < b.id {
        (&r, "$passed_b")
    } else {
        (&second, "$passed_a")
    };
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_agent_intents SET state='fenced' WHERE request_id=$1 AND state<>'fenced'")
        .bind::<Text, _>(&first.id)
        .execute(&mut db)
        .await
        .unwrap();
    let offered = f
        .domain
        .poll_shared_budget_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .demand
        .expect("the other Agent's message is offered");
    assert_eq!(offered.offer.event_id, other);
}

/// The budget queue passes over the messages it can never offer: one that
/// was blocked when it arrived, and one whose room binding is gone. The
/// latter reads as ended as soon as it is read, and the same once a poll has
/// recorded its end.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn budget_queue_passes_over_messages_it_can_never_offer() {
    let (mut f, r, admin, a, m) = Box::pin(setup()).await;
    f.policy = f
        .policy
        .clone()
        .with_agent_budget_deltas(true)
        .with_shared_budget_execution(true);
    let (d, _) = qualify(&f).await;
    attempts::stop(&f, &d, 201).await;
    let room = work::message(&f, &r, "scope").await.room_id;
    // Blocked when it arrived: the reservation's allocation had ended. Its
    // end is moved back before anything reads the Agent's room membership.
    let arrived = work::message(&f, &r, "never_arrived").await;
    let end = allocation_ends(&f, &r, db_now(&f).await - 1).await;
    f.domain
        .route_shared_event(&f.policy, &arrived, received(&f).await, &m.gateway)
        .await
        .unwrap();
    allocation_ends(&f, &r, end).await;
    retain(&f, &r, &m, "never_room").await;
    binding_left(&f, &a, "$never_room").await;
    retain(&f, &r, &m, "never_next").await;
    let gone = json!([
        "cancelled",
        "room_authority_changed",
        "room_authority_changed",
        true
    ]);
    assert_eq!(
        status_of(&f, &room, &f.outsider.p, "$never_room").await,
        gone
    );
    budget_tests::apply_delta(&f, &r, &admin, 25, 230).await;
    let offered = f
        .domain
        .poll_shared_budget_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .demand
        .expect("the next message is offered");
    assert_eq!(offered.offer.event_id, "$never_next");
    assert_eq!(
        status_of(&f, &room, &f.outsider.p, "$never_arrived").await,
        json!([
            "blocked",
            "assignment_authority_changed",
            "assignment_authority_changed",
            true
        ])
    );
    // The v2 poll records the end that the status read.
    f.domain
        .poll_shared_demand(&f.owner.p, &f.policy, SharedDemandPollInput::default())
        .await
        .unwrap();
    let history = f
        .domain
        .shared_intake_history(
            &f.outsider.p,
            &a.id,
            crate::management_shared_intake_types::SharedIntakeQuery { after: None },
        )
        .await
        .unwrap();
    assert!(
        history
            .items
            .iter()
            .any(|i| i.event_id == "$never_room" && i.state == "cancelled")
    );
    assert_eq!(
        status_of(&f, &room, &f.outsider.p, "$never_room").await,
        gone
    );
}

/// Two Agents' attempts, issued with their start windows already over, and a
/// test rule that refuses to stop the one a sweep takes first (it goes by
/// id). Returns that attempt and the other one.
async fn one_attempt_refuses_its_stop(
    f: &Fixture,
    r: &ManagedAgentRequest,
    admin: &Actor,
    a: &PendingManagedAssignment,
    m: &MatrixFixture,
) -> (String, String) {
    let (first, _) = qualify(f).await;
    let stop = attempts::stop(f, &first, 201).await;
    let one = issue_lapsed(f, m, attempts::start(&first, &stop)).await;
    let (_, b) = Box::pin(work::another_assignment(f, r, admin, m, 2)).await;
    let second = shared_poll(f).await;
    assert_eq!(second.command.assignment_id, b.id);
    save(f, &second, "qualified", 310).await.unwrap();
    let stop = attempts::stop(f, &second, 311).await;
    let two = issue_lapsed(f, m, attempts::start(&second, &stop)).await;
    assert_eq!(one.command.assignment_id, a.id);
    let mut ids = [one.command.command_id, two.command.command_id];
    ids.sort();
    let [refused, other] = ids;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!(
        "CREATE FUNCTION hagency_agent_v1.fixture_refuse_stop() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture_refuse_stop'; END; $$;\
         CREATE TRIGGER fixture_refuse_stop BEFORE UPDATE ON hagency_agent_v1.management_shared_runtimes FOR EACH ROW WHEN (OLD.id='{refused}' AND NEW.state='stopped') EXECUTE FUNCTION hagency_agent_v1.fixture_refuse_stop()"
    ))
    .await
    .unwrap();
    (refused, other)
}
async fn allow_every_stop(f: &Fixture) {
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("DROP TRIGGER fixture_refuse_stop ON hagency_agent_v1.management_shared_runtimes;DROP FUNCTION hagency_agent_v1.fixture_refuse_stop()")
        .await
        .unwrap();
}

/// Nor does such an attempt hold back its computer's attempt poll, which
/// first gives up that computer's due attempts in its own transaction: the
/// poll stops the other attempt and is answered, and a later poll stops the
/// first once it can.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn attempt_poll_goes_on_past_an_attempt_it_cannot_stop() {
    let (mut f, r, admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.clone().with_shared_successors(true);
    let (refused, other) = one_attempt_refuses_its_stop(&f, &r, &admin, &a, &m).await;
    let due = db_now(&f).await + 180_000 + 1_000;
    f.domain.set_management_test_clock(due).await.unwrap();
    let poll = async || {
        f.domain
            .poll_shared_attempt(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .map(|_| ())
    };
    let polled = poll().await;
    let states = (
        runtime_state(&f, &other).await.0,
        runtime_state(&f, &refused).await.0,
    );
    allow_every_stop(&f).await;
    assert!(polled.is_ok(), "the poll is answered: {:?}", polled.err());
    assert_eq!(
        states.0, "stopped",
        "the other attempt is stopped all the same"
    );
    assert_ne!(states.1, "stopped");
    poll().await.unwrap();
    assert_eq!(runtime_state(&f, &refused).await.0, "stopped");
    f.domain.reset_management_test_clock().await.unwrap();
}

/// The membership worker's pass counts as failed when the time bound cannot
/// stop an attempt (`shared_attempt_timeouts_failed` in worker health). The
/// line it logs cannot be read by a test: no log capture is set up.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn worker_health_reports_an_attempt_the_time_bound_cannot_stop() {
    let (mut f, r, admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.clone().with_shared_successors(true);
    one_attempt_refuses_its_stop(&f, &r, &admin, &a, &m).await;
    let due = db_now(&f).await + 180_000 + 1_000;
    f.domain.set_management_test_clock(due).await.unwrap();
    let runtime = crate::worker_runtime::WorkerRuntime::default();
    let worker = crate::workers::start_cleanup_controlled(
        f.domain.clone(),
        m.gateway.clone(),
        m.matrix.clone(),
        Some(f.policy.clone()),
        runtime.clone(),
    );
    let failed = tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            let health = serde_json::to_value(runtime.health()).unwrap();
            if health["queues"].as_array().unwrap().iter().any(|q| {
                q["name"] == "membership" && q["error"] == "shared_attempt_timeouts_failed"
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    worker.abort();
    let _ = worker.await;
    allow_every_stop(&f).await;
    f.domain.reset_management_test_clock().await.unwrap();
    assert!(failed.is_ok(), "the membership pass reports the failure");
}

/// Issue `input` with its start window already over in real time, so that it
/// is due for the time bound once the grace has passed.
async fn issue_lapsed(
    f: &Fixture,
    m: &MatrixFixture,
    input: SharedAttemptStartInput,
) -> SharedAttemptDelivery {
    f.domain
        .set_management_test_clock(db_now(f).await - 31_000)
        .await
        .unwrap();
    let issued = f
        .domain
        .start_shared_attempt(&f.owner.p, &f.policy, input, &m.gateway)
        .await;
    f.domain.reset_management_test_clock().await.unwrap();
    issued.unwrap()
}

/// An attempt the time bound cannot stop does not hold back another one: the
/// sweep stops the other, fails the pass, and stops the first once it can.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn sweep_goes_on_past_an_attempt_it_cannot_stop() {
    let (mut f, r, admin, a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.clone().with_shared_successors(true);
    let (refused, other) = one_attempt_refuses_its_stop(&f, &r, &admin, &a, &m).await;
    let due = db_now(&f).await + 180_000 + 1_000;
    f.domain.set_management_test_clock(due).await.unwrap();
    let swept = f.domain.sweep_shared_attempt_timeouts().await;
    let states = (
        runtime_state(&f, &other).await.0,
        runtime_state(&f, &refused).await.0,
    );
    allow_every_stop(&f).await;
    assert!(
        swept.is_err(),
        "the pass reports the attempt it could not stop"
    );
    assert_eq!(
        states.0, "stopped",
        "the other attempt is stopped all the same"
    );
    assert_ne!(states.1, "stopped");
    f.domain.sweep_shared_attempt_timeouts().await.unwrap();
    assert_eq!(runtime_state(&f, &refused).await.0, "stopped");
    f.domain.reset_management_test_clock().await.unwrap();
}
