//! What a message's status says once it ended without a turn, read from every
//! record that can end it; the budget queue after such a message; and the
//! time bound's sweep past an attempt it cannot stop.
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
/// the budget. It ends, its sender may send it again, and the budget queue
/// offers the next message instead of waiting behind it.
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
        if let V2End::Unrepresentable = end {
            // Without a budget, a block on usage is not an end while the
            // message has time; it is once its time is over, as for a block
            // on allowance. Reading writes nothing.
            assert_eq!(
                status_of(&f, &room, &f.outsider.p, &ended).await,
                json!(["blocked", "usage_unrepresentable", null, false])
            );
            f.domain
                .set_management_test_clock(db_now(&f).await + 86_400_000 + 1_000)
                .await
                .unwrap();
            let late = status_of(&f, &room, &f.outsider.p, &ended).await;
            f.domain.reset_management_test_clock().await.unwrap();
            assert_eq!(
                late,
                json!(["expired", "event_expired", "event_expired", true])
            );
        }
        budget_tests::apply_delta(&f, &r, &admin, 25, 230).await;
        // Read before any budget poll, the message the budget retired reads
        // as ended already, as the poll then records it.
        assert_eq!(
            status_of(&f, &room, &f.outsider.p, &ended).await,
            json!(["blocked", "head_changed", "head_changed", true]),
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
        for (reader, again) in [(&f.outsider.p, true), (&f.owner.p, false)] {
            assert_eq!(
                status_of(&f, &room, reader, &ended).await,
                json!(["blocked", "head_changed", "head_changed", again]),
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
    let (first, _) = qualify(&f).await;
    let stop = attempts::stop(&f, &first, 201).await;
    let one = issue_lapsed(&f, &m, attempts::start(&first, &stop)).await;
    let (_, b) = Box::pin(work::another_assignment(&f, &r, &admin, &m, 2)).await;
    let second = shared_poll(&f).await;
    assert_eq!(second.command.assignment_id, b.id);
    save(&f, &second, "qualified", 310).await.unwrap();
    let stop = attempts::stop(&f, &second, 311).await;
    let two = issue_lapsed(&f, &m, attempts::start(&second, &stop)).await;
    assert_eq!(one.command.assignment_id, a.id);
    let mut ids = [one.command.command_id, two.command.command_id];
    ids.sort();
    let [refused, other] = ids;
    // The attempt the sweep takes first (it goes by id) cannot be stopped.
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute(&format!(
        "CREATE FUNCTION hagency_agent_v1.fixture_refuse_stop() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture_refuse_stop'; END; $$;\
         CREATE TRIGGER fixture_refuse_stop BEFORE UPDATE ON hagency_agent_v1.management_shared_runtimes FOR EACH ROW WHEN (OLD.id='{refused}' AND NEW.state='stopped') EXECUTE FUNCTION hagency_agent_v1.fixture_refuse_stop()"
    ))
    .await
    .unwrap();
    let due = db_now(&f).await + 180_000 + 1_000;
    f.domain.set_management_test_clock(due).await.unwrap();
    let swept = f.domain.sweep_shared_attempt_timeouts().await;
    let states = (
        runtime_state(&f, &other).await.0,
        runtime_state(&f, &refused).await.0,
    );
    db.batch_execute("DROP TRIGGER fixture_refuse_stop ON hagency_agent_v1.management_shared_runtimes;DROP FUNCTION hagency_agent_v1.fixture_refuse_stop()")
        .await
        .unwrap();
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
