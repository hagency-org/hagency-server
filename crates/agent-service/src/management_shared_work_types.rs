//! Closed shared message transport; result recovery never grants another turn.
use crate::management_shared_execution_types::*;
use serde::{Deserialize, Serialize};
/// Read/history protocol only; individual live authority gates all effects.
pub const SHARED_WORK_HISTORY_FEATURE: &str = "shared-work-history-v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkPollInput {
    pub authority: SharedActivityRef,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkRecoverInput {
    pub runtime_command_digest: String,
    pub work_digest: String,
    pub observation_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkResultTicket {
    pub work_id: String,
    pub work_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub recorded: Option<SharedWorkReceipt>,
    pub state: String,
    pub reply_state: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkResultUpload {
    pub work_digest: String,
    pub token: String,
    pub receipt: SharedWorkReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkAcknowledgement {
    pub observation_id: String,
    pub receipt_digest: String,
    pub state: String,
    pub reply_state: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkQuery {
    pub after: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedObservedUsage {
    pub known_consumed_tokens: i64,
    pub retained_tokens: Option<i64>,
    pub complete: bool,
    pub period_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkHistoryItem {
    pub id: String,
    pub execution_id: String,
    pub event_id: String,
    pub requester_mxid: String,
    pub state: String,
    pub reply_state: Option<String>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub usage: Option<SharedObservedUsage>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkPage {
    pub items: Vec<SharedWorkHistoryItem>,
    pub next_after: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkIndexQuery {
    pub project_id: Option<String>,
    pub after: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkIndexItem {
    pub request_id: String,
    pub project_id: String,
    pub project_name: String,
    pub agent_name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkIndexPage {
    pub items: Vec<SharedWorkIndexItem>,
    pub next_after: Option<String>,
}
