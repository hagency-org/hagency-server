//! Descriptive run intent and retained event intake; never model authority.
use crate::{Error, Result, key};
use serde::{Deserialize, Serialize};
pub const SHARED_RUN_CONTROL_FEATURE: &str = "shared-run-control-v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedControlInput {
    pub command_id: String,
    pub expected_revision: i64,
    pub mode: String,
}
impl SharedControlInput {
    pub fn validate(&self) -> Result<()> {
        key(&self.command_id)?;
        if !(0..9_007_199_254_740_991).contains(&self.expected_revision)
            || !matches!(self.mode.as_str(), "running" | "paused")
        {
            return Err(Error::Invalid("invalid_shared_run_control"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedRunControl {
    pub assignment_id: String,
    pub revision: i64,
    pub mode: String,
    pub running_since_ms: Option<i64>,
    pub updated_at_ms: i64,
    pub origin: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedControlResult {
    pub command_id: String,
    pub applied_revision: i64,
    pub control: SharedRunControl,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedIntakeQuery {
    pub after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedIntakeRecord {
    pub id: String,
    pub event_id: String,
    pub control_revision: i64,
    pub state: String,
    pub reason: Option<String>,
    pub received_at_ms: i64,
    pub expires_at_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedIntakePage {
    pub items: Vec<SharedIntakeRecord>,
    pub next_after: Option<String>,
}
