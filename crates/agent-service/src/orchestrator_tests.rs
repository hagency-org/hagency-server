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
    git: Option<crate::orchestrator_git::GitHosts>,
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

    fn git(&self) -> Option<&crate::orchestrator_git::GitHosts> {
        self.git.as_ref()
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
            git: None,
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
        team: None,
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

/// A Plan reply with the Module block every Plan must carry.
const PLAN_REPLY: &str = "Plan: add route, add test.\n```gitfab-modules\n[{\"id\":\"M1\",\"title\":\"healthz\",\"spec\":\"GET /healthz returns 200\",\"tests\":[\"cargo test --locked\"],\"capabilities\":[\"rust\"]}]\n```";

/// The simulated Agent: claims the run's current dispatch, runs it and delivers `body` to the Room.
async fn answer(h: &Harness, run_id: &str, body: &str) {
    let current = run(h, run_id).await;
    let dispatch = current
        .dispatch_id
        .unwrap_or_else(|| panic!("a dispatch is waiting: {:?}", current.state));
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

/// The task events of one run. Tests share the database and every pass serves all open runs, so a
/// journal can also hold events of runs other tests left open.
fn own(events: &[Value], run_id: &str) -> Vec<Value> {
    events.iter().filter(|e| e["schema_version"] != "gitfab.task.v1" || e["run_id"] == run_id).cloned().collect()
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
        (Stage::Plan, 0, PLAN_REPLY),
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
        task_steps(&own(&journal(&h), &id)),
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
        task_steps(&own(&journal(&h), &id)),
        steps(&[
            ("intake", "succeeded", 0),
            ("spec", "running", 0),
            ("spec", "failed", 0),
        ])
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_a_plan_needing_what_the_team_cannot_do_is_retried_once_then_fails_the_run() {
    let h = harness(true).await;
    let team = crate::orchestrator_team::Team::new([
        ("coordinator", vec!["planning".to_owned()]),
        ("implementer", vec!["rust".to_owned()]),
        ("reviewer", vec!["code-review".to_owned()]),
    ])
    .unwrap();
    let o = Orchestrator {
        team: Some(&team),
        ..orchestrator(&h, DEFAULT_STAGE_TIMEOUT_MS)
    };
    let now = crate::api::now_ms;
    let id = start(&h, "$gitfab-e2e-team").await;
    o.pass(now()).await.unwrap();
    answer(&h, &id, "Spec v1").await;
    o.pass(now()).await.unwrap();
    o.decide(&id, Input::SpecDecision(SpecDecision::Approve), now())
        .await
        .unwrap();
    let python = PLAN_REPLY.replace("[\"rust\"]", "[\"python\"]");
    for attempt in 1..=2 {
        o.pass(now()).await.unwrap();
        assert_eq!(
            running_stage(&run(&h, &id).await.state),
            Some((Stage::Plan, 0, attempt))
        );
        answer(&h, &id, &python).await;
        o.pass(now()).await.unwrap();
    }
    assert_eq!(
        run(&h, &id).await.state,
        State::Failed {
            reason: "stage_failed"
        }
    );
    // The Plan message told the Coordinator what the team can do.
    let posts = h.room.posts.lock().unwrap();
    let plans: Vec<_> = posts
        .iter()
        .filter(|(_, b)| b.starts_with("[GitFab plan]"))
        .collect();
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().all(|(_, b)| b.contains("声明的能力为 rust")));
    // A Plan within the team's capabilities would have counted; the stock reply needs only rust.
    assert!(
        team.uncovered(&crate::orchestrator_modules::modules(PLAN_REPLY).unwrap())
            .is_empty()
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

/// A GitHub that holds one repository and records the Pull Request and Release GitFab creates.
fn fake_github() -> crate::orchestrator_git::GitHosts {
    use hagency_gitfab_forge::{Backend, Forge, Request, Response};
    let created = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let head = "c".repeat(40);
    let answer = move |r: &Request| -> Response {
        let repo = "/repos/example/gitfab-demo-02";
        let pull = json!({"number": 7, "html_url": "https://github.com/example/gitfab-demo-02/pull/7",
            "head": {"ref": "agent/DEMO-02/task-healthz", "sha": head}});
        let (status, body) = match (r.method, r.path.strip_prefix(repo).unwrap_or(&r.path)) {
            ("GET", "") => (
                200,
                json!({"full_name": "example/gitfab-demo-02", "default_branch": "main",
                "html_url": "https://github.com/example/gitfab-demo-02"}),
            ),
            ("GET", p)
                if p.starts_with(
                    "/pulls?state=open&head=example%3Aagent%2FDEMO-02%2Ftask-healthz&base=main",
                ) =>
            {
                (
                    200,
                    if std::sync::atomic::AtomicBool::load(&created, Ordering::SeqCst) {
                        json!([pull])
                    } else {
                        json!([])
                    },
                )
            }
            ("POST", "/pulls") => {
                assert_eq!(
                    r.body.as_ref().unwrap()["head"],
                    "agent/DEMO-02/task-healthz"
                );
                std::sync::atomic::AtomicBool::store(&created, true, Ordering::SeqCst);
                (201, pull)
            }
            ("PATCH", "/pulls/7") => (200, json!({})),
            ("GET", p) if p == format!("/commits/{head}/status") => {
                (200, json!({"total_count": 1, "state": "success"}))
            }
            ("GET", p) if p.starts_with(&format!("/commits/{head}/check-runs")) => {
                (200, json!({"check_runs": []}))
            }
            ("GET", "/pulls/7") => (200, {
                let mut p = pull.clone();
                p["mergeable"] = json!(true);
                p
            }),
            ("GET", "/pulls/7/reviews?per_page=100") => (
                200,
                json!([{"user": {"login": "maintainer"}, "state": "APPROVED"}]),
            ),
            ("GET", "/releases/tags/gitfab-task-healthz-r1") => (404, json!({})),
            ("POST", "/releases") => {
                assert_eq!(r.body.as_ref().unwrap()["target_commitish"], head.as_str());
                (
                    201,
                    json!({"html_url": "https://github.com/example/gitfab-demo-02/releases/tag/gitfab-task-healthz-r1"}),
                )
            }
            other => panic!("unexpected GitHub call {other:?}"),
        };
        Response { status, body }
    };
    crate::orchestrator_git::GitHosts {
        github: Some(Forge::new(
            Backend::GitHub,
            crate::orchestrator_git::GitTransport::Fake(std::sync::Arc::new(answer)),
        )),
        gitcode: None,
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_a_run_opens_its_pull_request_and_publishes_the_release_on_the_platform() {
    let mut h = harness(true).await;
    h.room.git = Some(fake_github());
    let o = orchestrator(&h, DEFAULT_STAGE_TIMEOUT_MS);
    let now = crate::api::now_ms;
    let id = start(&h, "$gitfab-e2e-git").await;
    o.pass(now()).await.unwrap();
    answer(&h, &id, "Spec v1").await;
    o.pass(now()).await.unwrap();
    o.decide(&id, Input::SpecDecision(SpecDecision::Approve), now())
        .await
        .unwrap();
    for reply in [
        PLAN_REPLY,
        "pushed ccc to agent/DEMO-02/task-healthz",
        "tests pass",
        "VERDICT: approve",
        "released",
    ] {
        o.pass(now()).await.unwrap();
        answer(&h, &id, reply).await;
        o.pass(now()).await.unwrap();
    }
    assert_eq!(run(&h, &id).await.state, State::Completed);
    let pr = "https://github.com/example/gitfab-demo-02/pull/7";
    let release = "https://github.com/example/gitfab-demo-02/releases/tag/gitfab-task-healthz-r1";
    let refs = |phase: &str| {
        journal(&h)
            .into_iter()
            .find(|e| e["phase"] == phase && e["status"] == "succeeded")
            .map(|e| e["artifact_refs"].clone())
    };
    assert_eq!(refs("plan"), Some(json!([])));
    assert_eq!(refs("execute"), Some(json!([pr])));
    assert_eq!(refs("test"), Some(json!([pr])));
    assert_eq!(refs("release"), Some(json!([release, pr])));
    let field = |phase: &str, name: &str| {
        journal(&h)
            .into_iter()
            .find(|e| e["phase"] == phase && e["status"] == "succeeded")
            .map(|e| e[name].clone())
    };
    let head = json!("c".repeat(40));
    for phase in ["execute", "test", "release"] {
        assert_eq!(field(phase, "commit_sha"), Some(head.clone()), "{phase}");
        assert_eq!(field(phase, "platform_error"), Some(Value::Null), "{phase}");
    }
    assert_eq!(field("plan", "commit_sha"), Some(Value::Null));
    assert_eq!(
        field("plan", "modules"),
        Some(
            json!([{"id":"M1","title":"healthz","spec":"GET /healthz returns 200","tests":["cargo test --locked"],"capabilities":["rust"]}])
        )
    );
    let posts = h.room.posts.lock().unwrap();
    assert!(
        posts
            .iter()
            .any(|(_, b)| b.contains("任务分支：agent/DEMO-02/task-healthz"))
    );
    assert!(
        posts
            .iter()
            .any(|(_, b)| b == &format!("已创建或更新 PR：{pr}"))
    );
    assert!(
        posts
            .iter()
            .any(|(_, b)| b == &format!("平台测试状态：通过（{pr}）；平台审查：已批准；合并：与默认分支无冲突"))
    );
    assert!(
        posts
            .iter()
            .any(|(_, b)| b == &format!("已发布 Release gitfab-task-healthz-r1：{release}"))
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_a_held_stage_is_reassigned_to_the_named_agent_of_the_room() {
    let h = harness(true).await;
    let impatient = orchestrator(&h, 0);
    let patient = orchestrator(&h, DEFAULT_STAGE_TIMEOUT_MS);
    let now = crate::api::now_ms;
    let root = "$gitfab-e2e-reassign";
    let id = start(&h, root).await;
    impatient.pass(now()).await.unwrap();
    let held = run(&h, &id).await;
    assert!(matches!(held.state, State::ManualIntervention { .. }), "{:?}", held.state);

    // Storage: a held run's role can be rebound once per revision.
    h.f.transport
        .rebind_orchestrator_role(&id, held.revision, "coordinator", "bnd_other", now())
        .await
        .unwrap();
    assert_eq!(run(&h, &id).await.init.bindings["coordinator"], "bnd_other");
    assert!(matches!(
        h.f.transport
            .rebind_orchestrator_role(&id, held.revision, "coordinator", "bnd_x", now())
            .await,
        Err(Error::Conflict("orchestrator_run_revision_changed"))
    ));

    // An Agent outside the Room is refused and the stage stays held.
    h.f.transport
        .record_orchestrator_command(&command(&h, "cmd-unknown", Some(root), "/gitfab reassign @nobody:example.test"), now())
        .await
        .unwrap();
    patient.pass(now()).await.unwrap();
    assert!(matches!(run(&h, &id).await.state, State::ManualIntervention { .. }));

    // The Room's Agent, named by its account, takes the held stage.
    let puppet = h.f.facts.puppet_mxid.clone();
    h.f.transport
        .record_orchestrator_command(&command(&h, "cmd-reassign", Some(root), &format!("/gitfab reassign {puppet}")), now())
        .await
        .unwrap();
    patient.pass(now()).await.unwrap();
    let after = run(&h, &id).await;
    assert_eq!(running_stage(&after.state), Some((Stage::Spec, 0, 2)));
    assert_eq!(after.init.bindings["coordinator"], h.f.binding.as_str());
    let posts = h.room.posts.lock().unwrap().clone();
    assert!(posts.iter().any(|(_, b)| b.contains("不是本房间里正在服务的 Agent")));
    assert!(posts.iter().any(|(_, b)| b.contains("已改派，coordinator 由")));
    // Leave no open run behind for the other tests' passes.
    impatient.pass(now()).await.unwrap();
    patient.decide(&id, Input::Human(HumanDecision::Abort), now()).await.unwrap();
    assert_eq!(run(&h, &id).await.state, State::Failed { reason: "aborted" });
}
