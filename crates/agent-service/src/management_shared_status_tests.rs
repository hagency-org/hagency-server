//! What a message's status says once it ended without a turn, read from every
//! record that can end it; whether its Agent serves, by the check that the
//! polls make; the polls past what they cannot offer; and the time bound past
//! an attempt it cannot stop.
use super::*;

/// How v2 left a message before a budget was applied to its Agent.
#[derive(Clone, Copy, Debug)]
enum V2End {
    /// Its start reported a process, which the time bound then gave up: it
    /// ended (`time_bound_stopped`).
    Interrupted,
    /// Its start never reported one: the time bound stopped it, and the
    /// computer asked for no retry. It still waits for the computer's
    /// evidence, and the budget ends it (`budget_applied`).
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
        // send it again. A block on usage without a budget ends it at once,
        // reads as cancelled and keeps its cause as the end; a copy would be
        // blocked the same way, since this Agent has no periods to turn.
        // Reading writes nothing.
        let (read_end, again) = match end {
            V2End::Unrepresentable => (json!(["cancelled", null, "usage_unrepresentable"]), false),
            V2End::Interrupted => (
                json!(["blocked", "head_changed", "time_bound_stopped"]),
                true,
            ),
            V2End::NeverReported => (json!(["blocked", "head_changed", "budget_applied"]), true),
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
pub(super) async fn allocation_ends(f: &Fixture, r: &ManagedAgentRequest, at: i64) -> i64 {
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

/// A block on usage under an applied budget ends the message at once, as it
/// does without one. The budget poll records that end as `head_changed`, the
/// end every computer keeps of this queue, also once the message's time is
/// over, and offers the next message; the block keeps its reason, which the
/// status reads as the end from the moment the block is recorded, and keeps
/// when a stop ends the intake later. Its sender is not offered to send it
/// again: a copy would be blocked the same way, since this Agent has no
/// periods to turn.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_block_on_usage_ends_the_message_in_the_budget_queue_at_once() {
    use crate::management_shared_budget_types::{
        SharedBudgetDemandBlockInput, SharedBudgetDemandStatus,
    };
    let (mut f, r, admin, a, m) = Box::pin(setup()).await;
    f.policy = f
        .policy
        .clone()
        .with_agent_budget_deltas(true)
        .with_shared_budget_execution(true);
    let (d, _) = qualify(&f).await;
    attempts::stop(&f, &d, 201).await;
    for id in ["usage_first", "usage_second", "usage_next"] {
        retain(&f, &r, &m, id).await;
    }
    budget_tests::apply_delta(&f, &r, &admin, 25, 230).await;
    let room = work::message(&f, &r, "scope").await.room_id;
    let poll = async || {
        f.domain
            .poll_shared_budget_demand(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .demand
    };
    let block = async |d: &SharedBudgetDemandStatus| {
        let blocked = f
            .domain
            .block_shared_budget_demand(
                &f.owner.p,
                &d.offer.demand_id,
                SharedBudgetDemandBlockInput {
                    demand_digest: d.demand_digest.clone(),
                    expected_control_revision: d.offer.control_revision,
                    original_block: d.original_block.clone(),
                    execution_budget: d.execution_budget.clone(),
                    reason: "usage_unrepresentable".into(),
                    budget: None,
                },
            )
            .await
            .unwrap();
        // The answer that the computer checks names the block.
        assert_eq!(blocked.state, "usage_unrepresentable");
    };
    // It reads as cancelled, with its cause as its end and no other reason:
    // Hagency Desktop 0.5.0-rc.19 reads the reason `usage_unrepresentable`
    // as a wait for usage to be confirmed, and `cancelled` as an end.
    let ended = json!(["cancelled", null, "usage_unrepresentable", false]);
    let first = poll().await.expect("the oldest message is offered");
    assert_eq!(first.offer.event_id, "$usage_first");
    block(&first).await;
    // Ended once the block is recorded, before any poll, whatever its time.
    assert_eq!(
        status_of(&f, &room, &f.outsider.p, "$usage_first").await,
        ended
    );
    f.domain
        .set_management_test_clock(db_now(&f).await + 86_400_000 + 1_000)
        .await
        .unwrap();
    let late = status_of(&f, &room, &f.outsider.p, "$usage_first").await;
    f.domain.reset_management_test_clock().await.unwrap();
    assert_eq!(late, ended);
    let second = poll().await.expect("the next message is offered");
    assert_eq!(second.offer.event_id, "$usage_second");
    // Blocked on usage, and its time over before a poll reads it: the poll
    // records the same end, and offers the next message.
    block(&second).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!(
        "ALTER TABLE hagency_agent_v1.management_shared_intake DISABLE TRIGGER USER;\
         UPDATE hagency_agent_v1.management_shared_intake SET expires_at_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1 WHERE assignment_id='{id}' AND event_id='$usage_second';\
         ALTER TABLE hagency_agent_v1.management_shared_intake ENABLE TRIGGER USER",
        id = a.id
    ))
    .await
    .unwrap();
    let next = poll().await.expect("the message after them is offered");
    assert_eq!(next.offer.event_id, "$usage_next");
    for blocked in [&first, &second] {
        let recorded = sql_query("SELECT t.reason||' '||b.reason AS payload FROM hagency_agent_v1.management_shared_budget_demand_terminals t JOIN hagency_agent_v1.management_shared_budget_demand_blocks b ON b.demand_id=t.demand_id WHERE t.demand_id=$1")
            .bind::<Text, _>(&blocked.offer.demand_id)
            .get_result::<Payload>(&mut db)
            .await
            .unwrap()
            .payload;
        assert_eq!(recorded, "head_changed usage_unrepresentable");
        let kept = f
            .domain
            .recover_shared_budget_demand(
                &f.owner.p,
                &blocked.offer.demand_id,
                SharedDemandRecoverInput {
                    demand_digest: blocked.demand_digest.clone(),
                    retries: false,
                    reoffers: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(kept.state, "head_changed");
    }
    for reader in [&f.outsider.p, &f.owner.p] {
        for event in ["$usage_first", "$usage_second"] {
            assert_eq!(status_of(&f, &room, reader, event).await, ended, "{event}");
        }
    }
    assert_eq!(
        status_of(&f, &room, &f.outsider.p, "$usage_next").await,
        json!(["pending", null, null, false])
    );
    // A stop ends the message that waits, and its sender may send it again
    // once the Agent runs; the ended ones keep their end.
    let revision = f
        .domain
        .shared_run_control(&f.outsider.p, &a.id)
        .await
        .unwrap()
        .revision;
    for (command, revision, mode) in [
        ("usage_pause", revision, "paused"),
        ("usage_resume", revision + 1, "running"),
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
    assert_eq!(
        status_of(&f, &room, &f.outsider.p, "$usage_next").await,
        json!(["cancelled", "explicit_stop", "explicit_stop", true])
    );
    for event in ["$usage_first", "$usage_second"] {
        assert_eq!(
            status_of(&f, &room, &f.outsider.p, event).await,
            ended,
            "{event}"
        );
    }
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

/// Nor does such an attempt hold back a message given back whose wait is
/// over: the sweep ends that message all the same, and reports the attempt.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn sweep_ends_a_message_given_back_past_an_attempt_it_cannot_stop() {
    let (f, r, admin, _a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    let stopped = attempts::stop(&f, &d, 201).await;
    retain(&f, &r, &m, "swept_given_back").await;
    let (lost, issued, first, _) = given(&f, &m, &d, &stopped, true).await;
    record(&f, &issued, attempts::result(&issued, "stopped", 220))
        .await
        .unwrap();
    assert_eq!(released(&f, &first.id).await, 1);
    // Another Agent has an attempt that the time bound cannot stop.
    let (_, b) = Box::pin(work::another_assignment(&f, &r, &admin, &m, 2)).await;
    let second = shared_poll(&f).await;
    assert_eq!(second.command.assignment_id, b.id);
    save(&f, &second, "qualified", 310).await.unwrap();
    let stop = attempts::stop(&f, &second, 311).await;
    let refused = issue_lapsed(&f, &m, attempts::start(&second, &stop))
        .await
        .command
        .command_id;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!(
        "CREATE FUNCTION hagency_agent_v1.fixture_refuse_stop() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture_refuse_stop'; END; $$;\
         CREATE TRIGGER fixture_refuse_stop BEFORE UPDATE ON hagency_agent_v1.management_shared_runtimes FOR EACH ROW WHEN (OLD.id='{refused}' AND NEW.state='stopped') EXECUTE FUNCTION hagency_agent_v1.fixture_refuse_stop()"
    ))
    .await
    .unwrap();
    // Past both the attempt's grace and the message's bound.
    let at = (db_now(&f).await + 180_000 + 1_000)
        .max(issued.command.expires_at_ms + 30_000 + 180_000 + 180_000);
    f.domain.set_management_test_clock(at).await.unwrap();
    let swept = f.domain.sweep_shared_attempt_timeouts().await;
    let (payload, state) = (
        block_payload(&f, &lost.offer.demand_id).await,
        runtime_state(&f, &refused).await.0,
    );
    db.batch_execute("DROP TRIGGER fixture_refuse_stop ON hagency_agent_v1.management_shared_runtimes;DROP FUNCTION hagency_agent_v1.fixture_refuse_stop()")
        .await
        .unwrap();
    f.domain.reset_management_test_clock().await.unwrap();
    assert!(
        swept.is_err(),
        "the pass reports the attempt it could not stop"
    );
    assert_ne!(state, "stopped");
    assert_eq!(
        payload.as_deref(),
        Some("\"retry_lapsed\""),
        "the message given back ends all the same"
    );
    assert_eq!(recovered(&f, &lost).await.state, "head_changed");
}
/// The row that `reader` reads of `event` in `room`, whole.
async fn row_of(f: &Fixture, room: &str, reader: &Principal, event: &str) -> Value {
    let query = serde_json::from_value(json!({"roomId": room})).unwrap();
    let read = f.domain.shared_approvals(reader, query).await.unwrap();
    read["executions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["eventId"] == event)
        .cloned()
        .unwrap_or_else(|| panic!("{event} is among the reader's newest messages"))
}

/// Whether `reader` may send `event` again, and why not when only its Agent
/// stands in the way.
async fn offer_of(f: &Fixture, room: &str, reader: &Principal, event: &str) -> Value {
    let row = row_of(f, room, reader, event).await;
    // What the answer is worked out from stays on the server.
    for kept in ["fromViewer", "agentAvailable", "agentRuns", "copyCanRun"] {
        assert!(row.get(kept).is_none(), "{kept}");
    }
    json!([row["canSendAgain"], row["notOfferedBecause"]])
}

/// A message that ended without a turn tells its sender why it is not
/// offered when nothing but its Agent stands in the way: the Agent does not
/// run, it cannot serve a new message, or the status has no Matrix ID to name
/// it by. In the last case it is not offered at all, as the client could not
/// say which Agent to send it to. Nobody else is told, and a message that
/// still waits says nothing.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_message_not_offered_for_its_agent_says_why() {
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    attempts::stop(&f, &d, 201).await;
    let room = work::message(&f, &r, "scope").await.room_id;
    retain(&f, &r, &m, "why_ended").await;
    let read = async |reader: &Principal| offer_of(&f, &room, reader, "$why_ended").await;
    let said = |again: bool, why: Option<&str>| json!([again, why]);
    assert_eq!(read(&f.outsider.p).await, said(false, None), "it waits");
    let revision = f
        .domain
        .shared_run_control(&f.outsider.p, &a.id)
        .await
        .unwrap()
        .revision;
    let control = async |command: &str, revision: i64, mode: &str| {
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
    };
    // Stopped while it waited: it has ended, and the Agent does not run.
    control("why_pause", revision, "paused").await;
    assert_eq!(
        read(&f.outsider.p).await,
        said(false, Some("agent_stopped"))
    );
    assert_eq!(read(&f.owner.p).await, said(false, None), "not its sender");
    control("why_resume", revision + 1, "running").await;
    assert_eq!(read(&f.outsider.p).await, said(true, None));
    // The Agent runs, but its reservation's time is over.
    let end = allocation_ends(&f, &r, db_now(&f).await - 1).await;
    assert_eq!(
        read(&f.outsider.p).await,
        said(false, Some("agent_unavailable"))
    );
    assert_eq!(read(&f.owner.p).await, said(false, None), "not its sender");
    allocation_ends(&f, &r, end).await;
    assert_eq!(read(&f.outsider.p).await, said(true, None));
    // No Matrix ID for the Agent: the row cannot name it.
    let hide = async |from: &str, to: &str| {
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        db.batch_execute(&format!(
            "SET session_replication_role=replica;\
             UPDATE hagency_agent_v1.management_agent_matrix SET assignment_id='{to}' WHERE assignment_id='{from}';\
             SET session_replication_role=origin"
        ))
        .await
        .unwrap();
    };
    let hidden = format!("{}_hidden", a.id);
    hide(&a.id, &hidden).await;
    let unnamed = row_of(&f, &room, &f.outsider.p, "$why_ended").await;
    hide(&hidden, &a.id).await;
    assert_eq!(unnamed["agentMxid"], Value::Null);
    assert_eq!(
        json!([unnamed["canSendAgain"], unnamed["notOfferedBecause"]]),
        said(false, Some("agent_unnamed"))
    );
    assert_eq!(read(&f.outsider.p).await, said(true, None));
}

/// The status says that a message's 24 hours are over only one claim window
/// after they are. A poll binds a message only while its own clock is before
/// that end, and commits within the claim window of reading it; so a message
/// whose sender is offered to send it again is one no poll can still run. An
/// end that a poll recorded reads at once: nothing binds a message after it.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_message_reads_as_expired_one_claim_window_after_its_time() {
    use crate::identity::CLAIM_WINDOW_MS;
    let (f, r, _admin, a, m) = Box::pin(setup()).await;
    let (d, _) = qualify(&f).await;
    attempts::stop(&f, &d, 201).await;
    let room = work::message(&f, &r, "scope").await.room_id;
    let received = retain(&f, &r, &m, "margin_waits").await;
    // Its time ends a millisecond after it arrived.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!(
        "ALTER TABLE hagency_agent_v1.management_shared_intake DISABLE TRIGGER USER;\
         UPDATE hagency_agent_v1.management_shared_intake SET expires_at_ms=received_at_ms+1 WHERE assignment_id='{id}' AND event_id='$margin_waits';\
         ALTER TABLE hagency_agent_v1.management_shared_intake ENABLE TRIGGER USER",
        id = a.id
    ))
    .await
    .unwrap();
    let ends = received + 1;
    let read_at = async |at: i64| {
        f.domain.set_management_test_clock(at).await.unwrap();
        let read = status_of(&f, &room, &f.outsider.p, "$margin_waits").await;
        f.domain.reset_management_test_clock().await.unwrap();
        read
    };
    let waits = json!(["pending", null, null, false]);
    let expired = json!(["expired", "event_expired", "event_expired", true]);
    assert_eq!(
        read_at(ends + CLAIM_WINDOW_MS - 1).await,
        waits,
        "within the claim window"
    );
    assert_eq!(
        read_at(ends + CLAIM_WINDOW_MS).await,
        expired,
        "a claim window after"
    );
    // Recorded by a poll, the end reads at once.
    assert_eq!(read_at(ends).await, waits);
    assert!(
        f.domain
            .poll_shared_demand(&f.owner.p, &f.policy, SharedDemandPollInput::default())
            .await
            .unwrap()
            .demand
            .is_none(),
        "the poll offers no message whose time is over"
    );
    assert_eq!(read_at(ends).await, expired, "recorded by the poll");
}

/// A message to two Agents is listed with both of its rows or not at all:
/// the status lists the reader's 16 newest messages, not 16 rows, so no
/// message is listed, nor offered, for only some of its Agents.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn chat_status_lists_each_message_with_all_of_its_agents() {
    let (f, r, admin, _a, m) = Box::pin(setup()).await;
    let (second, _b) = Box::pin(work::another_assignment(&f, &r, &admin, &m, 2)).await;
    let mut both = work::message(&f, &r, "cut_both").await;
    let other = work::message(&f, &second, "cut_other").await;
    assert_eq!(other.room_id, both.room_id, "one room");
    both.mentioned_mxids.extend(other.mentioned_mxids);
    f.domain
        .route_shared_event(&f.policy, &both, received(&f).await, &m.gateway)
        .await
        .unwrap();
    for n in 0..15 {
        retain(&f, &r, &m, &format!("cut_one_{n:02}")).await;
    }
    let listed = async |reader: &Principal| {
        let query = serde_json::from_value(json!({"roomId": both.room_id})).unwrap();
        let read = f.domain.shared_approvals(reader, query).await.unwrap();
        let events: Vec<String> = read["executions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["eventId"].as_str().unwrap().to_owned())
            .collect();
        let mut messages = events.clone();
        messages.dedup();
        (
            messages.len(),
            events.iter().filter(|e| *e == "$cut_both").count(),
        )
    };
    for reader in [&f.outsider.p, &f.owner.p] {
        assert_eq!(listed(reader).await, (16, 2), "both rows of the oldest");
    }
    retain(&f, &r, &m, "cut_one_15").await;
    for reader in [&f.outsider.p, &f.owner.p] {
        assert_eq!(listed(reader).await, (16, 0), "neither row");
    }
}

/// What `reader` reads of `event` once the database's clock is `at`.
async fn status_at(f: &Fixture, room: &str, reader: &Principal, event: &str, at: i64) -> Value {
    f.domain.set_management_test_clock(at).await.unwrap();
    let read = status_of(f, room, reader, event).await;
    f.domain.reset_management_test_clock().await.unwrap();
    read
}

/// Without an applied budget, a message blocked on allowance is offered
/// again only where a copy could have the allowance. While the Agent's known
/// use in its period still covers its reservation's tokens, a copy would be
/// blocked the same way, and end the same way a day later, so the message
/// is not offered once its time is over. It is while that use is below them,
/// as when the computer blocked it on tokens its work still held, and once a
/// budget has been applied.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_spent_allowance_without_a_budget_offers_no_copy() {
    use crate::identity::CLAIM_WINDOW_MS;
    let (mut f, r, admin, a, m) = Box::pin(setup()).await;
    let (d, activity) = qualify(&f).await;
    retain(&f, &r, &m, "spent_work").await;
    let w = f
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
        work_id: w.id.clone(),
        work_digest: allocation_digest(&w).unwrap(),
    };
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_current_work(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
    // The work uses exactly the reservation's tokens.
    let tokens = d.command.reserve_command.requested_tokens;
    let t = work::ticket(&f, &d, &w).await;
    let mut result = work::outcome(&d, &w, &t, 211, "completed");
    result.receipt.known_consumed_tokens = tokens;
    result.receipt.retained_tokens = Some(tokens);
    f.domain
        .record_shared_work_result(&f.owner.p, &f.policy, &w.id, result)
        .await
        .unwrap();
    attempts::stop(&f, &d, 212).await;
    let room = work::message(&f, &r, "scope").await.room_id;
    let received = retain(&f, &r, &m, "spent_blocked").await;
    assert!(
        f.domain
            .poll_shared_demand(&f.owner.p, &f.policy, SharedDemandPollInput::default())
            .await
            .unwrap()
            .demand
            .is_none(),
        "blocked on allowance"
    );
    assert_eq!(
        status_of(&f, &room, &f.outsider.p, "$spent_blocked").await,
        json!(["blocked", "budget_blocked", null, false])
    );
    // Its time ends a millisecond after it arrived.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!(
        "ALTER TABLE hagency_agent_v1.management_shared_intake DISABLE TRIGGER USER;\
         UPDATE hagency_agent_v1.management_shared_intake SET expires_at_ms=received_at_ms+1 WHERE assignment_id='{id}' AND event_id='$spent_blocked';\
         ALTER TABLE hagency_agent_v1.management_shared_intake ENABLE TRIGGER USER",
        id = a.id
    ))
    .await
    .unwrap();
    let over = received + 1 + CLAIM_WINDOW_MS;
    let ended = |again: bool| json!(["expired", "event_expired", "event_expired", again]);
    assert_eq!(
        status_at(&f, &room, &f.outsider.p, "$spent_blocked", over).await,
        ended(false),
        "the known use covers the reservation"
    );
    // As if the computer had blocked it on tokens its work still held: the
    // known use is below the reservation's tokens.
    let known = async |db: &mut AsyncPgConnection, used: i64| {
        db.batch_execute(&format!(
            "ALTER TABLE hagency_agent_v1.management_shared_work DISABLE TRIGGER USER;\
             UPDATE hagency_agent_v1.management_shared_work SET known_consumed_tokens={used},retained_tokens={used} WHERE id='{id}';\
             ALTER TABLE hagency_agent_v1.management_shared_work ENABLE TRIGGER USER",
            id = w.id
        ))
        .await
        .unwrap();
    };
    known(&mut db, tokens - 1).await;
    assert_eq!(
        status_at(&f, &room, &f.outsider.p, "$spent_blocked", over).await,
        ended(true),
        "a token is left"
    );
    known(&mut db, tokens).await;
    assert_eq!(
        status_at(&f, &room, &f.outsider.p, "$spent_blocked", over).await,
        ended(false)
    );
    assert_eq!(
        status_at(&f, &room, &f.owner.p, "$spent_blocked", over).await,
        ended(false),
        "never to the owner"
    );
    f.policy = f
        .policy
        .clone()
        .with_agent_budget_deltas(true)
        .with_shared_budget_execution(true);
    budget_tests::apply_delta(&f, &r, &admin, 25, 230).await;
    assert_eq!(
        status_at(&f, &room, &f.outsider.p, "$spent_blocked", over).await,
        ended(true),
        "a budget applied since"
    );
}

