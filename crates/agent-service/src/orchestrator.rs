//! The GitFab orchestrator worker: drives each run through `orchestrator_flow` by posting a stage message
//! into the task thread, admitting it for the next stage's Agent, and reading the dispatch's result back.
//! See docs/design/2026-11-12-gitfab-orchestrator.zh-CN.md.
//!
//! Crash safety: a transition and the journal events it produces are saved together (`pending_events`)
//! before anything is written to the journal, and journal appends are idempotent, so a restart writes
//! each event exactly once with the content first decided. Posting a stage message uses a transaction ID
//! fixed by the run, stage, round, attempt and Spec revision, so a repeated post returns the same event.

// Wired into the worker set and the [gitfab] configuration in the next change; tests drive it now.
#![allow(dead_code)]

use std::future::Future;
use std::sync::Mutex;

use hagency_gitfab_events::journal::{Journal, Signer};
use hagency_gitfab_events::sign::Key;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::orchestrator_flow::{self as flow, Action, Input, Outcome, Role, Slot, Stage, State};
use crate::transport::{
    DeliveryFacts, OrchestratedStage, OrchestratorRun, OrchestratorRunUpdate, RouteResult,
    RoutedEvent, RoutingScope, TransportStore,
};
use crate::{Error, Result};

/// A stage that has not produced a result after this long stops for a Human.
pub const DEFAULT_STAGE_TIMEOUT_MS: i64 = 2 * 60 * 60 * 1000;
/// The previous stage's reply carried into the next stage message.
const HANDOFF_LIMIT: usize = 32 * 1024;
/// A Reviewer's findings kept in the task event.
const FINDINGS_LIMIT: usize = 4 * 1024;
const SYSTEM_ACTOR: &str = "system:gitfab-orchestrator";

/// The Matrix side of the worker: posting stage messages and observing who may receive them.
pub(crate) trait StageRoom: Sync {
    /// Posts `body` as a thread message of `thread_root`. The same `txn` returns the same event ID.
    fn post(
        &self,
        room_id: &str,
        thread_root: &str,
        txn: &str,
        body: &str,
    ) -> impl Future<Output = Result<String>> + Send;
    /// Fresh delivery facts for `scope` with `requester` as the requester.
    fn facts(
        &self,
        scope: &RoutingScope,
        requester: &str,
    ) -> impl Future<Output = Result<DeliveryFacts>> + Send;
}

/// The signed event journal (`hagency-gitfab-events`).
pub(crate) struct Recorder {
    journal: Mutex<Journal>,
    key: Key,
    public_key_ref: String,
}

impl Recorder {
    pub(crate) fn new(journal: Journal, key: Key, public_key_ref: String) -> Self {
        Self {
            journal: Mutex::new(journal),
            key,
            public_key_ref,
        }
    }

    fn append(&self, event: &Value, actor_type: &str) -> Result<()> {
        let signer = Signer {
            key: &self.key,
            public_key_ref: &self.public_key_ref,
        };
        let mut journal = self
            .journal
            .lock()
            .map_err(|_| Error::Unavailable("gitfab_journal_poisoned"))?;
        journal
            .append(event, Some(&signer), actor_type)
            .map(|_| ())
            .map_err(|e| {
                tracing::error!(code = e.code(), "gitfab journal refused an event");
                Error::Conflict("gitfab_journal_refused")
            })
    }
}

pub(crate) struct Orchestrator<'a, R: StageRoom> {
    pub transport: &'a TransportStore,
    pub room: &'a R,
    pub recorder: &'a Recorder,
    pub stage_timeout_ms: i64,
}

fn role_key(role: Role) -> &'static str {
    match role {
        Role::Coordinator => "coordinator",
        Role::Implementer => "implementer",
        Role::Reviewer => "reviewer",
    }
}

fn binding_for(run: &OrchestratorRun, stage: Stage) -> Result<&str> {
    run.init.bindings[role_key(stage.role())]
        .as_str()
        .ok_or(Error::Invalid("orchestrator_role_unbound"))
}

fn sha256(text: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(text.as_bytes())))
}

