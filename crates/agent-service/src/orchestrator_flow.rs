//! The GitFab workflow as a pure transition function: one run moves spec → plan → execute → test → review
//! → release. Storage, dispatch and the event journal act on the returned actions; nothing here performs
//! I/O. See docs/design/2026-11-12-gitfab-orchestrator.zh-CN.md.
//!
//! Rules from the agreement (Annex 1 §1.2.1(3), Annex 2 §3.1, §6, §7.1):
//! - execution waits for a designated Human to confirm the Spec; rejection ends the run, revision
//!   re-drafts it;
//! - a Reviewer rejection returns to execution in a new round, at most two reworks, then Manual
//!   Intervention with the findings kept;
//! - a stage that failed or expired before running is retried once as a new dispatch;
//! - an unknown outcome or a stage timeout is never re-dispatched automatically: Manual Intervention;
//! - a result for a dispatch other than the current one is ignored.

/// Reworks after the first execution (Annex 1 §1.2.1(3)).
pub const MAX_REWORKS: u32 = 2;
/// Dispatches of one stage in one round before the run fails.
pub const MAX_ATTEMPTS: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Spec,
    Plan,
    Execute,
    Test,
    Review,
    Release,
}

impl Stage {
    /// The `phase` recorded in gitfab.task.v1 events.
    pub fn phase(self) -> &'static str {
        match self {
            Self::Spec => "spec",
            Self::Plan => "plan",
            Self::Execute => "execute",
            Self::Test => "test",
            Self::Review => "review",
            Self::Release => "release",
        }
    }

    /// The fixed capability route (Annex 1 §1.2.1(3)); Agents are matched by their workflow role.
    pub fn role(self) -> Role {
        match self {
            Self::Spec | Self::Plan | Self::Release => Role::Coordinator,
            Self::Execute | Self::Test => Role::Implementer,
            Self::Review => Role::Reviewer,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Coordinator,
    Implementer,
    Reviewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Reject,
    Blocked,
}

/// What became of one dispatch, as observed from the transport tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The Agent replied; a Reviewer reply carries its verdict.
    Replied(Option<Verdict>),
    /// Finished without a reply (failed or rejected by local policy).
    Failed,
    /// Expired or cancelled before it started.
    Expired,
    /// Started, result not known. Never re-run.
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecDecision {
    Approve,
    Reject,
    Revise,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HumanDecision {
    Continue,
    Abort,
    Reassign,
}

/// Identifies one dispatch of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub stage: Stage,
    /// 0 for spec and plan; 1 for the first execution, +1 per rework.
    pub round: u32,
    pub attempt: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    NotStarted,
    Running {
        slot: Slot,
        spec_revision: u32,
    },
    AwaitingSpecApproval {
        spec_revision: u32,
    },
    ManualIntervention {
        slot: Slot,
        spec_revision: u32,
        reason: &'static str,
    },
    Completed,
    Failed {
        reason: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Start,
    Result { slot: Slot, outcome: Outcome },
    SpecDecision(SpecDecision),
    Timeout { slot: Slot },
    Human(HumanDecision),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Dispatch {
        slot: Slot,
        role: Role,
        spec_revision: u32,
        reassigned: bool,
    },
    AwaitSpecApproval {
        spec_revision: u32,
    },
    /// A gitfab.task.v1 event to append.
    Record {
        phase: &'static str,
        status: &'static str,
        round: u32,
    },
    Finish {
        status: &'static str,
    },
}

fn dispatch(slot: Slot, spec_revision: u32, reassigned: bool) -> Action {
    Action::Dispatch {
        slot,
        role: slot.stage.role(),
        spec_revision,
        reassigned,
    }
}

fn record(stage: Stage, status: &'static str, round: u32) -> Action {
    Action::Record {
        phase: stage.phase(),
        status,
        round,
    }
}

fn run(slot: Slot, spec_revision: u32) -> State {
    State::Running {
        slot,
        spec_revision,
    }
}

fn first(stage: Stage, round: u32) -> Slot {
    Slot {
        stage,
        round,
        attempt: 1,
    }
}

fn manual(slot: Slot, spec_revision: u32, reason: &'static str) -> (State, Vec<Action>) {
    (
        State::ManualIntervention {
            slot,
            spec_revision,
            reason,
        },
        vec![record(slot.stage, "manual_intervention", slot.round)],
    )
}

fn advance(
    slot: Slot,
    spec_revision: u32,
    next: Stage,
    round: u32,
    status: &'static str,
) -> (State, Vec<Action>) {
    let to = first(next, round);
    (
        run(to, spec_revision),
        vec![
            record(slot.stage, status, slot.round),
            record(next, "running", round),
            dispatch(to, spec_revision, false),
        ],
    )
}

/// Returns the next state and what to do. An input that does not apply to the state changes nothing.
pub fn next(state: &State, input: Input) -> (State, Vec<Action>) {
    let unchanged = || (state.clone(), Vec::new());
    match (state, input) {
        (State::NotStarted, Input::Start) => {
            let slot = first(Stage::Spec, 0);
            (
                run(slot, 1),
                vec![
                    Action::Record {
                        phase: "intake",
                        status: "succeeded",
                        round: 0,
                    },
                    record(Stage::Spec, "running", 0),
                    dispatch(slot, 1, false),
                ],
            )
        }
        (
            State::Running {
                slot,
                spec_revision,
            },
            Input::Result {
                slot: done,
                outcome,
            },
        ) if done == *slot => {
            let (slot, rev) = (*slot, *spec_revision);
            match outcome {
                Outcome::Unknown => manual(slot, rev, "unknown_outcome"),
                Outcome::Failed | Outcome::Expired if slot.attempt < MAX_ATTEMPTS => {
                    let retry = Slot {
                        attempt: slot.attempt + 1,
                        ..slot
                    };
                    (run(retry, rev), vec![dispatch(retry, rev, false)])
                }
                Outcome::Failed | Outcome::Expired => (
                    State::Failed {
                        reason: "stage_failed",
                    },
                    vec![
                        record(slot.stage, "failed", slot.round),
                        Action::Finish { status: "failed" },
                    ],
                ),
                Outcome::Replied(verdict) => replied(slot, rev, verdict),
            }
        }
        (
            State::Running {
                slot,
                spec_revision,
            },
            Input::Timeout { slot: late },
        ) if late == *slot => manual(*slot, *spec_revision, "stage_timeout"),
        (State::AwaitingSpecApproval { spec_revision }, Input::SpecDecision(decision)) => {
            let rev = *spec_revision;
            let spec = first(Stage::Spec, 0);
            match decision {
                SpecDecision::Approve => advance(spec, rev, Stage::Plan, 0, "succeeded"),
                SpecDecision::Reject => (
                    State::Failed {
                        reason: "spec_rejected",
                    },
                    vec![
                        record(Stage::Spec, "failed", 0),
                        Action::Finish { status: "failed" },
                    ],
                ),
                // A revised Spec is a new version that must be confirmed again (Annex 2 §10).
                SpecDecision::Revise => (
                    run(spec, rev + 1),
                    vec![
                        record(Stage::Spec, "running", 0),
                        dispatch(spec, rev + 1, false),
                    ],
                ),
            }
        }
        (
            State::ManualIntervention {
                slot,
                spec_revision,
                ..
            },
            Input::Human(decision),
        ) => match decision {
            HumanDecision::Abort => (
                State::Failed { reason: "aborted" },
                vec![
                    record(slot.stage, "failed", slot.round),
                    Action::Finish { status: "failed" },
                ],
            ),
            HumanDecision::Continue | HumanDecision::Reassign => {
                // A Human decision is the only way past an unknown outcome; it is a new dispatch, never
                // a replay of the old one.
                let again = Slot {
                    attempt: slot.attempt + 1,
                    ..*slot
                };
                let reassigned = decision == HumanDecision::Reassign;
                (
                    run(again, *spec_revision),
                    vec![
                        record(slot.stage, "running", slot.round),
                        dispatch(again, *spec_revision, reassigned),
                    ],
                )
            }
        },
        _ => unchanged(),
    }
}

fn replied(slot: Slot, rev: u32, verdict: Option<Verdict>) -> (State, Vec<Action>) {
    match slot.stage {
        Stage::Spec => (
            State::AwaitingSpecApproval { spec_revision: rev },
            vec![
                record(Stage::Spec, "blocked", 0),
                Action::AwaitSpecApproval { spec_revision: rev },
            ],
        ),
        Stage::Plan => advance(slot, rev, Stage::Execute, 1, "succeeded"),
        Stage::Execute => advance(slot, rev, Stage::Test, slot.round, "succeeded"),
        Stage::Test => advance(slot, rev, Stage::Review, slot.round, "succeeded"),
        Stage::Review => match verdict {
            Some(Verdict::Approve) => advance(slot, rev, Stage::Release, slot.round, "succeeded"),
            Some(Verdict::Reject) if slot.round <= MAX_REWORKS => {
                advance(slot, rev, Stage::Execute, slot.round + 1, "failed")
            }
            Some(Verdict::Reject) => {
                let (state, mut actions) = manual(slot, rev, "rework_limit");
                actions.insert(0, record(Stage::Review, "failed", slot.round));
                (state, actions)
            }
            Some(Verdict::Blocked) => {
                let (state, mut actions) = manual(slot, rev, "review_blocked");
                actions.insert(0, record(Stage::Review, "blocked", slot.round));
                (state, actions)
            }
            // A Reviewer reply without a verdict cannot be read as approval.
            None => manual(slot, rev, "review_without_verdict"),
        },
        Stage::Release => (
            State::Completed,
            vec![
                record(Stage::Release, "succeeded", slot.round),
                Action::Finish {
                    status: "succeeded",
                },
            ],
        ),
    }
}

/// Every reason a run stops, so a stored state reads back to the same static text.
const REASONS: &[&str] = &[
    "unknown_outcome",
    "stage_timeout",
    "stage_failed",
    "spec_rejected",
    "aborted",
    "rework_limit",
    "review_blocked",
    "review_without_verdict",
];

impl Stage {
    pub fn from_phase(phase: &str) -> Option<Self> {
        [
            Self::Spec,
            Self::Plan,
            Self::Execute,
            Self::Test,
            Self::Review,
            Self::Release,
        ]
        .into_iter()
        .find(|s| s.phase() == phase)
    }
}

impl Slot {
    fn to_json(self) -> serde_json::Value {
        serde_json::json!({"stage": self.stage.phase(), "round": self.round, "attempt": self.attempt})
    }

    fn from_json(v: &serde_json::Value) -> Option<Self> {
        Some(Self {
            stage: Stage::from_phase(v["stage"].as_str()?)?,
            round: u32::try_from(v["round"].as_u64()?).ok()?,
            attempt: u32::try_from(v["attempt"].as_u64()?)
                .ok()
                .filter(|a| *a >= 1)?,
        })
    }
}

impl State {
    /// The stored form of a run's state (`orchestrator_runs.state`).
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::NotStarted => json!({"kind": "not_started"}),
            Self::Running {
                slot,
                spec_revision,
            } => json!({"kind": "running", "slot": slot.to_json(), "specRevision": spec_revision}),
            Self::AwaitingSpecApproval { spec_revision } => {
                json!({"kind": "awaiting_spec_approval", "specRevision": spec_revision})
            }
            Self::ManualIntervention {
                slot,
                spec_revision,
                reason,
            } => {
                json!({"kind": "manual_intervention", "slot": slot.to_json(), "specRevision": spec_revision, "reason": reason})
            }
            Self::Completed => json!({"kind": "completed"}),
            Self::Failed { reason } => json!({"kind": "failed", "reason": reason}),
        }
    }

    /// Reads a stored state back. Anything malformed or unknown is refused rather than guessed.
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        let revision = || {
            v["specRevision"]
                .as_u64()
                .and_then(|r| u32::try_from(r).ok())
                .filter(|r| *r >= 1)
        };
        let reason = || {
            v["reason"]
                .as_str()
                .and_then(|r| REASONS.iter().copied().find(|known| *known == r))
        };
        Some(match v["kind"].as_str()? {
            "not_started" => Self::NotStarted,
            "running" => Self::Running {
                slot: Slot::from_json(&v["slot"])?,
                spec_revision: revision()?,
            },
            "awaiting_spec_approval" => Self::AwaitingSpecApproval {
                spec_revision: revision()?,
            },
            "manual_intervention" => Self::ManualIntervention {
                slot: Slot::from_json(&v["slot"])?,
                spec_revision: revision()?,
                reason: reason()?,
            },
            "completed" => Self::Completed,
            "failed" => Self::Failed { reason: reason()? },
            _ => return None,
        })
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed { .. })
    }
}

