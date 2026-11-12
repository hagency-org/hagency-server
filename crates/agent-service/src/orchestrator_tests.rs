//! The GitFab orchestrator end to end against PostgreSQL: a simulated Room records the stage messages and
//! a simulated Agent claims, runs and answers each dispatch through the real transport.

use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU32, Ordering};

use hagency_gitfab_events::identity;
use hagency_gitfab_events::journal::Journal;
use hagency_gitfab_events::sign::{Key, Keyring};
use serde_json::{Value, json};

use super::*;
use crate::orchestrator::{DEFAULT_STAGE_TIMEOUT_MS, Orchestrator, Recorder, StageRoom};
use crate::orchestrator_flow::{HumanDecision, Input, SpecDecision, Stage, State};
use crate::transport::ORCHESTRATOR_ROLES;

struct Room {
    facts: DeliveryFacts,
    requester_in_room: bool,
    /// (transaction ID, body) of each distinct stage message.
    posts: StdMutex<Vec<(String, String)>>,
}

impl StageRoom for Room {
    async fn post(
        &self,
        _room: &str,
        _root: &str,
        txn: &str,
        body: &str,
        meta: &Value,
    ) -> Result<String> {
        assert_eq!(meta["version"], 1);
        let mut posts = self.posts.lock().unwrap();
        if !posts.iter().any(|(t, _)| t == txn) {
            posts.push((txn.to_owned(), body.to_owned()));
        }
        Ok(format!("${txn}"))
    }

    async fn admit_shared(&self, _event: &RoutedEvent, _received_at_ms: i64) -> Result<()> {
        Err(Error::Unauthorized("shared_execution_unavailable"))
    }

    async fn facts(&self, _scope: &RoutingScope, requester: &str) -> Result<DeliveryFacts> {
        Ok(DeliveryFacts {
            requester_mxid: requester.to_owned(),
            requester_in_room: self.requester_in_room,
            observed_at_ms: crate::api::now_ms(),
            ..self.facts.clone()
        })
    }
}

struct Harness {
    f: Fixture,
    room: Room,
    recorder: Recorder,
    journal: std::path::PathBuf,
    keyring: Keyring,
    lease: LeaseRef,
    answers: AtomicU32,
    _dir: tempfile::TempDir,
}

async fn harness(requester_in_room: bool) -> Harness {
    let f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join("events.jsonl");
    let key = Key::generate("gitfab-1").unwrap();
    let mut keyring = Keyring::default();
    keyring.insert("gitfab-1", &key.public_base64()).unwrap();
    let recorder = Recorder::new(
        Journal::open(&journal).unwrap(),
        std::sync::Arc::new(key),
        "/hagency/gitfab/v1/public-keys#gitfab-1".into(),
    );
    let now = crate::api::now_ms();
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    Harness {
        room: Room {
            facts: f.facts.clone(),
            requester_in_room,
            posts: StdMutex::new(Vec::new()),
        },
        f,
        recorder,
        journal,
        keyring,
        lease,
        answers: AtomicU32::new(0),
        _dir: dir,
    }
}

fn orchestrator(h: &Harness, stage_timeout_ms: i64) -> Orchestrator<'_, Room> {
    Orchestrator {
        transport: &h.f.transport,
        room: &h.room,
        recorder: &h.recorder,
        stage_timeout_ms,
    }
}

async fn start(h: &Harness, root: &str) -> String {
    let init = OrchestratorRunInit {
        project_id: "DEMO-02".into(),
        task_id: "task-healthz".into(),
        room_id: h.f.facts.room_id.clone(),
        thread_root: root.into(),
        requester_mxid: h.f.facts.requester_mxid.clone(),
        bindings: json!({"coordinator": h.f.binding, "implementer": h.f.binding, "reviewer": h.f.binding}),
        context: json!({"goal": "Add GET /healthz", "gitBackend": "github",
            "repoRef": "https://github.com/example/gitfab-demo-02", "baselineSha": "b".repeat(40),
            "specRef": "spec/DEMO-02.md@v1"}),
    };
    h.f.transport
        .create_orchestrator_run(&init, crate::api::now_ms())
        .await
        .unwrap()
        .id
}