/// A row whose reply may not have reached the room says whether its reader
/// may review the Agent's original task records, by the rule of the review's
/// own list: for a project's Agent, its manager and the project's
/// administrator may, and the owner of the computer that runs it may not.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn a_reply_that_may_be_lost_says_who_may_review_a_project_agent() {
    let (f, r, _admin, _a, m) = Box::pin(setup()).await;
    let (d, activity) = qualify(&f).await;
    retain(&f, &r, &m, "review_reply").await;
    let w = f
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
        work_id: w.id.clone(),
        work_digest: allocation_digest(&w).unwrap(),
    };
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_current_work(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
    let t = work::ticket(&f, &d, &w).await;
    f.domain
        .record_shared_work_result(
            &f.owner.p,
            &f.policy,
            &w.id,
            work::outcome(&d, &w, &t, 211, "completed"),
        )
        .await
        .unwrap();
    let room = work::message(&f, &r, "scope").await.room_id;
    let review = async |reader: &Principal| {
        let row = row_of(&f, &room, reader, "$review_reply").await;
        json!([row["state"], row.get("viewerCanReview")])
    };
    assert_eq!(
        review(&f.outsider.p).await,
        json!(["reply_pending", null]),
        "said only where the outcome is unknown"
    );
    // The send is cut off before its answer.
    let s = work::sender().await;
    s.drop_first
        .store(true, std::sync::atomic::Ordering::SeqCst);
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &s.matrix, &mut String::new())
        .await
        .unwrap();
    // The sender manages the project and administers it; the owner of the
    // computer does neither.
    assert_eq!(review(&f.outsider.p).await, json!(["reply_unknown", true]));
    assert_eq!(review(&f.owner.p).await, json!(["reply_unknown", false]));
    // The database holds a project's administrator to its manager, and so
    // to the Agent's manager when it was asked for. Were the Agent's manager
    // another, both would be told that they may review: the owner as the
    // Agent's manager, the sender as the project's administrator.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!(
        "ALTER TABLE hagency_agent_v1.management_agent_requests DISABLE TRIGGER USER;\
         UPDATE hagency_agent_v1.management_agent_requests SET manager_user_id='{owner}' WHERE id='{id}';\
         ALTER TABLE hagency_agent_v1.management_agent_requests ENABLE TRIGGER USER",
        owner = f.owner.p.user_id,
        id = r.id
    ))
    .await
    .unwrap();
    for reader in [&f.owner.p, &f.outsider.p] {
        assert_eq!(review(reader).await, json!(["reply_unknown", true]));
    }
}