/// One dispatch as recorded by the transport tables (`owner_events` joined with its `reply_outbox`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchRow {
    pub state: String,
    pub outcome: Option<String>,
    pub reply_state: Option<String>,
    pub reply_blocked: bool,
    pub reply_body: Option<String>,
}

/// What became of a dispatch, or `None` while it is still in progress. Anything not plainly a success or
/// a known failure is `Unknown`, so the run stops for a Human rather than guessing.
pub fn observed(row: &DispatchRow, stage: Stage) -> Option<Outcome> {
    match row.state.as_str() {
        "pending" | "offered" | "acknowledged" | "running" => None,
        "cancelled" => Some(Outcome::Expired),
        "completed" => match row.outcome.as_deref() {
            Some("replied") => match (row.reply_state.as_deref(), row.reply_blocked) {
                (_, true) => Some(Outcome::Unknown),
                (Some("sent"), false) => {
                    let verdict = (stage == Stage::Review)
                        .then(|| row.reply_body.as_deref().and_then(verdict))
                        .flatten();
                    Some(Outcome::Replied(verdict))
                }
                (Some("pending" | "sending"), false) => None,
                _ => Some(Outcome::Unknown),
            },
            Some("failed" | "rejected") => Some(Outcome::Failed),
            _ => Some(Outcome::Unknown),
        },
        _ => Some(Outcome::Unknown),
    }
}

