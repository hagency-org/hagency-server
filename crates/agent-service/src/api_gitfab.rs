//! GitFab workflow runs for the Humans of a project:
//!
//! - `POST /hagency/api/v1/projects/{project}/gitfab/runs` starts a run in a task thread;
//! - `GET  .../gitfab/runs/{run}` reads it;
//! - `POST .../gitfab/runs/{run}/spec` confirms, rejects or asks to revise the Spec;
//! - `POST .../gitfab/runs/{run}/decision` continues, reassigns or aborts after Manual Intervention.
//!
//! Any joined member of the project's Space may start a run; only the Human who started it decides on it
//! (Annex 2 §10: the designated confirmer). Every Agent binding must belong to the run's Room.

use salvo::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::orchestrator::{MatrixStageRoom, Orchestrator, Recorder};
use crate::orchestrator_flow::{HumanDecision, Input, SpecDecision, State};
use crate::transport::{ORCHESTRATOR_ROLES, OrchestratorRun, OrchestratorRunInit, TransportStore};
use crate::{
    Error, Result, domain::DomainStore, gateway::Gateway, matrix_client::MatrixClient, store::Store,
};

pub(crate) struct Gitfab<'a> {
    pub store: &'a Store,
    pub domain: &'a DomainStore,
    pub gateway: &'a Gateway,
    pub transport: &'a TransportStore,
    pub matrix: &'a MatrixClient,
    pub recorder: &'a Recorder,
    pub stage_timeout_ms: i64,
    pub shared: Option<(
        &'a DomainStore,
        &'a crate::management_types::ManagementPolicy,
    )>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Start {
    task_id: String,
    room_id: String,
    thread_root: String,
    bindings: Value,
    context: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    decision: String,
}

async fn body<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T> {
    req.set_secure_max_size(65536);
    let bytes = req
        .payload()
        .await
        .map_err(|_| Error::Invalid("invalid_gitfab_payload"))?;
    serde_json::from_slice(bytes).map_err(|_| Error::Invalid("invalid_gitfab_payload"))
}

pub(crate) fn matches(path: &str) -> bool {
    let p = path.split('/').collect::<Vec<_>>();
    p.len() >= 8 && p[4] == "projects" && p[6] == "gitfab" && p[7] == "runs"
}

fn view(run: &OrchestratorRun) -> Value {
    json!({
        "id": run.id,
        "projectId": run.init.project_id,
        "taskId": run.init.task_id,
        "roomId": run.init.room_id,
        "threadRoot": run.init.thread_root,
        "requester": run.init.requester_mxid,
        "bindings": run.init.bindings,
        "context": run.init.context,
        "state": run.state.to_json(),
        "dispatchId": run.dispatch_id,
        "revision": run.revision,
        "updatedAtMs": run.updated_at_ms,
    })
}

impl Gitfab<'_> {
    pub(crate) async fn call(&self, req: &mut Request, credential: &str) -> Result<Option<Value>> {
        let path = req.uri().path().to_owned();
        if !matches(&path) {
            return Ok(None);
        }
        let parts = path.split('/').collect::<Vec<_>>();
        let project_id = parts[5];
        crate::key(project_id)?;
        let now = crate::api::now_ms();
        let principal = self.store.authenticate(credential, now, false).await?;
        let project = self
            .domain
            .project(&principal, project_id, now)
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::Unauthorized("project_membership_required"),
                other => other,
            })?;
        let state = self.gateway.state(&project.space_id).await?;
        let joined = state.iter().any(|e| {
            e["type"] == "m.room.member"
                && e["content"]["membership"] == "join"
                && e["state_key"] == principal.mxid.as_str()
        });
        if !project.active || !joined {
            return Err(Error::Unauthorized("project_membership_required"));
        }
        let room = MatrixStageRoom {
            gateway: self.gateway,
            matrix: self.matrix,
            shared: self.shared,
            // Human decisions only start or resume stages; a stage's success reaches the worker,
            // which holds the Git hosts.
            git: None,
        };
        let orchestrator = Orchestrator {
            transport: self.transport,
            room: &room,
            recorder: self.recorder,
            stage_timeout_ms: self.stage_timeout_ms,
        };
        let method = req.method().clone();
        let value = match (method, &parts[8..]) {
            (salvo::http::Method::POST, []) => {
                let start: Start = body(req).await?;
                for role in ORCHESTRATOR_ROLES {
                    let binding = start.bindings[*role]
                        .as_str()
                        .ok_or(Error::Invalid("invalid_orchestrator_run"))?;
                    crate::key(binding)?;
                    if binding.starts_with("mas_") {
                        let pool = self.transport.shared_room_agents(&start.room_id).await?;
                        if !pool.iter().any(|(a, ..)| a == binding) {
                            return Err(Error::Unauthorized("binding_outside_run_room"));
                        }
                        continue;
                    }
                    let scope = self.transport.binding_scope(binding).await?;
                    if scope.room_id != start.room_id || scope.space_id != project.space_id {
                        return Err(Error::Unauthorized("binding_outside_run_room"));
                    }
                }
                let init = OrchestratorRunInit {
                    project_id: project.id.clone(),
                    task_id: start.task_id,
                    room_id: start.room_id,
                    thread_root: start.thread_root,
                    requester_mxid: principal.mxid.clone(),
                    bindings: start.bindings,
                    context: start.context,
                };
                let run = self.transport.create_orchestrator_run(&init, now).await?;
                view(&run)
            }
            (salvo::http::Method::GET, [run]) => view(&self.run(&project.id, run).await?),
            (salvo::http::Method::POST, [run, kind @ ("spec" | "decision")]) => {
                let current = self.run(&project.id, run).await?;
                if current.init.requester_mxid != principal.mxid {
                    return Err(Error::Unauthorized("run_requester_required"));
                }
                let Decision { decision } = body(req).await?;
                let input = match (*kind, decision.as_str()) {
                    ("spec", "approve") => Input::SpecDecision(SpecDecision::Approve),
                    ("spec", "reject") => Input::SpecDecision(SpecDecision::Reject),
                    ("spec", "revise") => Input::SpecDecision(SpecDecision::Revise),
                    ("decision", "continue") => Input::Human(HumanDecision::Continue),
                    ("decision", "reassign") => Input::Human(HumanDecision::Reassign),
                    ("decision", "abort") => Input::Human(HumanDecision::Abort),
                    _ => return Err(Error::Invalid("invalid_gitfab_decision")),
                };
                let state: State = orchestrator.decide(&current.id, input, now).await?;
                json!({"runId": current.id, "state": state.to_json()})
            }
            _ => return Err(Error::NotFound("not_found")),
        };
        Ok(Some(value))
    }

    async fn run(&self, project_id: &str, id: &str) -> Result<OrchestratorRun> {
        match self.transport.orchestrator_run(id).await? {
            Some(run) if run.init.project_id == project_id => Ok(run),
            _ => Err(Error::NotFound("orchestrator_run_not_found")),
        }
    }
}