/// Messages that wait, encrypted, for a daemon that can read them are listed
/// the same way as the others: the reader's 16 newest, each with every
/// Agent's row, so none is listed for only some of its Agents.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn chat_status_lists_whole_encrypted_messages_that_wait_for_a_daemon() {
    let (f, r, admin, _a, m) = Box::pin(setup()).await;
    let (_, second) = Box::pin(work::another_assignment(&f, &r, &admin, &m, 2)).await;
    f.domain
        .change_shared_run_control(
            &f.outsider.p,
            &f.policy,
            &second.id,
            intake_tests::change("arm_second", 1, "running"),
            &m.gateway,
        )
        .await
        .unwrap();
    // The owner's computer has not said that it can read them.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(
        "DELETE FROM hagency_agent_v1.management_shared_executor_protocols WHERE device_id=$1",
    )
    .bind::<Text, _>(f.owner.p.device_id.as_ref().unwrap())
    .execute(&mut db)
    .await
    .unwrap();
    let sealed = async |id: &str| {
        let mut event = work::message(&f, &r, id).await;
        event.body = String::new();
        event.mentioned_mxids = Default::default();
        event.encrypted = true;
        f.domain
            .route_shared_event(&f.policy, &event, now_ms(), &m.gateway)
            .await
            .unwrap();
        event.room_id
    };
    let mut room = String::new();
    for n in 0..16 {
        room = sealed(&format!("sealed_{n:02}")).await;
    }
    // Messages listed, rows listed, and rows of the oldest message.
    let listed = async |reader: &Principal| {
        let query = serde_json::from_value(json!({"roomId": room})).unwrap();
        let read = f.domain.shared_approvals(reader, query).await.unwrap();
        let rows = read["executions"].as_array().unwrap().clone();
        assert!(
            rows.iter()
                .all(|row| row["reason"] == "owner_daemon_update_required"),
            "{rows:?}"
        );
        let mut messages: Vec<_> = rows.iter().map(|row| row["eventId"].clone()).collect();
        messages.dedup();
        (
            messages.len(),
            rows.len(),
            rows.iter()
                .filter(|row| row["eventId"] == "$sealed_00")
                .count(),
        )
    };
    for reader in [&f.outsider.p, &f.owner.p] {
        assert_eq!(listed(reader).await, (16, 32, 2), "both rows of each");
    }
    sealed("sealed_16").await;
    for reader in [&f.outsider.p, &f.owner.p] {
        assert_eq!(
            listed(reader).await,
            (16, 32, 0),
            "neither row of the oldest"
        );
    }
}