async fn run(h: &Harness, id: &str) -> OrchestratorRun {
    h.f.transport.orchestrator_run(id).await.unwrap().unwrap()
}

/// The simulated Agent: claims the run's current dispatch, runs it and delivers `body` to the Room.
async fn answer(h: &Harness, run_id: &str, body: &str) {
    let dispatch = run(h, run_id)
        .await
        .dispatch_id
        .expect("a dispatch is waiting");
    let n = h.answers.fetch_add(1, Ordering::SeqCst);
    let execution = format!("exec-{run_id}-{n}");
    running(&h.f, &h.lease, &dispatch, &execution).await;
    let now = crate::api::now_ms();
    let reply = SubmitReply {
        dispatch_id: dispatch,
        execution_id: execution,
        body: body.into(),
    };
    let committed =
        h.f.transport
            .submit_reply(&h.f.p, &h.lease, reply, &h.f.facts, now)
            .await
            .unwrap();
    let sending =
        h.f.transport
            .claim_reply(&committed.id, &h.f.facts, now)
            .await
            .unwrap();
    h.f.transport
        .confirm_reply(
            &sending.id,
            sending.worker_token.as_deref().unwrap(),
            Some(&format!("$reply-{run_id}-{n}")),
            now,
        )
        .await
        .unwrap();
}

fn running_stage(state: &State) -> Option<(Stage, u32, u32)> {
    match state {
        State::Running { slot, .. } => Some((slot.stage, slot.round, slot.attempt)),
        _ => None,
    }
}

