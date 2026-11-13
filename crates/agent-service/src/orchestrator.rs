//! The GitFab orchestrator worker: drives each run through `orchestrator_flow` by posting a stage message
//! into the task thread, admitting it for the next stage's Agent, and reading the dispatch's result back.
//! See docs/design/2026-11-12-gitfab-orchestrator.zh-CN.md.
//!
//! Crash safety: a transition and the journal events it produces are saved together (`pending_events`)
//! before anything is written to the journal, and journal appends are idempotent, so a restart writes
//! each event exactly once with the content first decided. Posting a stage message uses a transaction ID
//! fixed by the run, stage, round, attempt and Spec revision, so a repeated post returns the same event.

use std::future::Future;
use std::sync::{Arc, Mutex};

use hagency_gitfab_events::journal::{Journal, Signer};
use hagency_gitfab_events::sign::Key;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::orchestrator_flow::{
    self as flow, Action, HumanDecision, Input, Outcome, Role, Slot, SpecDecision, Stage, State,
};
use crate::transport::{
    DeliveryFacts, ORCHESTRATOR_ROLES, OrchestratedStage, OrchestratorCommand, OrchestratorRun,
    OrchestratorRunInit, OrchestratorRunUpdate, RouteResult, RoutedEvent, RoutingScope,
    TransportStore,
};
use crate::{Error, Result};

/// A stage that has not produced a result after this long stops for a Human.
pub const DEFAULT_STAGE_TIMEOUT_MS: i64 = 2 * 60 * 60 * 1000;
/// The previous stage's reply carried into the next stage message.
const HANDOFF_LIMIT: usize = 32 * 1024;
/// A Reviewer's findings kept in the task event.
const FINDINGS_LIMIT: usize = 4 * 1024;
const SYSTEM_ACTOR: &str = "system:gitfab-orchestrator";
/// A role bound to a pool Agent names its assignment (`mas_…`) instead of a personal binding.
const SHARED_PREFIX: &str = "mas_";
/// Dispatch ids of pool stages: `shared:<assignment>:<stage event id>`.
const SHARED_DISPATCH: &str = "shared:";

fn shared_dispatch_id(dispatch: &str) -> Option<(&str, &str)> {
    dispatch.strip_prefix(SHARED_DISPATCH)?.split_once(':')
}

/// The Matrix side of the worker: posting stage messages and observing who may receive them.
pub(crate) trait StageRoom: Sync {
    /// Posts `body` as a thread message of `thread_root`, with `meta` describing the stage for clients.
    /// The same `txn` returns the same event ID.
    fn post(
        &self,
        room_id: &str,
        thread_root: &str,
        txn: &str,
        body: &str,
        meta: &Value,
    ) -> impl Future<Output = Result<String>> + Send;
    /// Fresh delivery facts for `scope` with `requester` as the requester.
    fn facts(
        &self,
        scope: &RoutingScope,
        requester: &str,
    ) -> impl Future<Output = Result<DeliveryFacts>> + Send;
    /// Admits a stage message for a pool Agent through the ordinary shared routing, as if the
    /// requester had sent it mentioning that Agent, so every shared check, budget and intake mode
    /// applies unchanged.
    fn admit_shared(
        &self,
        event: &RoutedEvent,
        received_at_ms: i64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Hosts for Pull Requests and Releases; `None` leaves the platform alone.
    fn git(&self) -> Option<&crate::orchestrator_git::GitHosts> {
        None
    }
}

/// The signed event journal (`hagency-gitfab-events`).
pub struct Recorder {
    journal: Mutex<Journal>,
    key: Arc<Key>,
    public_key_ref: String,
}

impl Recorder {
    pub fn new(journal: Journal, key: Arc<Key>, public_key_ref: String) -> Self {
        Self {
            journal: Mutex::new(journal),
            key,
            public_key_ref,
        }
    }

