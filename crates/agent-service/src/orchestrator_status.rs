//! Agent status for the GitFab board (Annex 1 §1.2.1(7): Active, Running, Pending), derived from the open
//! orchestrator runs. Served read-only at `GET /hagency/gitfab/v1/agents` behind the board's own token;
//! it reports binding ids, roles, stages and run states, never credentials.

use std::collections::BTreeMap;
use std::sync::Arc;

use salvo::http::StatusCode;
use salvo::prelude::*;
use serde_json::{Value, json};

use crate::orchestrator_flow::{Role, State};
use crate::transport::{OrchestratorRun, TransportStore};

const ROLES: [(Role, &str); 3] = [
    (Role::Coordinator, "coordinator"),
    (Role::Implementer, "implementer"),
    (Role::Reviewer, "reviewer"),
];

/// How busy a role's Agent is in one run. Order matters: the busiest state across runs wins.
fn role_state(state: &State, role: Role) -> (&'static str, Option<&'static str>) {
    match state {
        State::Running { slot, .. } if slot.stage.role() == role => {
            ("running", Some(slot.stage.phase()))
        }
        // A Human holds the run: the Agent whose stage it is waits.
        State::ManualIntervention { slot, .. } if slot.stage.role() == role => {
            ("pending", Some(slot.stage.phase()))
        }
        State::AwaitingSpecApproval { .. } if role == Role::Coordinator => {
            ("pending", Some("spec"))
        }
        State::NotStarted => ("pending", None),
        _ => ("active", None),
    }
}

fn rank(status: &str) -> u8 {
    match status {
        "running" => 2,
        "pending" => 1,
        _ => 0,
    }
}

/// Every Agent bound in an open run with its busiest state and what it does in each run.
pub(crate) fn agents(runs: &[OrchestratorRun]) -> Value {
    let mut by_binding: BTreeMap<String, Value> = BTreeMap::new();
    for run in runs {
        for (role, key) in ROLES {
            let Some(binding) = run.init.bindings[key].as_str() else {
                continue;
            };
            let (status, stage) = role_state(&run.state, role);
            let entry = by_binding.entry(binding.to_owned()).or_insert_with(|| {
                json!({"binding": binding, "pool": binding.starts_with("mas_"), "status": "active", "roles": [], "runs": []})
            });
            if rank(status) > rank(entry["status"].as_str().unwrap_or("")) {
                entry["status"] = json!(status);
            }
            let roles = entry["roles"].as_array_mut().expect("array");
            if !roles.contains(&json!(key)) {
                roles.push(json!(key));
            }
            entry["runs"].as_array_mut().expect("array").push(json!({
                "run": run.id, "task": run.init.task_id,
                "project": run.init.context["projectId"].as_str().unwrap_or(&run.init.project_id),
                "role": key, "status": status, "stage": stage, "run_state": run.state.to_json()["kind"],
            }));
        }
    }
    json!({"agents": by_binding.into_values().collect::<Vec<_>>(), "open_runs": runs.len()})
}

/// `GET /hagency/gitfab/v1/agents` with `Authorization: Bearer <board token>`.
#[derive(Clone)]
pub struct BoardStatus {
    pub transport: TransportStore,
    pub token: Arc<String>,
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[handler]
impl BoardStatus {
    async fn handle(&self, req: &mut Request, res: &mut Response) {
        res.add_header("cache-control", "no-store", true).ok();
        let presented = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if !same(presented.as_bytes(), self.token.as_bytes()) {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(json!({"code": "board_token_required"})));
            return;
        }
        match self.transport.active_orchestrator_runs(200).await {
            Ok(runs) => res.render(Json(agents(&runs))),
            Err(_) => {
                res.status_code(StatusCode::SERVICE_UNAVAILABLE);
                res.render(Json(json!({"code": "orchestrator_state_unavailable"})));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator_flow::{Slot, Stage};
    use crate::transport::OrchestratorRunInit;

    fn run(id: &str, state: State, reviewer: &str) -> OrchestratorRun {
        OrchestratorRun {
            id: id.into(),
            init: OrchestratorRunInit {
                project_id: "prj".into(),
                task_id: format!("task-{id}"),
                room_id: "!r:s".into(),
                thread_root: "$t".into(),
                requester_mxid: "@h:s".into(),
                bindings: json!({"coordinator": "bnd_coord", "implementer": "mas_impl", "reviewer": reviewer}),
                context: json!({"projectId": "DEMO-02"}),
            },
            state,
            revision: 1,
            dispatch_id: None,
            handoff: None,
            pending_events: json!([]),
            updated_at_ms: 0,
        }
    }

    #[test]
    fn the_busiest_state_across_runs_wins_and_humans_hold_as_pending() {
        let execute = Slot {
            stage: Stage::Execute,
            round: 1,
            attempt: 1,
        };
        let review = Slot {
            stage: Stage::Review,
            round: 1,
            attempt: 1,
        };
        let runs = [
            run(
                "a",
                State::Running {
                    slot: execute,
                    spec_revision: 1,
                },
                "bnd_rev",
            ),
            run(
                "b",
                State::AwaitingSpecApproval { spec_revision: 1 },
                "bnd_rev",
            ),
            run(
                "c",
                State::ManualIntervention {
                    slot: review,
                    spec_revision: 1,
                    reason: "stage_timeout",
                },
                "bnd_rev2",
            ),
        ];
        let out = agents(&runs);
        let get = |b: &str| {
            out["agents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["binding"] == b)
                .cloned()
                .unwrap()
        };
        assert_eq!(get("mas_impl")["status"], "running");
        assert_eq!(get("mas_impl")["pool"], true);
        assert_eq!(get("mas_impl")["runs"][0]["stage"], "execute");
        assert_eq!(
            get("bnd_coord")["status"],
            "pending",
            "the Spec waits for a Human in run b"
        );
        assert_eq!(get("bnd_rev")["status"], "active");
        assert_eq!(
            get("bnd_rev2")["status"],
            "pending",
            "manual intervention on its review"
        );
        assert_eq!(out["open_runs"], 3);
    }

    #[test]
    fn only_the_board_token_is_accepted() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd") && !same(b"abc", b"ab") && !same(b"", b"abc"));
    }
}