fn journal(h: &Harness) -> Vec<Value> {
    std::fs::read_to_string(&h.journal)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn task_steps(events: &[Value]) -> Vec<(String, String, i64)> {
    events
        .iter()
        .filter(|e| e["schema_version"] == "gitfab.task.v1")
        .map(|e| {
            (
                e["phase"].as_str().unwrap().to_owned(),
                e["status"].as_str().unwrap().to_owned(),
                e["round"].as_i64().unwrap(),
            )
        })
        .collect()
}

fn steps(list: &[(&str, &str, i64)]) -> Vec<(String, String, i64)> {
    list.iter()
        .map(|(p, s, r)| (p.to_string(), s.to_string(), *r))
        .collect()
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_a_run_goes_from_spec_to_release_with_one_rework_and_a_signed_journal() {
    let h = harness(true).await;
    let o = orchestrator(&h, DEFAULT_STAGE_TIMEOUT_MS);
    let now = crate::api::now_ms;
    let id = start(&h, "$gitfab-e2e-happy").await;

    o.pass(now()).await.unwrap();
    assert_eq!(
        running_stage(&run(&h, &id).await.state),
        Some((Stage::Spec, 0, 1))
    );
    answer(
        &h,
        &id,
        "Spec v1: GET /healthz returns 200 with {\"ok\":true}.",
    )
    .await;
    o.pass(now()).await.unwrap();
    assert_eq!(
        run(&h, &id).await.state,
        State::AwaitingSpecApproval { spec_revision: 1 }
    );
    // Nothing runs until the Spec is confirmed.
    o.pass(now()).await.unwrap();
    assert_eq!(h.room.posts.lock().unwrap().len(), 1);
    assert!(matches!(
        o.decide(&id, Input::Human(HumanDecision::Continue), now())
            .await
            .unwrap_err(),
        Error::Conflict("orchestrator_decision_not_applicable")
    ));
    o.decide(&id, Input::SpecDecision(SpecDecision::Approve), now())
        .await
        .unwrap();
    assert_eq!(
        running_stage(&run(&h, &id).await.state),
        Some((Stage::Plan, 0, 1))
    );

    for (stage, round, reply) in [
        (Stage::Plan, 0, "Plan: add route, add test."),
        (
            Stage::Execute,
            1,
            "Committed 1111111 on gitfab/task-healthz.",
        ),
        (Stage::Test, 1, "cargo test: 12 passed, 0 failed."),
        (
            Stage::Review,
            1,
            "1. No test for the error path.\nVERDICT: reject",
        ),
        (
            Stage::Execute,
            2,
            "Committed 2222222: added the error-path test.",
        ),
        (Stage::Test, 2, "cargo test: 13 passed, 0 failed."),
        (
            Stage::Review,
            2,
            "All findings addressed.\nVERDICT: approve",
        ),
        (Stage::Release, 2, "Tagged v0.1.0 at 2222222."),
    ] {
        assert_eq!(
            running_stage(&run(&h, &id).await.state),
            Some((stage, round, 1)),
            "{stage:?}"
        );
        answer(&h, &id, reply).await;
        o.pass(now()).await.unwrap();
    }
    assert_eq!(run(&h, &id).await.state, State::Completed);
    assert!(
        !h.f.transport
            .active_orchestrator_runs(50)
            .await
            .unwrap()
            .iter()
            .any(|r| r.id == id)
    );

    // One message per stage; the rework message carries the Reviewer's findings; passes after the end
    // post nothing.
    o.pass(now()).await.unwrap();
    let posts = h.room.posts.lock().unwrap().clone();
    assert_eq!(posts.len(), 9);
    assert!(
        posts[5].1.starts_with("[GitFab execute]")
            && posts[5].1.contains("No test for the error path")
    );
    assert!(
        posts[1].1.contains("Spec v1: GET /healthz"),
        "the plan stage sees the confirmed Spec"
    );
    assert!(
        posts
            .iter()
            .all(|(_, body)| body.contains("https://github.com/example/gitfab-demo-02"))
    );

    let events = journal(&h);
    assert_eq!(
        task_steps(&events),
        steps(&[
            ("intake", "succeeded", 0),
            ("spec", "running", 0),
            ("spec", "blocked", 0),
            ("spec", "succeeded", 0),
            ("plan", "running", 0),
            ("plan", "succeeded", 0),
            ("execute", "running", 1),
            ("execute", "succeeded", 1),
            ("test", "running", 1),
            ("test", "succeeded", 1),
            ("review", "running", 1),
            ("review", "failed", 1),
            ("execute", "running", 2),
            ("execute", "succeeded", 2),
            ("test", "running", 2),
            ("test", "succeeded", 2),
            ("review", "running", 2),
            ("review", "succeeded", 2),
            ("release", "running", 2),
            ("release", "succeeded", 2),
        ])
    );
    // Every key event is signed and verifies, and each has its identity event.
    let signed: Vec<&Value> = events
        .iter()
        .filter(|e| e.get("signature").is_some_and(|s| !s.is_null()))
        .collect();
    assert_eq!(
        signed.len(),
        6,
        "task created, two execution results, two verdicts, the release"
    );
    for e in &signed {
        h.keyring.verify(e).unwrap();
    }
    assert!(
        identity::check(&events).is_empty(),
        "{:?}",
        identity::check(&events)
    );
    let identities: Vec<&Value> = events
        .iter()
        .filter(|e| e["schema_version"] == "gitfab.identity.v1")
        .collect();
    assert_eq!(identities[0]["actor_type"], "human");
    assert!(
        identities[1..]
            .iter()
            .all(|i| i["actor_type"] == "agent" && i["actor_id"] == h.f.facts.puppet_mxid.as_str())
    );
    let rejected = events
        .iter()
        .find(|e| e["phase"] == "review" && e["status"] == "failed")
        .unwrap();
    assert_eq!(
        rejected["findings"][0],
        "1. No test for the error path.\nVERDICT: reject"
    );
    assert!(
        rejected["output_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_a_timed_out_stage_waits_for_a_human_and_is_never_redispatched_by_itself() {
    let h = harness(true).await;
    let impatient = orchestrator(&h, 0);
    let patient = orchestrator(&h, DEFAULT_STAGE_TIMEOUT_MS);
    let now = crate::api::now_ms;
    let id = start(&h, "$gitfab-e2e-timeout").await;

    impatient.pass(now()).await.unwrap();
    let stopped = run(&h, &id).await.state;
    assert!(
        matches!(
            stopped,
            State::ManualIntervention {
                reason: "stage_timeout",
                ..
            }
        ),
        "{stopped:?}"
    );
    for _ in 0..3 {
        impatient.pass(now()).await.unwrap();
    }
    assert_eq!(
        h.room.posts.lock().unwrap().len(),
        1,
        "no automatic re-dispatch"
    );

    // A Human continuing is a new dispatch with its own message, not a replay.
    patient
        .decide(&id, Input::Human(HumanDecision::Continue), now())
        .await
        .unwrap();
    assert_eq!(
        running_stage(&run(&h, &id).await.state),
        Some((Stage::Spec, 0, 2))
    );
    let posts = h.room.posts.lock().unwrap().clone();
    assert_eq!(posts.len(), 2);
    assert_ne!(posts[0].0, posts[1].0);

    impatient.pass(now()).await.unwrap();
    patient
        .decide(&id, Input::Human(HumanDecision::Abort), now())
        .await
        .unwrap();
    assert_eq!(
        run(&h, &id).await.state,
        State::Failed { reason: "aborted" }
    );
    assert!(identity::check(&journal(&h)).is_empty());
    assert_eq!(
        task_steps(&journal(&h)),
        steps(&[
            ("intake", "succeeded", 0),
            ("spec", "running", 0),
            ("spec", "manual_intervention", 0),
            ("spec", "running", 0),
            ("spec", "manual_intervention", 0),
            ("spec", "failed", 0),
        ])
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_a_stage_the_transport_refuses_is_retried_once_then_fails_the_run() {
    // The requester has left the Room, so the stage message is refused each time.
    let h = harness(false).await;
    let o = orchestrator(&h, DEFAULT_STAGE_TIMEOUT_MS);
    let id = start(&h, "$gitfab-e2e-refused").await;
    o.pass(crate::api::now_ms()).await.unwrap();
    assert_eq!(
        run(&h, &id).await.state,
        State::Failed {
            reason: "stage_failed"
        }
    );
    assert_eq!(
        h.room.posts.lock().unwrap().len(),
        2,
        "first attempt and one retry"
    );
    assert_eq!(
        task_steps(&journal(&h)),
        steps(&[
            ("intake", "succeeded", 0),
            ("spec", "running", 0),
            ("spec", "failed", 0),
        ])
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_journal_entries_saved_before_a_crash_are_written_once_on_the_next_pass() {
    let h = harness(true).await;
    let o = orchestrator(&h, DEFAULT_STAGE_TIMEOUT_MS);
    let id = start(&h, "$gitfab-e2e-crash").await;
    o.pass(crate::api::now_ms()).await.unwrap();
    let before = journal(&h).len();
    // As if the worker stopped after saving a transition but before writing its events: put the
    // already-written entries back as pending.
    let current = run(&h, &id).await;
    let replay: Vec<Value> = journal(&h)
        .into_iter()
        .filter(|e| e["schema_version"] == "gitfab.task.v1")
        .map(|mut e| {
            let actor = if e["phase"] == "intake" {
                "human"
            } else {
                "system"
            };
            e["signature"] = Value::Null;
            json!({"event": e, "actorType": actor})
        })
        .collect();
    let saved =
        h.f.transport
            .save_orchestrator_run(
                &id,
                current.revision,
                &OrchestratorRunUpdate {
                    state: &current.state,
                    dispatch_id: current.dispatch_id.as_deref(),
                    handoff: current.handoff.as_deref(),
                    pending_events: &Value::Array(replay),
                },
                crate::api::now_ms(),
            )
            .await
            .unwrap();
    o.pass(crate::api::now_ms()).await.unwrap();
    assert_eq!(journal(&h).len(), before, "nothing written twice");
    let after = run(&h, &id).await;
    assert_eq!((after.revision, after.pending_events), (saved, json!([])));
}

fn command(h: &Harness, id: &str, thread: Option<&str>, body: &str) -> RoutedEvent {
    RoutedEvent {
        event_id: format!("${id}"),
        room_id: h.f.facts.room_id.clone(),
        sender_mxid: h.f.facts.requester_mxid.clone(),
        body: body.into(),
        mentioned_mxids: Default::default(),
        thread_root: thread.map(str::to_owned),
        encrypted: false,
        is_edit: false,
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_humans_start_and_confirm_a_run_with_room_commands() {
    let h = harness(true).await;
    let o = orchestrator(&h, DEFAULT_STAGE_TIMEOUT_MS);
    let now = crate::api::now_ms;
    let start = command(
        &h,
        "gitfab-cmd-start",
        None,
        &format!(
            "/gitfab start task-cmd\nproject: DEMO-02\nrepo: https://github.com/example/gitfab-demo-02\nbaseline: {}\nspec: spec/DEMO-02.md@v1\ngoal: Add GET /healthz",
            "c".repeat(40)
        ),
    );
    h.f.transport
        .record_orchestrator_command(&start, now())
        .await
        .unwrap();
    // Recording again is harmless.
    h.f.transport
        .record_orchestrator_command(&start, now())
        .await
        .unwrap();
    o.pass(now()).await.unwrap();

    // The command message became the task thread; the single Agent of the Room serves every role.
    let run =
        h.f.transport
            .orchestrator_run_in_thread(&h.f.facts.room_id, "$gitfab-cmd-start")
            .await
            .unwrap()
            .expect("a run for the thread");
    assert_eq!(run.init.requester_mxid, h.f.facts.requester_mxid);
    assert_eq!(run.init.context["projectId"], "DEMO-02");
    assert!(
        ORCHESTRATOR_ROLES
            .iter()
            .all(|r| run.init.bindings[*r] == h.f.binding.as_str())
    );
    assert_eq!(running_stage(&run.state), Some((Stage::Spec, 0, 1)));
    let posts = h.room.posts.lock().unwrap().clone();
    assert!(
        posts
            .iter()
            .any(|(t, b)| t.starts_with("gitfab.cmd.") && b.contains("已发起流程"))
    );
    assert!(
        h.f.transport
            .pending_orchestrator_commands(10)
            .await
            .unwrap()
            .is_empty()
    );

    answer(&h, &run.id, "Spec v1: GET /healthz returns 200.").await;
    o.pass(now()).await.unwrap();
    assert_eq!(
        run_state(&h, &run.id).await,
        State::AwaitingSpecApproval { spec_revision: 1 }
    );

    // Someone else cannot confirm; a confirmation outside the thread finds no run.
    let mut stranger = command(
        &h,
        "gitfab-cmd-stranger",
        Some("$gitfab-cmd-start"),
        "/gitfab approve",
    );
    stranger.sender_mxid = "@someone-else:example.test".into();
    h.f.transport
        .record_orchestrator_command(&stranger, now())
        .await
        .unwrap();
    let elsewhere = command(&h, "gitfab-cmd-elsewhere", None, "/gitfab approve");
    h.f.transport
        .record_orchestrator_command(&elsewhere, now())
        .await
        .unwrap();
    o.pass(now()).await.unwrap();
    assert_eq!(
        run_state(&h, &run.id).await,
        State::AwaitingSpecApproval { spec_revision: 1 }
    );
    let replies: Vec<String> = h
        .room
        .posts
        .lock()
        .unwrap()
        .iter()
        .map(|(_, b)| b.clone())
        .collect();
    assert!(replies.iter().any(|b| b.contains("只有发起人")));
    assert!(replies.iter().any(|b| b.contains("没有进行中的流程")));

    let approve = command(
        &h,
        "gitfab-cmd-approve",
        Some("$gitfab-cmd-start"),
        "/gitfab approve",
    );
    h.f.transport
        .record_orchestrator_command(&approve, now())
        .await
        .unwrap();
    o.pass(now()).await.unwrap();
    assert_eq!(
        running_stage(&run_state(&h, &run.id).await),
        Some((Stage::Plan, 0, 1))
    );

    // A malformed command is answered with the usage and never retried.
    let bad = command(&h, "gitfab-cmd-bad", None, "/gitfab start only-a-task-id");
    h.f.transport
        .record_orchestrator_command(&bad, now())
        .await
        .unwrap();
    o.pass(now()).await.unwrap();
    assert!(
        h.room
            .posts
            .lock()
            .unwrap()
            .iter()
            .any(|(_, b)| b.contains("缺少 goal") && b.contains("用法"))
    );
    assert!(
        h.f.transport
            .pending_orchestrator_commands(10)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn run_state(h: &Harness, id: &str) -> State {
    run(h, id).await.state
}