    /// Opens the journal file, refusing one that does not read back as valid events.
    pub fn open(
        path: &std::path::Path,
        key: Arc<Key>,
        public_key_ref: String,
    ) -> std::result::Result<Self, String> {
        let journal = Journal::open(path).map_err(|e| format!("{}: {e}", e.code()))?;
        Ok(Self::new(journal, key, public_key_ref))
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
    /// The declared Agent team; `None` leaves Plans unchecked against it.
    pub team: Option<&'a crate::orchestrator_team::Team>,
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

/// The Reviewer's reply as kept in the signed event: its first 4 KiB, and its verdict line too when
/// a long reply puts the verdict past the cut.
fn findings(reply: &str) -> Value {
    let head = truncate(reply, FINDINGS_LIMIT);
    let verdict = crate::orchestrator_flow::verdict;
    match verdict(reply) {
        Some(v) if verdict(head) != Some(v) || head.len() < reply.len() => {
            let line = reply
                .lines()
                .rev()
                .find(|l| verdict(l) == Some(v))
                .map(str::trim);
            match line {
                Some(line) if !head.ends_with(line) => json!([head, line]),
                _ => json!([head]),
            }
        }
        _ => json!([head]),
    }
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
            "根据目标起草本任务的 Spec：范围、验收标准、不做的事、风险。验收标准单独成节，标题含“验收标准”，下面逐条列出（每条以 -、* 或序号开头）。只输出 Spec 正文，等待人工确认。没有验收标准的 Spec 不算完成。"
        }
        Stage::Plan => {
            "根据已确认的 Spec 给出实施计划：步骤、涉及文件、测试方法。并把任务拆成 Module，在回复中用一个 ```gitfab-modules 代码块给出 JSON 数组，每项含 id（字母数字、-、_）、title、spec（该 Module 的范围与验收要求）、tests（测试命令或用例，至少一项）、capabilities（所需能力，至少一项），最多 16 项。缺少或格式不对的计划不算完成。"
        }
        Stage::Execute => {
            "按计划在仓库中实现，提交到任务分支。回复中列出提交 SHA、改动文件和自测结果。若上一轮评审驳回，逐条处理其中的问题。"
        }
        Stage::Test => "运行 PRD 冻结的测试命令，逐条回复：命令原文、退出码、通过与失败数、失败原因、日志摘要（末尾若干行）和日志位置、产出的工件。",
        Stage::Review => {
            "评审本轮实现和测试结果是否满足 Spec。逐条对照 Spec 的验收标准，写明是否满足以及证据（测试输出、提交 SHA 或改动位置）；再逐条列出问题；最后单独一行写结论：VERDICT: approve、VERDICT: reject 或 VERDICT: blocked。"
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
    team: Option<&crate::orchestrator_team::Team>,
) -> String {
    let c = &run.init.context;
    let text = |k: &str| c[k].as_str().unwrap_or("");
    let mut body = format!(
        "[GitFab {phase}] 任务 {task} · 第 {round} 轮 · 第 {attempt} 次\n目标：{goal}\n仓库：{repo}（{backend}，基线 {sha}）\n任务分支：{branch}\nSpec：{spec}（第 {rev} 版）\n\n本阶段要求：{ask}",
        phase = slot.stage.phase(),
        task = run.init.task_id,
        round = slot.round,
        attempt = slot.attempt,
        goal = text("goal"),
        repo = text("repoRef"),
        backend = text("gitBackend"),
        sha = text("baselineSha"),
        branch = crate::orchestrator_git::task_branch(run),
        spec = text("specRef"),
        rev = spec_revision,
        ask = instruction(slot.stage),
    );
    if slot.stage == Stage::Plan
        && let Some(team) = team
    {
        body.push('\n');
        body.push_str(&team.plan_note());
    }
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
        for command in self.transport.pending_orchestrator_commands(20).await? {
            if let Err(e) = self.command(&command, now).await {
                tracing::warn!(error = %e, "gitfab command left for the next pass");
                first.get_or_insert(e);
            }
        }
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

    /// Handles one `/gitfab` message and answers it in its thread. Storage or Matrix trouble leaves the
    /// command for the next pass; anything else is answered and finished.
    async fn command(&self, command: &OrchestratorCommand, now: i64) -> Result<()> {
        let root = command
            .thread_root
            .clone()
            .unwrap_or_else(|| command.event_id.clone());
        let (outcome, reply) = match self.handle(command, &root, now).await {
            Ok(done) => done,
            Err(e @ (Error::Unavailable(_) | Error::Database(_))) => return Err(e),
            Err(e) => (format!("refused:{e}"), format!("GitFab：未能执行（{e}）。")),
        };
        let digest = hex::encode(Sha256::digest(command.event_id.as_bytes()));
        let meta =
            json!({"version": 1, "kind": "command_reply", "commandEventId": command.event_id});
        self.room
            .post(
                &command.room_id,
                &root,
                &format!("gitfab.cmd.{}", &digest[..32]),
                &reply,
                &meta,
            )
            .await?;
        self.transport
            .finish_orchestrator_command(&command.event_id, &outcome, now)
            .await
    }

    async fn handle(
        &self,
        command: &OrchestratorCommand,
        root: &str,
        now: i64,
    ) -> Result<(String, String)> {
        if command.sender_mxid.starts_with("@_hagency_") {
            return Ok((
                "ignored:service".into(),
                "GitFab：只接受 Human 的命令。".into(),
            ));
        }
        let parsed = match parse_command(&command.body) {
            Ok(parsed) => parsed,
            Err(reason) => {
                return Ok((
                    format!("refused:{reason}"),
                    format!("GitFab：{reason}\n\n{USAGE}"),
                ));
            }
        };
        match parsed {
            Command::Help => Ok(("help".into(), USAGE.into())),
            Command::Start(start) => self.start(command, root, *start, now).await,
            Command::Decide(_) | Command::Reassign(..) => {
                let (input, to) = match parsed {
                    Command::Reassign(who, input) => (input, Some(who)),
                    Command::Decide(input) => (input, None),
                    _ => unreachable!("matched above"),
                };
                let Some(run) = self
                    .transport
                    .orchestrator_run_in_thread(&command.room_id, root)
                    .await?
                else {
                    return Ok((
                        "refused:no_run".into(),
                        "GitFab：本 Thread 没有进行中的流程。请在发起流程的那个 Thread 里回复命令。".into(),
                    ));
                };
                if run.init.requester_mxid != command.sender_mxid {
                    return Ok((
                        "refused:not_requester".into(),
                        format!(
                            "GitFab：只有发起人 {} 可以确认 Spec 或决定人工介入。",
                            run.init.requester_mxid
                        ),
                    ));
                }
                if let Some(who) = to {
                    return self.reassign_to(command, run, &who, now).await;
                }
                match self.decide(&run.id, input, now).await {
                    Ok(state) => Ok((
                        "applied".into(),
                        format!(
                            "GitFab：已收到。流程 {} 当前状态：{}。",
                            run.id,
                            state_text(&state)
                        ),
                    )),
                    Err(Error::Conflict("orchestrator_decision_not_applicable")) => Ok((
                        "refused:not_applicable".into(),
                        format!(
                            "GitFab：流程当前状态为「{}」，不能执行这个命令。",
                            state_text(&run.state)
                        ),
                    )),
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// Personal bindings and pool assignments serving a Room: (role value, puppet, Agent id).
    async fn room_agents(&self, room_id: &str) -> Result<Vec<(String, String, String)>> {
        let mut agents: Vec<(String, String, String)> = self
            .transport
            .routing_scopes(room_id)
            .await?
            .into_iter()
            .filter(|s| s.active)
            .map(|s| (s.binding_id, s.puppet_mxid, s.agent_id))
            .collect();
        for (assignment, agent, puppet) in self.transport.shared_room_agents(room_id).await? {
            agents.push((assignment, puppet, agent));
        }
        Ok(agents)
    }

    /// `/gitfab reassign <Agent>`: the stage a Human holds goes to another Agent of the Room.
    async fn reassign_to(&self, command: &OrchestratorCommand, run: OrchestratorRun, who: &str, now: i64) -> Result<(String, String)> {
        let State::ManualIntervention { slot, .. } = run.state else {
            return Ok((
                "refused:not_applicable".into(),
                format!("GitFab：流程当前状态为「{}」，只有人工介入时才能改派。", state_text(&run.state)),
            ));
        };
        let agents = self.room_agents(&command.room_id).await?;
        let Some((binding, ..)) = agents.iter().find(|(id, puppet, agent)| puppet == who || id == who || agent == who) else {
            return Ok((
                "refused:unknown_agent".into(),
                format!("GitFab：{who} 不是本房间里正在服务的 Agent。"),
            ));
        };
        let role = role_key(slot.stage.role());
        if run.init.bindings[role] != binding.as_str() {
            self.transport
                .rebind_orchestrator_role(&run.id, run.revision, role, binding, now)
                .await?;
        }
        let state = self.decide(&run.id, Input::Human(HumanDecision::Reassign), now).await?;
        Ok((
            "applied".into(),
            format!("GitFab：已改派，{role} 由 {who} 接手。流程 {} 当前状态：{}。", run.id, state_text(&state)),
        ))
    }

    async fn start(
        &self,
        command: &OrchestratorCommand,
        root: &str,
        start: StartCommand,
        now: i64,
    ) -> Result<(String, String)> {
        let refuse = |reason: &str| {
            Ok((
                format!("refused:{reason}"),
                format!("GitFab：未能发起，{reason}"),
            ))
        };
        let Some(project) = self.transport.room_project(&command.room_id).await? else {
            return refuse("此房间不是已登记的项目房间。");
        };
        let agents = self.room_agents(&command.room_id).await?;
        let mut bindings = serde_json::Map::new();
        for (role, wanted) in ORCHESTRATOR_ROLES.iter().zip(&start.roles) {
            let found = match wanted.as_deref() {
                Some(who) => agents
                    .iter()
                    .find(|(id, puppet, agent)| puppet == who || id == who || agent == who),
                None if agents.len() == 1 => agents.first(),
                None => None,
            };
            let Some((id, ..)) = found else {
                return refuse(&match wanted {
                    Some(who) => format!("{role} 指定的 {who} 不是本房间里正在服务的 Agent。"),
                    None => format!(
                        "本房间有 {} 个 Agent，请用 `{role}: @Agent账号` 指定。",
                        agents.len()
                    ),
                });
            };
            bindings.insert((*role).to_owned(), json!(id));
        }
        // The Human must still be in the Room. Pool admissions check the requester themselves.
        let first = self
            .transport
            .routing_scopes(&command.room_id)
            .await?
            .into_iter()
            .find(|s| s.active);
        if let Some(first) = &first {
            let facts = self.room.facts(first, &command.sender_mxid).await?;
            if !facts.requester_in_room {
                return refuse("发起人不在本房间。");
            }
        }
        let mut context = json!({
            "goal": start.goal, "gitBackend": start.backend, "repoRef": start.repo,
            "baselineSha": start.baseline, "specRef": start.spec,
        });
        if let Some(code) = &start.project {
            context["projectId"] = json!(code);
        }
        let init = OrchestratorRunInit {
            project_id: project,
            task_id: start.task_id.clone(),
            room_id: command.room_id.clone(),
            thread_root: root.to_owned(),
            requester_mxid: command.sender_mxid.clone(),
            bindings: serde_json::Value::Object(bindings),
            context,
        };
        let run = match self.transport.create_orchestrator_run(&init, now).await {
            Ok(run) => run,
            Err(Error::Invalid(_)) => {
                return refuse("参数不完整或格式不对（基线须为 40 位提交 SHA）。");
            }
            Err(Error::Conflict("orchestrator_thread_in_use")) => {
                return refuse("本 Thread 已经有另一个任务的流程。");
            }
            Err(Error::Conflict("orchestrator_task_in_use")) => {
                return refuse(&format!(
                    "任务 {} 在本房间已有进行中的流程，请在它的 Thread 里继续，不会重复创建。",
                    start.task_id
                ));
            }
            Err(e) => return Err(e),
        };
        Ok((
            format!("started:{}", run.id),
            format!(
                "GitFab：已发起流程 {}（任务 {}）。先由 Coordinator 起草 Spec；Spec 回复后，请在本 Thread 发送 `/gitfab approve`、`/gitfab revise` 或 `/gitfab reject`。",
                run.id, start.task_id
            ),
        ))
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
                    let row = match shared_dispatch_id(dispatch) {
                        Some((assignment, event)) => {
                            self.transport.shared_dispatch(assignment, event).await?
                        }
                        None => self.transport.orchestrated_dispatch(dispatch).await?,
                    };
                    // A dispatch the transport no longer has cannot be shown to have run.
                    let outcome = match &row {
                        Some(row) => flow::observed(row, slot.stage),
                        None => Some(Outcome::Unknown),
                    };
                    // A Plan whose Modules need what the Implementer does not declare cannot be
                    // executed by the fixed team; it is not a finished Plan.
                    let outcome = match (outcome, self.team, row.as_ref()) {
                        (Some(Outcome::Replied(_)), Some(team), Some(row))
                            if slot.stage == Stage::Plan
                                && crate::orchestrator_modules::modules(
                                    row.reply_body.as_deref().unwrap_or(""),
                                )
                                .is_ok_and(|m| !team.uncovered(&m).is_empty()) =>
                        {
                            Some(Outcome::Failed)
                        }
                        (outcome, ..) => outcome,
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
        let run_state = run.state.clone();
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
        // A stage that just succeeded may drive the Git platform; its links go into that event.
        let git = match completed {
            Some(slot)
                if crate::orchestrator_git::drives(slot.stage)
                    && actions.iter().any(|a| {
                        matches!(a, Action::Record { phase, status: "succeeded", round }
                            if *phase == slot.stage.phase() && *round == slot.round)
                    }) =>
            {
                match crate::orchestrator_git::target(self.room.git(), &run) {
                    Some((host, repo)) => Some((
                        slot,
                        crate::orchestrator_git::step(host, &repo, &run, slot.stage, slot.round)
                            .await,
                    )),
                    None => None,
                }
            }
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
            // The Spec version the record's stage message worked from (Annex 1: Spec versioned).
            let mut spec_revision = None;
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
                        Some((slot, rev, prior)) => {
                            spec_revision = Some(rev);
                            sha256(&stage_message(&run, slot, rev, prior, self.team))
                        }
                        None => sha256(&run.init.context.to_string()),
                    }
                }
            };
            let c = &run.init.context;
            let mut event = json!({
                "schema_version": hagency_gitfab_events::schema::TASK,
                "event_id": format!("{}.{revision}.{index}", run.id),
                "timestamp": timestamp(now),
                // The acceptance project code (DEMO-01…) when given, else the Hagency project.
                "project_id": c["projectId"].as_str().unwrap_or(&run.init.project_id),
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
                "artifact_refs": match &git {
                    Some((slot, step)) if finishes_completed && status == "succeeded"
                        && slot.stage.phase() == phase => json!(step.refs),
                    _ => json!([]),
                },
                "request_id": run.dispatch_id.clone().unwrap_or_else(|| run.id.clone()),
                "signature": null,
                "round": round,
            });
            if let Some((slot, step)) = &git
                && finishes_completed
                && status == "succeeded"
                && slot.stage.phase() == phase
            {
                if let Some(sha) = &step.commit {
                    event["commit_sha"] = json!(sha);
                }
                if let Some(error) = &step.error {
                    event["platform_error"] = json!(truncate(error, FINDINGS_LIMIT));
                }
            }
            if let Some(rev) = spec_revision {
                event["spec_revision"] = json!(rev);
            }
            if phase == "review"
                && let Some(reply) = reply.filter(|_| finishes_completed)
            {
                event["findings"] = findings(reply);
            }
            // The Plan's Module breakdown is the task card list (Annex 1 Chapter 2).
            if phase == "plan"
                && status == "succeeded"
                && let Some(Ok(modules)) = reply
                    .filter(|_| finishes_completed)
                    .map(crate::orchestrator_modules::modules)
            {
                event["modules"] = json!(modules);
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
        if let Some((
            slot,
            crate::orchestrator_git::GitStep {
                note: Some(note), ..
            },
        )) = &git
        {
            let meta = json!({"version": 1, "runId": advanced.id, "taskId": advanced.init.task_id,
                "stage": slot.stage.phase(), "round": slot.round, "git": true});
            let txn = format!(
                "{}.git.{}.{}.{revision}",
                advanced.id,
                slot.stage.phase(),
                slot.round
            );
            if let Err(e) = self
                .room
                .post(
                    &advanced.init.room_id,
                    &advanced.init.thread_root,
                    &txn,
                    note,
                    &meta,
                )
                .await
            {
                tracing::warn!(error = %e, "gitfab Git note was not posted");
            }
        }
        // Waiting on a Human or finished: say so in the thread. Best effort, like the Git note; the
        // signed journal is the record.
        if advanced.state != run_state
            && let Some(notice) = status_notice(&advanced.id, &advanced.state)
        {
            let meta = json!({"version": 1, "kind": "status", "runId": advanced.id,
                "taskId": advanced.init.task_id, "state": advanced.state.to_json()});
            let txn = format!("{}.status.{revision}", advanced.id);
            if let Err(e) = self
                .room
                .post(
                    &advanced.init.room_id,
                    &advanced.init.thread_root,
                    &txn,
                    &notice,
                    &meta,
                )
                .await
            {
                tracing::warn!(error = %e, "gitfab status notice was not posted");
            }
        }
        Ok(Some(OrchestratorRun {
            pending_events: Value::Array(Vec::new()),
            ..advanced
        }))
    }

    /// Posts the stage message and admits it for a pool Agent (`binding` is its assignment). A message
    /// the shared routing does not take counts as a stage that expired before it ran.
    async fn dispatch_shared(
        &self,
        run: OrchestratorRun,
        slot: Slot,
        spec_revision: u32,
        assignment: &str,
        now: i64,
    ) -> Result<Option<OrchestratorRun>> {
        let refused = Input::Result {
            slot,
            outcome: Outcome::Expired,
        };
        let agents = self.transport.shared_room_agents(&run.init.room_id).await?;
        let Some((.., puppet)) = agents.into_iter().find(|(a, ..)| a == assignment) else {
            return self.apply(run, refused, Cause::System, now).await;
        };
        let body = stage_message(&run, slot, spec_revision, run.handoff.as_deref(), self.team);
        let meta = json!({"version": 1, "runId": run.id, "taskId": run.init.task_id,
            "stage": slot.stage.phase(), "round": slot.round, "attempt": slot.attempt,
            "specRevision": spec_revision});
        let event_id = self
            .room
            .post(
                &run.init.room_id,
                &run.init.thread_root,
                &txn_id(&run, slot, spec_revision),
                &body,
                &meta,
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
            .register_orchestrated_stage(assignment, &event_id, &stage, now)
            .await?;
        let event = RoutedEvent {
            event_id: event_id.clone(),
            room_id: run.init.room_id.clone(),
            sender_mxid: run.init.requester_mxid.clone(),
            body,
            mentioned_mxids: [puppet].into(),
            thread_root: Some(run.init.thread_root.clone()),
            encrypted: false,
            is_edit: false,
        };
        match self.room.admit_shared(&event, now).await {
            Ok(()) => {}
            Err(Error::Unauthorized(_) | Error::Invalid(_) | Error::Conflict(_)) => {
                return self.apply(run, refused, Cause::System, now).await;
            }
            Err(e) => return Err(e),
        }
        // Shared routing skips an event that no runtime, membership or budget allows without an error.
        if self
            .transport
            .shared_dispatch(assignment, &event_id)
            .await?
            .is_none()
        {
            return self.apply(run, refused, Cause::System, now).await;
        }
        let dispatch_id = format!("{SHARED_DISPATCH}{assignment}:{event_id}");
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

    async fn agent_actor(&self, run: &OrchestratorRun, stage: Stage) -> String {
        let Ok(binding) = binding_for(run, stage) else {
            return SYSTEM_ACTOR.to_owned();
        };
        if binding.starts_with(SHARED_PREFIX) {
            return match self.transport.shared_room_agents(&run.init.room_id).await {
                Ok(agents) => agents
                    .into_iter()
                    .find(|(a, ..)| a == binding)
                    .map_or_else(|| binding.to_owned(), |(.., puppet)| puppet),
                Err(_) => binding.to_owned(),
            };
        }
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
        if binding.starts_with(SHARED_PREFIX) {
            return self
                .dispatch_shared(run, slot, spec_revision, &binding, now)
                .await;
        }
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
        let body = stage_message(&run, slot, spec_revision, run.handoff.as_deref(), self.team);
        let meta = json!({"version": 1, "runId": run.id, "taskId": run.init.task_id,
            "stage": slot.stage.phase(), "round": slot.round, "attempt": slot.attempt,
            "specRevision": spec_revision});
        let event_id = self
            .room
            .post(
                &run.init.room_id,
                &run.init.thread_root,
                &txn_id(&run, slot, spec_revision),
                &body,
                &meta,
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

/// How to use the Room commands, shown on `/gitfab help` and with every malformed command.
pub(crate) const USAGE: &str = "用法：\n\
`/gitfab start <任务ID>` 后面每行一项：\n\
  repo: 仓库 URL\n\
  baseline: 40 位基线提交 SHA\n\
  spec: Spec 引用（如 spec/DEMO-02.md@v1）\n\
  backend: github 或 gitcode（可从仓库 URL 推断）\n\
  project: 验收项目编号（可选，如 DEMO-02）\n\
  coordinator / implementer / reviewer: Agent 账号（房间里只有一个 Agent 时可省略）\n\
  goal: 目标（这一行之后的内容都算目标）\n\
在流程的 Thread 里：`/gitfab approve`、`/gitfab revise`、`/gitfab reject` 处理 Spec；`/gitfab continue`、`/gitfab reassign [@Agent账号]`、`/gitfab abort` 处理人工介入（reassign 指定账号时由该 Agent 接手）。";

/// A message is a command when its first word is `/gitfab`.
pub(crate) fn is_command(body: &str) -> bool {
    let body = body.trim_start();
    body.strip_prefix("/gitfab")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StartCommand {
    pub task_id: String,
    pub project: Option<String>,
    pub repo: String,
    pub backend: String,
    pub baseline: String,
    pub spec: String,
    /// Coordinator, implementer, reviewer, as written.
    pub roles: [Option<String>; 3],
    pub goal: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Start(Box<StartCommand>),
    Decide(Input),
    /// `/gitfab reassign <Agent>`: the held stage goes to that Agent.
    Reassign(String, Input),
    Help,
}


/// Reads a `/gitfab` message. Errors are shown to the Human as they are.
pub(crate) fn parse_command(body: &str) -> std::result::Result<Command, &'static str> {
    let mut lines = body.trim().lines();
    let head = lines.next().unwrap_or_default();
    let mut words = head.split_whitespace().skip(1);
    let verb = words.next().unwrap_or("help").to_ascii_lowercase();
    let mut decision = |input| {
        if words.next().is_some() {
            Err("命令后面不需要参数。")
        } else {
            Ok(Command::Decide(input))
        }
    };
    match verb.as_str() {
        "help" => return Ok(Command::Help),
        "approve" => return decision(Input::SpecDecision(SpecDecision::Approve)),
        "reject" => return decision(Input::SpecDecision(SpecDecision::Reject)),
        "revise" => return decision(Input::SpecDecision(SpecDecision::Revise)),
        "continue" => return decision(Input::Human(HumanDecision::Continue)),
        "reassign" => {
            let input = Input::Human(HumanDecision::Reassign);
            return match (words.next(), words.next()) {
                (None, _) => Ok(Command::Decide(input)),
                (Some(who), None) => Ok(Command::Reassign(who.to_owned(), input)),
                _ => Err("改派只能指定一个 Agent。"),
            };
        }
        "abort" => return decision(Input::Human(HumanDecision::Abort)),
        "start" => {}
        _ => return Err("不认识这个命令。"),
    }
    let task_id = words.next().ok_or("缺少任务ID。")?.to_owned();
    if words.next().is_some() || task_id.len() > 128 {
        return Err("任务ID 只能是一个词。");
    }
    let (mut fields, mut goal) = (std::collections::BTreeMap::new(), None::<String>);
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .or_else(|| line.split_once('：'))
            .ok_or("每行应为「名称: 值」。")?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        if key == "goal" {
            goal = Some(value);
            break;
        }
        if ![
            "project",
            "repo",
            "backend",
            "baseline",
            "spec",
            "coordinator",
            "implementer",
            "reviewer",
        ]
        .contains(&key.as_str())
        {
            return Err("有不认识的项目名称。");
        }
        if fields.insert(key, value).is_some() {
            return Err("同一项写了两次。");
        }
    }
    let rest: Vec<&str> = lines.collect();
    let goal = match (goal, rest.is_empty()) {
        (Some(first), true) => first,
        (Some(first), false) => format!("{first}\n{}", rest.join("\n")).trim().to_owned(),
        (None, _) => return Err("缺少 goal。"),
    };
    let mut take = |k: &str| fields.remove(k).filter(|v| !v.is_empty());
    let repo = take("repo").ok_or("缺少 repo。")?;
    let backend = match take("backend") {
        Some(b) => b.to_ascii_lowercase(),
        None if repo.contains("github.com") => "github".into(),
        None if repo.contains("gitcode") => "gitcode".into(),
        None => return Err("无法从仓库 URL 判断 backend，请写 backend: github 或 gitcode。"),
    };
    Ok(Command::Start(Box::new(StartCommand {
        task_id,
        project: take("project"),
        backend,
        baseline: take("baseline")
            .ok_or("缺少 baseline。")?
            .to_ascii_lowercase(),
        spec: take("spec").ok_or("缺少 spec。")?,
        roles: [take("coordinator"), take("implementer"), take("reviewer")],
        repo,
        goal: if goal.is_empty() {
            return Err("goal 不能为空。");
        } else {
            goal
        },
    })))
}

/// The thread notice for a run that now waits on a Human or has ended (Annex 1 §1.2.1(3):
/// Supervisor status visible where people work). `None` for states that a stage message announces.
fn status_notice(run_id: &str, state: &State) -> Option<String> {
    Some(match state {
        State::AwaitingSpecApproval { spec_revision } => format!(
            "GitFab：Spec 第 {spec_revision} 版已出稿，等待发起人确认：/gitfab approve、/gitfab revise 或 /gitfab reject。"
        ),
        State::ManualIntervention { slot, reason, .. } => format!(
            "GitFab：{} 阶段（第 {} 轮）转入人工介入（{reason}）。可用 /gitfab continue、/gitfab reassign [@Agent账号] 或 /gitfab abort。",
            slot.stage.phase(),
            slot.round
        ),
        State::Completed => format!("GitFab：流程 {run_id} 已完成。"),
        State::Failed { reason } => format!("GitFab：流程 {run_id} 已结束（{reason}）。"),
        State::NotStarted | State::Running { .. } => return None,
    })
}

fn state_text(state: &State) -> String {
    match state {
        State::NotStarted => "未开始".into(),
        State::Running { slot, .. } => {
            format!("{} 阶段进行中（第 {} 轮）", slot.stage.phase(), slot.round)
        }
        State::AwaitingSpecApproval { spec_revision } => {
            format!("等待确认 Spec 第 {spec_revision} 版")
        }
        State::ManualIntervention { reason, .. } => format!("人工介入（{reason}）"),
        State::Completed => "已完成".into(),
        State::Failed { reason } => format!("已结束（{reason}）"),
    }
}

/// A thread notice with the stage described for clients, and no mention.
pub(crate) fn stage_content(room_id: &str, thread_root: &str, body: &str, meta: &Value) -> Value {
    let mut content = crate::workers::reply_content(false, room_id, thread_root, body);
    content["msgtype"] = json!("m.notice");
    content["org.hagency.gitfab_stage"] = meta.clone();
    content
}

/// The Room as the service sees it: stage messages are notices from `@_hagency_service`, so neither
/// ordinary nor pooled routing takes them, and they carry no mention.
pub(crate) struct MatrixStageRoom<'a> {
    pub gateway: &'a crate::gateway::Gateway,
    pub matrix: &'a crate::matrix_client::MatrixClient,
    /// Where pool Agents execute: the domain and management policy. `None` when shared execution is
    /// not configured, and pool roles are then refused.
    pub shared: Option<(
        &'a crate::domain::DomainStore,
        &'a crate::management_types::ManagementPolicy,
    )>,
    /// Backends with a configured token.
    pub git: Option<&'a crate::orchestrator_git::GitHosts>,
}

impl StageRoom for MatrixStageRoom<'_> {
    fn git(&self) -> Option<&crate::orchestrator_git::GitHosts> {
        self.git
    }

    async fn post(
        &self,
        room_id: &str,
        thread_root: &str,
        txn: &str,
        body: &str,
        meta: &Value,
    ) -> Result<String> {
        let content = stage_content(room_id, thread_root, body, meta);
        self.matrix
            .send(self.matrix.service_mxid(), room_id, txn, content)
            .await
    }

    async fn admit_shared(&self, event: &RoutedEvent, received_at_ms: i64) -> Result<()> {
        let (domain, policy) = self
            .shared
            .ok_or(Error::Unauthorized("shared_execution_unavailable"))?;
        tokio::time::timeout(
            std::time::Duration::from_secs(8),
            domain.route_shared_event(policy, event, received_at_ms, self.gateway),
        )
        .await
        .map_err(|_| Error::Unavailable("matrix_state_unavailable"))?
    }

    async fn facts(&self, scope: &RoutingScope, requester: &str) -> Result<DeliveryFacts> {
        tokio::time::timeout(
            std::time::Duration::from_secs(8),
            self.gateway.delivery(
                &scope.room_id,
                &scope.space_id,
                &scope.owner_mxid,
                requester,
                &scope.puppet_mxid,
            ),
        )
        .await
        .map_err(|_| Error::Unavailable("matrix_state_unavailable"))?
    }
}

/// The orchestrator worker: one pass every five seconds until shutdown.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_controlled(
    transport: TransportStore,
    gateway: crate::gateway::Gateway,
    matrix: crate::matrix_client::MatrixClient,
    shared: Option<(
        crate::domain::DomainStore,
        crate::management_types::ManagementPolicy,
    )>,
    recorder: Arc<Recorder>,
    stage_timeout_ms: i64,
    git: Option<Arc<crate::orchestrator_git::GitHosts>>,
    team: Option<Arc<crate::orchestrator_team::Team>>,
    control: crate::worker_runtime::WorkerRuntime,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let ctx = control.queue("gitfab_orchestrator");
        let room = MatrixStageRoom {
            gateway: &gateway,
            matrix: &matrix,
            shared: shared.as_ref().map(|(d, p)| (d, p)),
            git: git.as_deref(),
        };
        let orchestrator = Orchestrator {
            transport: &transport,
            room: &room,
            recorder: &recorder,
            stage_timeout_ms,
            team: team.as_deref(),
        };
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { biased; _ = control.stopped() => break, _ = interval.tick() => {} }
            if ctx.stopping() {
                break;
            }
            let result = orchestrator.pass(crate::api::now_ms()).await;
            ctx.finish(&result);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_messages_are_thread_notices_that_no_router_takes_and_that_mention_nobody() {
        let meta = json!({"version": 1, "stage": "review"});
        let content = stage_content("!room:example.test", "$root", "[GitFab review] body", &meta);
        assert_eq!(content["msgtype"], "m.notice");
        assert_eq!(content["m.relates_to"]["rel_type"], "m.thread");
        assert_eq!(content["m.relates_to"]["event_id"], "$root");
        assert!(content.get("m.mentions").is_none());
        assert_eq!(content["org.hagency.gitfab_stage"], meta);
        let raw = json!({"type": "m.room.message", "event_id": "$stage", "room_id": "!room:example.test",
            "sender": "@_hagency_service:example.test", "content": content});
        assert!(
            crate::workers::parse_event(&raw).is_none(),
            "ordinary and pooled routing ignore it"
        );
    }

    #[test]
    fn a_long_review_keeps_its_verdict_line_in_the_findings() {
        assert_eq!(findings("ok\nVERDICT: approve"), json!(["ok\nVERDICT: approve"]));
        assert_eq!(findings("no verdict"), json!(["no verdict"]));
        let long = format!("{}\nVERDICT: reject\n", "标准 1：未满足。\n".repeat(400));
        let kept = findings(&long);
        assert_eq!(kept.as_array().unwrap().len(), 2);
        assert!(kept[0].as_str().unwrap().len() <= FINDINGS_LIMIT);
        assert_eq!(kept[1], "VERDICT: reject");
    }

    #[test]
    fn commands_are_recognised_only_by_their_first_word() {
        assert!(is_command("/gitfab"));
        assert!(is_command("  /gitfab approve"));
        assert!(!is_command("/gitfabulous"));
        assert!(!is_command("please /gitfab approve"));
    }

    #[test]
    fn reassign_takes_at_most_one_agent() {
        let input = Input::Human(HumanDecision::Reassign);
        assert_eq!(parse_command("/gitfab reassign").unwrap(), Command::Decide(input));
        assert_eq!(
            parse_command("/gitfab reassign @_hagency_agt_b:example.test").unwrap(),
            Command::Reassign("@_hagency_agt_b:example.test".into(), input)
        );
        assert!(parse_command("/gitfab reassign @a:s @b:s").is_err());
        assert!(parse_command("/gitfab continue @a:s").is_err(), "only reassign names an Agent");
    }

    #[test]
    fn a_start_command_reads_its_fields_and_the_rest_is_the_goal() {
        let body = "/gitfab start DEMO-02-task-1\nproject: DEMO-02\nrepo: https://github.com/example/gitfab-demo-02\nbaseline: ABCDEF0123456789abcdef0123456789abcdef01\nspec：spec/DEMO-02.md@v1\nreviewer: @_hagency_agt_r:example.test\ngoal: 实现任务清单 API\n见 PRD-DEMO-02-v1。";
        let Command::Start(start) = parse_command(body).unwrap() else {
            panic!("expected start")
        };
        assert_eq!(start.task_id, "DEMO-02-task-1");
        assert_eq!(start.project.as_deref(), Some("DEMO-02"));
        assert_eq!(start.backend, "github", "inferred from the repository");
        assert_eq!(start.baseline, "abcdef0123456789abcdef0123456789abcdef01");
        assert_eq!(
            start.spec, "spec/DEMO-02.md@v1",
            "a full-width colon works too"
        );
        assert_eq!(
            start.roles,
            [None, None, Some("@_hagency_agt_r:example.test".into())]
        );
        assert_eq!(start.goal, "实现任务清单 API\n见 PRD-DEMO-02-v1。");
    }

    #[test]
    fn decisions_and_mistakes_are_read_strictly() {
        assert_eq!(
            parse_command("/gitfab approve").unwrap(),
            Command::Decide(Input::SpecDecision(SpecDecision::Approve))
        );
        assert_eq!(
            parse_command("/gitfab ABORT").unwrap(),
            Command::Decide(Input::Human(HumanDecision::Abort))
        );
        assert_eq!(parse_command("/gitfab").unwrap(), Command::Help);
        assert!(parse_command("/gitfab approve now").is_err());
        assert!(parse_command("/gitfab merge").is_err());
        assert!(
            parse_command(
                "/gitfab start t1\nrepo: https://example.org/x\nbaseline: a\nspec: s\ngoal: g"
            )
            .is_err(),
            "backend cannot be inferred"
        );
        assert!(
            parse_command("/gitfab start t1\nrepo: https://github.com/x/y\nspec: s\ngoal: g")
                .is_err(),
            "baseline is required"
        );
        assert!(
            parse_command("/gitfab start t1\nrepo: https://github.com/x/y\nbaseline: a\nspec: s")
                .is_err(),
            "goal is required"
        );
        assert!(parse_command("/gitfab start t1\ncolour: red\ngoal: g").is_err());
    }

    #[test]
    fn a_run_that_waits_on_a_human_or_ends_says_so_in_its_thread() {
        let slot = Slot {
            stage: Stage::Review,
            round: 2,
            attempt: 1,
        };
        let manual = State::ManualIntervention {
            slot,
            spec_revision: 1,
            reason: "rework_limit",
        };
        let text = status_notice("run1", &manual).unwrap();
        assert!(
            text.contains("review 阶段（第 2 轮）转入人工介入（rework_limit）")
                && text.contains("/gitfab continue")
        );
        assert!(
            status_notice("run1", &State::AwaitingSpecApproval { spec_revision: 3 })
                .unwrap()
                .contains("Spec 第 3 版")
        );
        assert_eq!(
            status_notice("run1", &State::Completed).as_deref(),
            Some("GitFab：流程 run1 已完成。")
        );
        assert!(
            status_notice(
                "run1",
                &State::Failed {
                    reason: "stage_failed"
                }
            )
            .unwrap()
            .contains("stage_failed")
        );
        assert_eq!(
            status_notice(
                "run1",
                &State::Running {
                    slot,
                    spec_revision: 1
                }
            ),
            None
        );
        assert_eq!(status_notice("run1", &State::NotStarted), None);
    }

    #[test]
    fn truncation_keeps_whole_characters() {
        assert_eq!(truncate("评审结论", 7), "评审");
        assert_eq!(truncate("ok", 7), "ok");
    }
}
