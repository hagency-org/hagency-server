//! Versioned shared-resource management wire types. Authority is host supplied.
use crate::{Error, Result, key};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PROTOCOL: &str = "hagency-management-v1";
pub const MAX_BYTES: usize = 16 * 1024;
pub const MAX_PAGE: i64 = 32;

/// Constructed by the server configuration loader, never from an API body.
#[derive(Clone)]
pub struct ManagementPolicy {
    administrator: String,
    project_reservations: bool,
    parent_settlement: bool,
    project_setup: bool,
    agent_admission: bool,
    agent_executor: bool,
    agent_matrix_provisioning: bool,
    agent_runtime_provisioning: bool,
    shared_execution: bool,
    shared_successors: bool,
    shared_durable_intake: bool,
    shared_demands: bool,
    shared_thread_context: bool,
    agent_budget_deltas: bool,
    shared_budget_execution: bool,
}
impl ManagementPolicy {
    pub fn new(administrator: String, server: &str) -> Result<Self> {
        local_user(&administrator, server)?;
        Ok(Self {
            administrator,
            project_reservations: false,
            parent_settlement: false,
            project_setup: false,
            agent_admission: false,
            agent_executor: false,
            agent_matrix_provisioning: false,
            agent_runtime_provisioning: false,
            shared_execution: false,
            shared_successors: false,
            shared_durable_intake: false,
            shared_demands: false,
            shared_thread_context: false,
            agent_budget_deltas: false,
            shared_budget_execution: false,
        })
    }
    pub fn with_shared_budget_execution(mut self, enabled: bool) -> Self {
        self.shared_budget_execution = enabled;
        self
    }
    pub fn shared_budget_execution(&self) -> bool {
        self.shared_budget_execution
    }
    pub fn with_agent_budget_deltas(mut self, enabled: bool) -> Self {
        self.agent_budget_deltas = enabled;
        self
    }
    pub fn agent_budget_deltas(&self) -> bool {
        self.agent_budget_deltas
    }
    pub fn with_shared_thread_context(mut self, enabled: bool) -> Self {
        self.shared_thread_context = enabled;
        self
    }
    pub fn shared_thread_context(&self) -> bool {
        self.shared_thread_context
    }
    pub fn with_shared_demands(mut self, enabled: bool) -> Self {
        self.shared_demands = enabled;
        self
    }
    pub fn shared_demands(&self) -> bool {
        self.shared_demands
    }
    pub fn with_shared_durable_intake(mut self, enabled: bool) -> Self {
        self.shared_durable_intake = enabled;
        self
    }
    pub fn shared_durable_intake(&self) -> bool {
        self.shared_durable_intake
    }
    pub fn with_shared_successors(mut self, enabled: bool) -> Self {
        self.shared_successors = enabled;
        self
    }
    pub fn shared_successors(&self) -> bool {
        self.shared_successors
    }
    pub fn with_shared_execution(mut self, enabled: bool) -> Self {
        self.shared_execution = enabled;
        self
    }
    pub fn shared_execution(&self) -> bool {
        self.shared_execution
    }
    pub fn with_agent_runtime_provisioning(mut self, enabled: bool) -> Self {
        self.agent_runtime_provisioning = enabled;
        self
    }
    pub fn agent_runtime_provisioning(&self) -> bool {
        self.agent_runtime_provisioning
    }
    pub fn with_agent_executor(mut self, enabled: bool) -> Self {
        self.agent_executor = enabled;
        self
    }
    pub fn with_agent_matrix_provisioning(mut self, enabled: bool) -> Self {
        self.agent_matrix_provisioning = enabled;
        self
    }
    pub fn agent_matrix_provisioning(&self) -> bool {
        self.agent_matrix_provisioning
    }
    pub fn agent_executor(&self) -> bool {
        self.agent_executor
    }
    pub fn administrator(&self) -> &str {
        &self.administrator
    }
    pub fn with_agent_admission(mut self, enabled: bool) -> Self {
        self.agent_admission = enabled;
        self
    }
    pub fn agent_admission(&self) -> bool {
        self.agent_admission
    }
    pub fn with_project_reservations(mut self, enabled: bool) -> Self {
        self.project_reservations = enabled;
        self
    }
    pub fn project_reservations(&self) -> bool {
        self.project_reservations
    }
    pub fn with_parent_settlement(mut self, enabled: bool) -> Self {
        self.parent_settlement = enabled;
        self
    }
    pub fn with_project_setup(mut self, enabled: bool) -> Self {
        self.project_setup = enabled;
        self
    }
    pub fn project_setup(&self) -> bool {
        self.project_setup
    }
    pub fn parent_settlement(&self) -> bool {
        self.parent_settlement
    }
}
pub(crate) fn local_user(value: &str, server: &str) -> Result<()> {
    if value.len() > 255
        || !value.starts_with('@')
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
        || !value[1..]
            .split_once(':')
            .is_some_and(|(local, host)| !local.is_empty() && host == server)
    {
        return Err(Error::Invalid("invalid_management_user"));
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Codex,
    Claude,
    Octos,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContributionInput {
    pub command_id: String,
    pub resource_id: String,
    pub resource_revision: String,
    pub label: String,
    pub runtime: Runtime,
    pub capabilities: BTreeSet<String>,
    pub coordinator_mxid: String,
}
impl ContributionInput {
    pub(crate) fn validate(&self, server: &str) -> Result<()> {
        key(&self.command_id)?;
        key(&self.resource_id)?;
        if self.resource_revision.len() != 64
            || !self
                .resource_revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.label.trim().is_empty()
            || self.label.len() > 160
            || self.label.chars().any(char::is_control)
            || self.capabilities.is_empty()
            || self.capabilities.len() > 16
        {
            return Err(Error::Invalid("invalid_resource_offer"));
        }
        for value in &self.capabilities {
            if value.is_empty()
                || value.len() > 80
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-/".contains(&b))
            {
                return Err(Error::Invalid("invalid_resource_capability"));
            }
        }
        local_user(&self.coordinator_mxid, server)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Reject,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionInput {
    pub command_id: String,
    pub expected_revision: i64,
    pub decision: Decision,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeInput {
    pub command_id: String,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoverCommandInput {
    pub operation: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifyConnectionInput {
    pub command_id: String,
    pub expected_revision: i64,
    pub resource_id: String,
    pub resource_revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionVerification {
    pub status: String,
    pub verified_at_ms: i64,
    pub fresh_until_ms: i64,
}
#[derive(Default, Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagementQuery {
    pub after: Option<String>,
    #[serde(default)]
    pub needs_my_action: bool,
    /// Older clients understand only resource admissions.
    #[serde(default)]
    pub include_projects: bool,
    #[serde(default)]
    pub include_allocations: bool,
}
impl ManagementQuery {
    pub(crate) fn validate(&self) -> Result<()> {
        if let Some(after) = &self.after {
            key(after)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contribution {
    pub id: String,
    pub owner_mxid: String,
    pub executor_device_id: String,
    pub executor_generation: i64,
    pub resource_id: String,
    pub resource_revision: String,
    pub label: String,
    pub runtime: Runtime,
    pub capabilities: BTreeSet<String>,
    pub coordinator_mxid: String,
    pub state: String,
    pub revision: i64,
    pub request_id: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub verification: Option<ConnectionVerification>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagementRequest {
    pub id: String,
    pub kind: String,
    pub contribution_id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub allocation_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub resource_label: Option<String>,
    pub requester_mxid: String,
    pub approver_mxid: String,
    pub state: String,
    pub revision: i64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub decided_at_ms: Option<i64>,
    pub can_decide: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Admission {
    pub contribution: Contribution,
    pub request: ManagementRequest,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagementPage<T> {
    pub protocol: &'static str,
    pub observed_at_ms: i64,
    pub items: Vec<T>,
    pub next_after: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectRequestInput {
    pub command_id: String,
    pub contribution_id: String,
    pub name: String,
    pub purpose: String,
    /// Requested tokens per calendar month. This expresses intent, not an
    /// allocation or a billing period already started; the later executor
    /// allocation must establish its timezone, start, and renewal boundary.
    pub requested_tokens: i64,
}
impl ProjectRequestInput {
    pub(crate) fn validate(&self) -> Result<()> {
        key(&self.command_id)?;
        key(&self.contribution_id)?;
        if self.name.trim().is_empty()
            || self.name.len() > 160
            || self.name.chars().any(char::is_control)
            || self.purpose.trim().is_empty()
            || self.purpose.len() > 2000
            || self.purpose.chars().any(|c| c.is_control() && c != '\n')
            || !(1..=9_007_199_254_740_991).contains(&self.requested_tokens)
        {
            return Err(Error::Invalid("invalid_project_request"));
        }
        Ok(())
    }
}
/// Approval is a project plan. A separate executor-confirmed allocation and
/// Matrix setup are required before this can become an executable project.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPlan {
    pub id: String,
    pub manager_mxid: String,
    pub contribution_id: String,
    pub resource_label: String,
    pub coordinator_mxid: String,
    pub name: String,
    pub purpose: String,
    /// Monthly token request, with the same semantics as ProjectRequestInput.
    pub requested_tokens: i64,
    pub state: String,
    pub revision: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub allocation_status: String,
    pub can_revoke: bool,
    pub request: ManagementRequest,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogQuery {
    pub after: Option<String>,
    pub capability: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogResource {
    pub id: String,
    pub label: String,
    pub runtime: Runtime,
    pub capabilities: BTreeSet<String>,
    pub coordinator_mxid: String,
    pub verification: Option<ConnectionVerification>,
}
