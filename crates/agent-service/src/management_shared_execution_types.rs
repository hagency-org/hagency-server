//! Closed first-turn execution protocol. Metadata-only runtime v1 is unchanged.
use crate::{
    Error, Result, key,
    management_agent_types::ManagedAgentReserveCommand,
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
};
use serde::{Deserialize, Serialize};
pub const SHARED_EXECUTION_CAPABILITY: &str = "hagency.managed_agents.execution.v1";
pub const SHARED_TEXT_POLICY: &str = "codex_text_v1";
pub const MAX_SHARED_TEXT: usize = 4096;
pub const MAX_SHARED_REPLY: usize = 32 * 1024;
fn exact(n: i64) -> bool {
    (1..=MAX_EXACT).contains(&n)
}
fn invalid() -> Error {
    Error::Invalid("invalid_shared_execution")
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedRuntimeCommand {
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub assignment_id: String,
    pub assignment_generation: i64,
    pub agent_reservation_id: String,
    pub reserve_command: ManagedAgentReserveCommand,
    pub reserve_command_digest: String,
    pub reserve_receipt_digest: String,
    pub policy: String,
    /// Deadline for original one-shot issuance, never an activity renewal.
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}
impl SharedRuntimeCommand {
    pub fn validate(&self, server: &str) -> Result<()> {
        for id in [
            &self.command_id,
            &self.assignment_id,
            &self.agent_reservation_id,
        ] {
            key(id)?;
        }
        self.reserve_command.validate(server)?;
        if self.version != 1
            || self.capability != SHARED_EXECUTION_CAPABILITY
            || self.operation != "agent.execution.provision"
            || self.assignment_generation != 1
            || self.policy != SHARED_TEXT_POLICY
            || self.reserve_command.definition.tool_mode != "none"
            || self.reserve_command_digest != allocation_digest(&self.reserve_command)?
            || !valid_digest(&self.reserve_receipt_digest)
            || !exact(self.issued_at_ms)
            || !exact(self.expires_at_ms)
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms - self.issued_at_ms > 30_000
            || self.expires_at_ms > self.reserve_command.expires_at_ms
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedProcessReceipt {
    pub version: u16,
    pub observation_id: String,
    pub command_id: String,
    pub command_digest: String,
    pub assignment_id: String,
    pub agent_reservation_id: String,
    pub ledger_revision: i64,
    /// Native original process binding; absent if no process was proved.
    pub process_digest: Option<String>,
    pub state: String,
    pub policy_digest: Option<String>,
}
impl SharedProcessReceipt {
    pub fn validate_for(&self, c: &SharedRuntimeCommand) -> Result<()> {
        key(&self.observation_id)?;
        if self.version != 1
            || self.command_id != c.command_id
            || self.command_digest != allocation_digest(c)?
            || self.assignment_id != c.assignment_id
            || self.agent_reservation_id != c.agent_reservation_id
            || !exact(self.ledger_revision)
            || !matches!(self.state.as_str(), "qualified" | "stopped" | "unknown")
            || self
                .process_digest
                .as_deref()
                .is_some_and(|v| !valid_digest(v))
            || self
                .policy_digest
                .as_deref()
                .is_some_and(|v| !valid_digest(v))
            || (self.state == "qualified"
                && (self.process_digest.is_none() || self.policy_digest.is_none()))
            || (self.state != "qualified" && self.policy_digest.is_some())
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedRuntimeDelivery {
    pub command: SharedRuntimeCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub execution_allowed: bool,
    pub cleanup_allowed: bool,
    pub recorded: Option<SharedProcessReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedProcessInput {
    pub command_digest: String,
    pub token: String,
    pub receipt: SharedProcessReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedActivityRef {
    pub runtime_command_id: String,
    pub epoch: i64,
    pub token: String,
}
impl SharedActivityRef {
    pub fn validate(&self) -> Result<()> {
        key(&self.runtime_command_id)?;
        if !exact(self.epoch) || !valid_digest(&self.token) {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedActivityRenewInput {
    pub authority: SharedActivityRef,
    pub renewal_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedActivityLease {
    pub authority: SharedActivityRef,
    pub command_digest: String,
    pub process_digest: String,
    pub expires_at_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedRuntimeAcknowledgement {
    pub observation_id: String,
    pub receipt_digest: String,
    /// Present only after a fresh qualified result and current authority check.
    /// Historical receipt recovery never renews an expired activity.
    pub activity: Option<SharedActivityLease>,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkItem {
    pub id: String,
    pub assignment_id: String,
    pub runtime_command_digest: String,
    pub event_id: String,
    pub room_id: String,
    pub requester_mxid: String,
    pub thread_root: String,
    pub body: String,
    pub body_digest: String,
    pub execution_id: String,
    pub created_at_ms: i64,
}
impl SharedWorkItem {
    pub fn validate(&self) -> Result<()> {
        key(&self.id)?;
        key(&self.assignment_id)?;
        key(&self.execution_id)?;
        if !valid_digest(&self.runtime_command_digest)
            || !valid_digest(&self.body_digest)
            || !exact(self.created_at_ms)
            || self.body.is_empty()
            || self.body.len() > MAX_SHARED_TEXT
            || self.body_digest != crate::hash(&self.body)
            || !self.event_id.starts_with('$')
            || self.event_id.len() > 255
            || !self.room_id.starts_with('!')
            || self.room_id.len() > 255
            || !self.requester_mxid.starts_with('@')
            || self.requester_mxid.len() > 255
            || !self.thread_root.starts_with('$')
            || self.thread_root.len() > 255
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkInput {
    pub authority: SharedActivityRef,
    pub work_id: String,
    pub work_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkStart {
    pub work: SharedWorkItem,
    pub work_digest: String,
    pub activity_epoch: i64,
    pub newly_started: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkReceipt {
    pub version: u16,
    pub observation_id: String,
    pub work_id: String,
    pub work_digest: String,
    pub execution_id: String,
    pub call_id: String,
    pub runtime_command_digest: String,
    pub process_digest: String,
    pub ledger_revision: i64,
    pub state: String,
    pub known_consumed_tokens: i64,
    pub retained_tokens: Option<i64>,
    pub usage_complete: bool,
    /// Exact durable result. Missing output is never reconstructed from a rerun.
    pub body: Option<String>,
    pub body_digest: Option<String>,
}
impl SharedWorkReceipt {
    pub fn validate_for(&self, w: &SharedWorkItem, process: &str) -> Result<()> {
        key(&self.observation_id)?;
        key(&self.call_id)?;
        if self.version != 1
            || self.work_id != w.id
            || self.work_digest != allocation_digest(w)?
            || self.execution_id != w.execution_id
            || self.runtime_command_digest != w.runtime_command_digest
            || self.process_digest != process
            || !valid_digest(&self.process_digest)
            || !exact(self.ledger_revision)
            || !matches!(self.state.as_str(), "completed" | "failed" | "unknown")
            || !(0..=MAX_EXACT).contains(&self.known_consumed_tokens)
            || self
                .retained_tokens
                .is_some_and(|n| n < self.known_consumed_tokens || n > MAX_EXACT)
            || self.usage_complete && self.retained_tokens != Some(self.known_consumed_tokens)
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 48 * 1024)
            || self.body.is_some() != (self.state == "completed")
            || self.body.is_some() != self.body_digest.is_some()
            || self.body.as_ref().is_some_and(|b| {
                b.is_empty()
                    || b.len() > MAX_SHARED_REPLY
                    || Some(crate::hash(b)) != self.body_digest
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkResultInput {
    pub authority: SharedActivityRef,
    pub receipt: SharedWorkReceipt,
}
/// UI observation only: no lease token, input, reply, provider or receipt data.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedWorkSummary {
    pub id: String,
    pub execution_id: String,
    pub state: String,
    pub reply_state: Option<String>,
    pub known_consumed_tokens: Option<i64>,
    pub retained_tokens: Option<i64>,
    pub usage_complete: bool,
    pub created_at_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedExecutionStatus {
    pub assignment_id: String,
    pub state: String,
    pub policy: String,
    pub activity_expires_at_ms: Option<i64>,
    pub work_available: bool,
    pub capacity_released: bool,
    pub recent_work: Vec<SharedWorkSummary>,
}