fn truncate(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn timestamp(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// What one stage asks of its Agent.
fn instruction(stage: Stage) -> &'static str {
    match stage {
        Stage::Spec => {
            "根据目标起草本任务的 Spec：范围、验收标准、不做的事、风险。只输出 Spec 正文，等待人工确认。"
        }
        Stage::Plan => "根据已确认的 Spec 给出实施计划：步骤、涉及文件、测试方法。",
        Stage::Execute => {
            "按计划在仓库中实现，提交到任务分支。回复中列出提交 SHA、改动文件和自测结果。若上一轮评审驳回，逐条处理其中的问题。"
        }
        Stage::Test => "运行验收测试，回复测试命令、通过与失败数、失败原因和日志位置。",
        Stage::Review => {
            "评审本轮实现和测试结果是否满足 Spec。逐条列出问题，最后单独一行写结论：VERDICT: approve、VERDICT: reject 或 VERDICT: blocked。"
        }
        Stage::Release => "按发布流程打标签并发布，回复标签、提交 SHA、制品摘要和回退方式。",
    }
}

/// The stage message: everything the Agent needs, since it sees only this message.
pub(crate) fn stage_message(
    run: &OrchestratorRun,
    slot: Slot,
    spec_revision: u32,
    handoff: Option<&str>,
) -> String {
    let c = &run.init.context;
    let text = |k: &str| c[k].as_str().unwrap_or("");
    let mut body = format!(
        "[GitFab {phase}] 任务 {task} · 第 {round} 轮 · 第 {attempt} 次\n目标：{goal}\n仓库：{repo}（{backend}，基线 {sha}）\nSpec：{spec}（第 {rev} 版）\n\n本阶段要求：{ask}",
        phase = slot.stage.phase(),
        task = run.init.task_id,
        round = slot.round,
        attempt = slot.attempt,
        goal = text("goal"),
        repo = text("repoRef"),
        backend = text("gitBackend"),
        sha = text("baselineSha"),
        spec = text("specRef"),
        rev = spec_revision,
        ask = instruction(slot.stage),
    );
    if let Some(prior) = handoff.filter(|h| !h.trim().is_empty()) {
        body.push_str("\n\n上一阶段产出：\n");
        body.push_str(prior);
    }
    body
}

fn txn_id(run: &OrchestratorRun, slot: Slot, spec_revision: u32) -> String {
    format!(
        "gitfab.{}.{}.r{}.a{}.s{}",
        run.id,
        slot.stage.phase(),
        slot.round,
        slot.attempt,
        spec_revision
    )
}

/// Who caused a transition, for the actor of the events it records.
#[derive(Clone, Copy)]
enum Cause<'r> {
    Human,
    Agent { reply: &'r str },
    System,
}

impl<R: StageRoom> Orchestrator<'_, R> {
    /// One pass over the unfinished runs. A failing run does not hold up the others; the first error is
    /// returned after all were tried.
    pub(crate) async fn pass(&self, now: i64) -> Result<()> {
        let mut first = None;
        for run in self.transport.active_orchestrator_runs(50).await? {
            match self.step(run, now).await {
                Ok(()) | Err(Error::Conflict("orchestrator_run_revision_changed")) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "gitfab orchestrator run did not advance");
                    first.get_or_insert(e);
                }
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// A Human's decision on a run waiting for one: the Spec confirmation, or the way out of Manual
    /// Intervention. Returns the run's new state.
    pub(crate) async fn decide(&self, run_id: &str, input: Input, now: i64) -> Result<State> {
        if !matches!(input, Input::SpecDecision(_) | Input::Human(_)) {
            return Err(Error::Invalid("orchestrator_decision_invalid"));
        }
        let run = self
            .transport
            .orchestrator_run(run_id)
            .await?
            .ok_or(Error::NotFound("orchestrator_run_not_found"))?;
        self.flush(&run).await?;
        let Some(run) = self.apply(run, input, Cause::Human, now).await? else {
            return Err(Error::Conflict("orchestrator_decision_not_applicable"));
        };
        // Start the next stage now rather than on the next pass.
        let state = run.state.clone();
        self.step(run, now).await?;
        Ok(state)
    }

    /// Advances one run as far as it can go without waiting.
    async fn step(&self, mut run: OrchestratorRun, now: i64) -> Result<()> {
        // Bounded: each iteration either saves a transition or returns.
        for _ in 0..8 {
            self.flush(&run).await?;
            let next = match (&run.state, run.dispatch_id.as_deref()) {
                (State::NotStarted, _) => self.apply(run, Input::Start, Cause::Human, now).await?,
                (
                    State::Running {
                        slot,
                        spec_revision,
                    },
                    None,
                ) => {
                    let (slot, rev) = (*slot, *spec_revision);
                    self.dispatch(run, slot, rev, now).await?
                }
                (State::Running { slot, .. }, Some(dispatch)) => {
                    let slot = *slot;
                    let row = self.transport.orchestrated_dispatch(dispatch).await?;
                    // A dispatch the transport no longer has cannot be shown to have run.
                    let outcome = match &row {
                        Some(row) => flow::observed(row, slot.stage),
                        None => Some(Outcome::Unknown),
                    };
                    match outcome {
                        Some(outcome) => {
                            let reply = row.as_ref().and_then(|r| r.reply_body.clone());
                            let cause = match (&outcome, reply.as_deref()) {
                                (Outcome::Replied(_), Some(reply)) => Cause::Agent { reply },
                                _ => Cause::System,
                            };
                            self.apply(run, Input::Result { slot, outcome }, cause, now)
                                .await?
                        }
                        None if now - run.updated_at_ms >= self.stage_timeout_ms => {
                            self.apply(run, Input::Timeout { slot }, Cause::System, now)
                                .await?
                        }
                        None => None,
                    }
                }
                // Waiting for a Human, or finished.
                _ => None,
            };
            match next {
                Some(advanced) => run = advanced,
                None => return Ok(()),
            }
        }
        Ok(())
    }

    /// Writes the journal entries saved with the run's current revision.
    async fn flush(&self, run: &OrchestratorRun) -> Result<()> {
        let pending = run
            .pending_events
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if pending.is_empty() {
            return Ok(());
        }
        for entry in pending {
            let actor_type = entry["actorType"].as_str().unwrap_or("system");
            self.recorder.append(&entry["event"], actor_type)?;
        }
        self.transport
            .clear_orchestrator_events(&run.id, run.revision)
            .await
    }

    /// Applies `input`, saves the transition with its journal entries and writes them. `None` when the
    /// input does not apply.
    async fn apply(
        &self,
        run: OrchestratorRun,
        input: Input,
        cause: Cause<'_>,
        now: i64,
    ) -> Result<Option<OrchestratorRun>> {
        let (state, actions) = flow::next(&run.state, input);
        if state == run.state && actions.is_empty() {
            return Ok(None);
        }
        let reply = match cause {
            Cause::Agent { reply } => Some(reply),
            _ => None,
        };
        let handoff = match reply {
            Some(reply) => Some(truncate(reply, HANDOFF_LIMIT).to_owned()),
            None => run.handoff.clone(),
        };
        let revision = run.revision + 1;
        let completed = match (&run.state, input) {
            (State::Running { slot, .. }, Input::Result { .. } | Input::Timeout { .. }) => {
                Some(*slot)
            }
            (State::ManualIntervention { slot, .. }, _) => Some(*slot),
            _ => None,
        };
        let mut events = Vec::new();
        for (index, action) in actions.iter().enumerate() {
            let Action::Record {
                phase,
                status,
                round,
            } = *action
            else {
                continue;
            };
            let finishes_completed = completed.is_some_and(|s| {
                s.stage.phase() == phase && s.round == round && status != "running"
            });
            let (actor_id, actor_type) = match cause {
                _ if phase == "intake" => (run.init.requester_mxid.clone(), "human"),
                Cause::Human => (run.init.requester_mxid.clone(), "human"),
                Cause::Agent { .. } if finishes_completed => {
                    let stage = Stage::from_phase(phase).expect("flow phases are stages");
                    (self.agent_actor(&run, stage).await, "agent")
                }
                _ => (SYSTEM_ACTOR.to_owned(), "system"),
            };
            let input_digest = match (phase, &actions[..]) {
                ("intake", _) => sha256(&run.init.context.to_string()),
                _ => {
                    // The stage message the record is about: the one just dispatched, or the one that
                    // produced the result.
                    let dispatched = actions.iter().find_map(|a| match a {
                        Action::Dispatch {
                            slot,
                            spec_revision,
                            ..
                        } if slot.stage.phase() == phase && slot.round == round => {
                            Some((*slot, *spec_revision, handoff.as_deref()))
                        }
                        _ => None,
                    });
                    let current = match &run.state {
                        State::Running {
                            slot,
                            spec_revision,
                        }
                        | State::ManualIntervention {
                            slot,
                            spec_revision,
                            ..
                        } => Some((*slot, *spec_revision, run.handoff.as_deref())),
                        State::AwaitingSpecApproval { spec_revision } => Some((
                            Slot {
                                stage: Stage::Spec,
                                round: 0,
                                attempt: 1,
                            },
                            *spec_revision,
                            run.handoff.as_deref(),
                        )),
                        _ => None,
                    };
                    match dispatched.or(current) {
                        Some((slot, rev, prior)) => sha256(&stage_message(&run, slot, rev, prior)),
                        None => sha256(&run.init.context.to_string()),
                    }
                }
            };
            let c = &run.init.context;
            let mut event = json!({
                "schema_version": hagency_gitfab_events::schema::TASK,
                "event_id": format!("{}.{revision}.{index}", run.id),
                "timestamp": timestamp(now),
                "project_id": run.init.project_id,
                "task_id": run.init.task_id,
                "run_id": run.id,
                "matrix_room_id": run.init.room_id,
                "matrix_thread_event_id": run.init.thread_root,
                "git_backend": c["gitBackend"],
                "repo_ref": c["repoRef"],
                "baseline_sha": c["baselineSha"],
                "spec_ref": c["specRef"],
                "phase": phase,
                "status": status,
                "actor_id": actor_id,
                "input_digest": input_digest,
                "output_digest": match reply {
                    Some(reply) if finishes_completed => json!(sha256(reply)),
                    _ => Value::Null,
                },
                "artifact_refs": [],
                "request_id": run.dispatch_id.clone().unwrap_or_else(|| run.id.clone()),
                "signature": null,
                "round": round,
            });
            if phase == "review"
                && let Some(reply) = reply.filter(|_| finishes_completed)
            {
                event["findings"] = json!([truncate(reply, FINDINGS_LIMIT)]);
            }
            events.push(json!({"event": event, "actorType": actor_type}));
        }
        let pending = Value::Array(events);
        let saved = self
            .transport
            .save_orchestrator_run(
                &run.id,
                run.revision,
                &OrchestratorRunUpdate {
                    state: &state,
                    dispatch_id: None,
                    handoff: handoff.as_deref(),
                    pending_events: &pending,
                },
                now,
            )
            .await?;
        let advanced = OrchestratorRun {
            state,
            revision: saved,
            dispatch_id: None,
            handoff,
            pending_events: pending,
            updated_at_ms: now,
            ..run
        };
        self.flush(&advanced).await?;
        Ok(Some(OrchestratorRun {
            pending_events: Value::Array(Vec::new()),
            ..advanced
        }))
    }

    async fn agent_actor(&self, run: &OrchestratorRun, stage: Stage) -> String {
        let Ok(binding) = binding_for(run, stage) else {
            return SYSTEM_ACTOR.to_owned();
        };
        match self.transport.binding_scope(binding).await {
            Ok(scope) => scope.puppet_mxid,
            Err(_) => binding.to_owned(),
        }
    }

    /// Posts the stage message, admits it for the stage's Agent and saves the dispatch. A message the
    /// transport refuses for good counts as a stage that expired before it ran.
    async fn dispatch(
        &self,
        run: OrchestratorRun,
        slot: Slot,
        spec_revision: u32,
        now: i64,
    ) -> Result<Option<OrchestratorRun>> {
        let binding = binding_for(&run, slot.stage)?.to_owned();
        let scope = match self.transport.binding_scope(&binding).await {
            Ok(scope) => scope,
            Err(Error::Unauthorized(_)) => {
                let input = Input::Result {
                    slot,
                    outcome: Outcome::Expired,
                };
                return self.apply(run, input, Cause::System, now).await;
            }
            Err(e) => return Err(e),
        };
        let body = stage_message(&run, slot, spec_revision, run.handoff.as_deref());
        let event_id = self
            .room
            .post(
                &run.init.room_id,
                &run.init.thread_root,
                &txn_id(&run, slot, spec_revision),
                &body,
            )
            .await?;
        let stage = OrchestratedStage {
            run_id: run.id.clone(),
            stage: slot.stage.phase(),
            round: i32::try_from(slot.round).map_err(|_| Error::Invalid("orchestrator_round"))?,
            attempt: i32::try_from(slot.attempt)
                .map_err(|_| Error::Invalid("orchestrator_attempt"))?,
        };
        self.transport
            .register_orchestrated_stage(&binding, &event_id, &stage, now)
            .await?;
        let facts = self.room.facts(&scope, &run.init.requester_mxid).await?;
        let event = RoutedEvent {
            event_id,
            room_id: run.init.room_id.clone(),
            sender_mxid: run.init.requester_mxid.clone(),
            body,
            mentioned_mxids: Default::default(),
            thread_root: Some(run.init.thread_root.clone()),
            encrypted: false,
            is_edit: false,
        };
        let dispatch_id = match self
            .transport
            .ingest_orchestrated(&binding, event, &facts, now)
            .await
        {
            Ok(RouteResult::Queued { dispatch_id } | RouteResult::Duplicate { dispatch_id }) => {
                dispatch_id
            }
            Ok(RouteResult::Ignored)
            | Err(Error::Unauthorized(_) | Error::Invalid(_) | Error::Conflict(_)) => {
                let input = Input::Result {
                    slot,
                    outcome: Outcome::Expired,
                };
                return self.apply(run, input, Cause::System, now).await;
            }
            Err(e) => return Err(e),
        };
        let saved = self
            .transport
            .save_orchestrator_run(
                &run.id,
                run.revision,
                &OrchestratorRunUpdate {
                    state: &run.state,
                    dispatch_id: Some(&dispatch_id),
                    handoff: run.handoff.as_deref(),
                    pending_events: &run.pending_events,
                },
                now,
            )
            .await?;
        Ok(Some(OrchestratorRun {
            revision: saved,
            dispatch_id: Some(dispatch_id),
            updated_at_ms: now,
            ..run
        }))
    }
}
