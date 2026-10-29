//! Recorded cleanup observations, never proof of refunded or runnable capacity.
use serde::{Deserialize, Serialize};
pub const AGENT_CLEANUP_FEATURE: &str = "agent-cleanup-v1";
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRemovalQuery {
    pub project_id: Option<String>,
    pub after: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRemovalIndexItem {
    pub request_id: String,
    pub project_id: String,
    pub project_name: String,
    pub agent_name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRemovalPage {
    pub items: Vec<AgentRemovalIndexItem>,
    pub next_after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRemovalProgress {
    pub request_id: String,
    pub project_id: String,
    pub project_name: String,
    pub proposed_agent_id: String,
    pub agent_name: String,
    pub request_revision: i64,
    pub admission_state: String,
    pub can_cancel: bool,
    pub can_revoke: bool,
    pub central_fenced: bool,
    pub native_fence: Option<AgentFenceProgress>,
    pub process_observation: Option<AgentProcessObservation>,
    pub matrix: Option<AgentMatrixCleanup>,
    pub usage: AgentCleanupUsage,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFenceProgress {
    pub delivery_state: String,
    pub native_state: Option<String>,
    pub ledger_revision: Option<i64>,
    pub process_state: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentProcessObservation {
    pub kind: String,
    pub state: String,
    pub ledger_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMatrixCleanup {
    pub state: String,
    pub cleanup_observed_at_ms: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCleanupUsage {
    pub period_key: String,
    pub known_consumed_tokens: Option<i64>,
    pub retained_tokens_floor: Option<i64>,
    pub fence_usage_complete: Option<bool>,
    pub requires_reconciliation: bool,
}