/// The Reviewer's verdict: the last line of the form `VERDICT: approve|reject|blocked` (or
/// `结论：通过|驳回|阻塞`). The Reviewer prompt asks for exactly one such line.
pub fn verdict(body: &str) -> Option<Verdict> {
    body.lines().rev().find_map(|line| {
        let line = line.trim().trim_matches(|c| c == '*' || c == '`').trim();
        let (key, value) = line.split_once(':').or_else(|| line.split_once('：'))?;
        if !matches!(key.trim().to_ascii_lowercase().as_str(), "verdict" | "结论") {
            return None;
        }
        match value.trim().to_ascii_lowercase().as_str() {
            "approve" | "approved" | "通过" => Some(Verdict::Approve),
            "reject" | "rejected" | "驳回" => Some(Verdict::Reject),
            "blocked" | "阻塞" => Some(Verdict::Blocked),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(state: State, inputs: &[Input]) -> (State, Vec<Action>) {
        let mut all = Vec::new();
        let mut state = state;
        for input in inputs {
            let (next_state, actions) = next(&state, *input);
            state = next_state;
            all.extend(actions);
        }
        (state, all)
    }

    fn current(state: &State) -> Slot {
        match state {
            State::Running { slot, .. } | State::ManualIntervention { slot, .. } => *slot,
            other => panic!("no current slot in {other:?}"),
        }
    }

    fn ok(state: &State, verdict: Option<Verdict>) -> Input {
        Input::Result {
            slot: current(state),
            outcome: Outcome::Replied(verdict),
        }
    }

    /// Drives a run to the first review and returns it.
    fn to_review() -> State {
        let mut s = next(&State::NotStarted, Input::Start).0;
        s = next(&s, ok(&s, None)).0;
        s = next(&s, Input::SpecDecision(SpecDecision::Approve)).0;
        for _ in 0..3 {
            s = next(&s, ok(&s, None)).0; // plan, execute, test
        }
        assert_eq!(current(&s).stage, Stage::Review);
        s
    }

    fn dispatched(actions: &[Action]) -> Vec<(Stage, u32, u32)> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Dispatch { slot, .. } => Some((slot.stage, slot.round, slot.attempt)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_happy_path_dispatches_each_stage_once_and_records_the_key_events() {
        let mut s = next(&State::NotStarted, Input::Start).0;
        let mut actions = Vec::new();
        let step = |s: &State, input: Input, actions: &mut Vec<Action>| {
            let (n, a) = next(s, input);
            actions.extend(a);
            n
        };
        s = step(&s, ok(&s, None), &mut actions);
        assert_eq!(s, State::AwaitingSpecApproval { spec_revision: 1 });
        s = step(&s, Input::SpecDecision(SpecDecision::Approve), &mut actions);
        for verdict in [None, None, None, Some(Verdict::Approve), None] {
            s = step(&s, ok(&s, verdict), &mut actions);
        }
        assert_eq!(s, State::Completed);
        assert_eq!(
            dispatched(&actions),
            vec![
                (Stage::Plan, 0, 1),
                (Stage::Execute, 1, 1),
                (Stage::Test, 1, 1),
                (Stage::Review, 1, 1),
                (Stage::Release, 1, 1)
            ]
        );
        for key in [
            ("execute", "succeeded"),
            ("review", "succeeded"),
            ("release", "succeeded"),
        ] {
            assert!(actions.iter().any(|a| matches!(a, Action::Record { phase, status, .. } if (*phase, *status) == key)), "{key:?}");
        }
        assert_eq!(
            actions.last(),
            Some(&Action::Finish {
                status: "succeeded"
            })
        );
    }

    #[test]
    fn nothing_executes_before_the_spec_is_confirmed() {
        let s = next(&State::NotStarted, Input::Start).0;
        let (s, actions) = next(&s, ok(&s, None));
        assert!(dispatched(&actions).is_empty());
        // Results and timeouts for other dispatches do nothing while waiting for the Human.
        let stray = Input::Result {
            slot: first(Stage::Execute, 1),
            outcome: Outcome::Replied(None),
        };
        assert_eq!(next(&s, stray), (s.clone(), vec![]));
        let (revised, actions) = next(&s, Input::SpecDecision(SpecDecision::Revise));
        assert_eq!(
            revised,
            State::Running {
                slot: first(Stage::Spec, 0),
                spec_revision: 2
            }
        );
        assert_eq!(dispatched(&actions), vec![(Stage::Spec, 0, 1)]);
        let (rejected, _) = next(&s, Input::SpecDecision(SpecDecision::Reject));
        assert_eq!(
            rejected,
            State::Failed {
                reason: "spec_rejected"
            }
        );
    }

    #[test]
    fn two_reworks_then_manual_intervention() {
        let mut s = to_review();
        let mut rounds = vec![];
        for _ in 0..MAX_REWORKS {
            let (n, actions) = next(&s, ok(&s, Some(Verdict::Reject)));
            rounds.extend(dispatched(&actions));
            s = feed(n, &[]).0;
            s = next(&s, ok(&s, None)).0; // execute
            s = next(&s, ok(&s, None)).0; // test
        }
        assert_eq!(rounds, vec![(Stage::Execute, 2, 1), (Stage::Execute, 3, 1)]);
        let (s, actions) = next(&s, ok(&s, Some(Verdict::Reject)));
        assert!(matches!(
            s,
            State::ManualIntervention {
                reason: "rework_limit",
                slot: Slot { round: 3, .. },
                ..
            }
        ));
        assert!(dispatched(&actions).is_empty());
    }

    #[test]
    fn unknown_and_timeout_are_never_redispatched_automatically() {
        let s = to_review();
        let (unknown, actions) = next(
            &s,
            Input::Result {
                slot: current(&s),
                outcome: Outcome::Unknown,
            },
        );
        assert!(matches!(
            unknown,
            State::ManualIntervention {
                reason: "unknown_outcome",
                ..
            }
        ));
        assert!(dispatched(&actions).is_empty());
        let (timeout, actions) = next(&s, Input::Timeout { slot: current(&s) });
        assert!(matches!(
            timeout,
            State::ManualIntervention {
                reason: "stage_timeout",
                ..
            }
        ));
        assert!(dispatched(&actions).is_empty());

        // Only a Human decision moves on, as a new dispatch.
        let (cont, actions) = next(&unknown, Input::Human(HumanDecision::Continue));
        assert_eq!(dispatched(&actions), vec![(Stage::Review, 1, 2)]);
        assert!(matches!(cont, State::Running { .. }));
        let (_, actions) = next(&unknown, Input::Human(HumanDecision::Reassign));
        assert!(matches!(
            actions.last(),
            Some(Action::Dispatch {
                reassigned: true,
                ..
            })
        ));
        assert_eq!(
            next(&unknown, Input::Human(HumanDecision::Abort)).0,
            State::Failed { reason: "aborted" }
        );
    }

    #[test]
    fn a_failed_or_expired_stage_is_retried_once() {
        let s = to_review();
        let (retry, actions) = next(
            &s,
            Input::Result {
                slot: current(&s),
                outcome: Outcome::Expired,
            },
        );
        assert_eq!(dispatched(&actions), vec![(Stage::Review, 1, 2)]);
        let (failed, actions) = next(
            &retry,
            Input::Result {
                slot: current(&retry),
                outcome: Outcome::Failed,
            },
        );
        assert_eq!(
            failed,
            State::Failed {
                reason: "stage_failed"
            }
        );
        assert_eq!(actions.last(), Some(&Action::Finish { status: "failed" }));
    }

    #[test]
    fn results_for_other_dispatches_are_ignored() {
        let s = to_review();
        let old = Slot {
            attempt: 7,
            ..current(&s)
        };
        assert_eq!(
            next(
                &s,
                Input::Result {
                    slot: old,
                    outcome: Outcome::Replied(Some(Verdict::Approve))
                }
            ),
            (s.clone(), vec![])
        );
        assert_eq!(
            next(
                &s,
                Input::Timeout {
                    slot: first(Stage::Plan, 0)
                }
            ),
            (s.clone(), vec![])
        );
        assert_eq!(
            next(&State::Completed, Input::Start),
            (State::Completed, vec![])
        );
    }

    #[test]
    fn a_review_without_a_verdict_is_not_approval() {
        let s = to_review();
        let (s, actions) = next(&s, ok(&s, None));
        assert!(matches!(
            s,
            State::ManualIntervention {
                reason: "review_without_verdict",
                ..
            }
        ));
        assert!(dispatched(&actions).is_empty());
    }

    #[test]
    fn stages_route_to_fixed_roles() {
        assert_eq!(Stage::Spec.role(), Role::Coordinator);
        assert_eq!(Stage::Execute.role(), Role::Implementer);
        assert_eq!(Stage::Test.role(), Role::Implementer);
        assert_eq!(Stage::Review.role(), Role::Reviewer);
    }

    fn row(state: &str, outcome: Option<&str>, reply: Option<&str>, body: &str) -> DispatchRow {
        DispatchRow {
            state: state.into(),
            outcome: outcome.map(Into::into),
            reply_state: reply.map(Into::into),
            reply_blocked: false,
            reply_body: Some(body.into()),
        }
    }

    #[test]
    fn dispatch_rows_map_to_outcomes() {
        for state in ["pending", "offered", "acknowledged", "running"] {
            assert_eq!(
                observed(&row(state, None, None, ""), Stage::Plan),
                None,
                "{state}"
            );
        }
        assert_eq!(
            observed(&row("unknown", None, None, ""), Stage::Plan),
            Some(Outcome::Unknown)
        );
        assert_eq!(
            observed(&row("cancelled", None, None, ""), Stage::Plan),
            Some(Outcome::Expired)
        );
        assert_eq!(
            observed(&row("completed", Some("failed"), None, ""), Stage::Plan),
            Some(Outcome::Failed)
        );
        assert_eq!(
            observed(&row("completed", Some("rejected"), None, ""), Stage::Plan),
            Some(Outcome::Failed)
        );
        assert_eq!(
            observed(
                &row("completed", Some("replied"), Some("pending"), ""),
                Stage::Plan
            ),
            None
        );
        assert_eq!(
            observed(
                &row("completed", Some("replied"), Some("unknown"), ""),
                Stage::Plan
            ),
            Some(Outcome::Unknown)
        );
        assert_eq!(
            observed(
                &row("completed", Some("replied"), Some("cancelled"), ""),
                Stage::Plan
            ),
            Some(Outcome::Unknown)
        );
        let mut blocked = row("completed", Some("replied"), Some("sent"), "");
        blocked.reply_blocked = true;
        assert_eq!(observed(&blocked, Stage::Plan), Some(Outcome::Unknown));
        assert_eq!(
            observed(
                &row("completed", Some("replied"), Some("sent"), "done"),
                Stage::Plan
            ),
            Some(Outcome::Replied(None))
        );
        assert_eq!(
            observed(
                &row("completed", Some("something_new"), None, ""),
                Stage::Plan
            ),
            Some(Outcome::Unknown)
        );
    }

    #[test]
    fn only_review_replies_carry_a_verdict() {
        let r = row(
            "completed",
            Some("replied"),
            Some("sent"),
            "Findings...\nVERDICT: reject",
        );
        assert_eq!(
            observed(&r, Stage::Review),
            Some(Outcome::Replied(Some(Verdict::Reject)))
        );
        assert_eq!(observed(&r, Stage::Execute), Some(Outcome::Replied(None)));
    }

    #[test]
    fn verdict_lines_are_read_strictly() {
        assert_eq!(verdict("ok\nVERDICT: approve"), Some(Verdict::Approve));
        assert_eq!(verdict("**Verdict: Rejected**"), Some(Verdict::Reject));
        assert_eq!(verdict("结论：阻塞"), Some(Verdict::Blocked));
        // The last verdict line wins; prose that merely mentions approval does not count.
        assert_eq!(
            verdict("VERDICT: reject\nfixed\nVERDICT: approve"),
            Some(Verdict::Approve)
        );
        assert_eq!(verdict("I would approve this"), None);
        assert_eq!(verdict("VERDICT: maybe"), None);
    }

    #[test]
    fn stored_states_read_back_exactly() {
        let slot = Slot {
            stage: Stage::Review,
            round: 2,
            attempt: 1,
        };
        let states = [
            State::NotStarted,
            State::Running {
                slot,
                spec_revision: 3,
            },
            State::AwaitingSpecApproval { spec_revision: 1 },
            State::ManualIntervention {
                slot,
                spec_revision: 1,
                reason: "rework_limit",
            },
            State::Completed,
            State::Failed {
                reason: "spec_rejected",
            },
        ];
        for state in states {
            assert_eq!(
                State::from_json(&state.to_json()),
                Some(state.clone()),
                "{state:?}"
            );
        }
        // Every reason the flow can produce is storable.
        for reason in REASONS {
            assert!(
                State::from_json(&State::Failed { reason }.to_json()).is_some(),
                "{reason}"
            );
        }
        assert_eq!(
            State::from_json(&serde_json::json!({"kind": "failed", "reason": "made_up"})),
            None
        );
        assert_eq!(
            State::from_json(
                &serde_json::json!({"kind": "running", "slot": {"stage": "deploy", "round": 0, "attempt": 1}, "specRevision": 1})
            ),
            None
        );
        assert_eq!(
            State::from_json(
                &serde_json::json!({"kind": "running", "slot": {"stage": "plan", "round": 0, "attempt": 0}, "specRevision": 1})
            ),
            None
        );
    }
}
