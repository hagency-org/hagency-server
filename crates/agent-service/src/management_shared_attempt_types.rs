//! Append-only successor process protocol. Original v1 bytes remain unchanged.
use crate::management_shared_tool_policy::SharedAgentToolPolicy;
use crate::{
    Error, Result, key,
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
    management_shared_execution_types::{SharedActivityLease, SharedRuntimeCommand},
};
use serde::{Deserialize, Serialize};

pub const SHARED_ATTEMPT_CAPABILITY: &str = "hagency.managed_agents.execution.v2";
pub const MAX_ATTEMPT_BYTES: usize = 32 * 1024;
fn invalid() -> Error {
    Error::Invalid("invalid_shared_attempt")
}
fn positive(n: i64) -> bool {
    (1..=MAX_EXACT).contains(&n)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetObservation {
    pub observed_ledger_revision: i64,
    pub allocated_tokens: i64,
    pub known_consumed_tokens: i64,
    pub retained_tokens: i64,
    pub remaining_tokens: i64,
    pub usage_complete: bool,
    pub liability_digest: String,
}
impl SharedBudgetObservation {
    pub fn validate(&self) -> Result<()> {
        if !positive(self.observed_ledger_revision)
            || !positive(self.allocated_tokens)
            || [
                self.known_consumed_tokens,
                self.retained_tokens,
                self.remaining_tokens,
            ]
            .iter()
            .any(|n| !(0..=MAX_EXACT).contains(n))
            || self.retained_tokens < self.known_consumed_tokens
            || self.remaining_tokens != (self.allocated_tokens - self.retained_tokens).max(0)
            || self.usage_complete && self.retained_tokens != self.known_consumed_tokens
            || !valid_digest(&self.liability_digest)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptPredecessor {
    pub command_id: String,
    pub command_digest: String,
    pub process_attempt: i64,
    pub stopped_receipt_digest: String,
    pub process_digest: String,
}
impl SharedAttemptPredecessor {
    pub fn validate(&self) -> Result<()> {
        key(&self.command_id)?;
        if !positive(self.process_attempt)
            || [
                &self.command_digest,
                &self.stopped_receipt_digest,
                &self.process_digest,
            ]
            .iter()
            .any(|v| !valid_digest(v))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptStartInput {
    /// Explicit owner opt-in, pinned in this successor start and command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web_search: bool,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub tool_policy: Option<SharedAgentToolPolicy>,
    pub start_intent_id: String,
    pub assignment_id: String,
    pub predecessor: SharedAttemptPredecessor,
    pub budget_observation: SharedBudgetObservation,
}
impl SharedAttemptStartInput {
    pub fn validate(&self) -> Result<()> {
        if let Some(policy)=&self.tool_policy {policy.validate()?;}
        key(&self.start_intent_id)?;
        key(&self.assignment_id)?;
        self.predecessor.validate()?;
        self.budget_observation.validate()?;
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptCommand {
    /// Explicit owner opt-in, pinned in this successor start and command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web_search: bool,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub tool_policy: Option<SharedAgentToolPolicy>,
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub assignment_id: String,
    pub assignment_generation: i64,
    pub agent_reservation_id: String,
    pub process_attempt: i64,
    pub original_runtime_command: SharedRuntimeCommand,
    pub original_runtime_digest: String,
    pub predecessor: SharedAttemptPredecessor,
    pub budget_observation: SharedBudgetObservation,
    pub start_intent_id: String,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}
impl SharedAttemptCommand {
    pub fn validate(&self, server: &str) -> Result<()> {
        if let Some(policy)=&self.tool_policy {policy.validate()?;}
        for id in [
            &self.command_id,
            &self.assignment_id,
            &self.agent_reservation_id,
            &self.start_intent_id,
        ] {
            key(id)?;
        }
        self.original_runtime_command.validate(server)?;
        self.predecessor.validate()?;
        self.budget_observation.validate()?;
        let original = &self.original_runtime_command;
        if self.version != 2
            || self.capability != SHARED_ATTEMPT_CAPABILITY
            || self.operation != "agent.execution.provision"
            || self.assignment_generation != original.assignment_generation
            || self.assignment_id != original.assignment_id
            || self.agent_reservation_id != original.agent_reservation_id
            || self.original_runtime_digest != allocation_digest(original)?
            || !positive(self.process_attempt)
            || self.process_attempt < 2
            || self.predecessor.process_attempt != self.process_attempt - 1
            || self.command_id == self.predecessor.command_id
            || self.command_id == original.command_id
            || self.process_attempt == 2
                && (self.predecessor.command_id != original.command_id
                    || self.predecessor.command_digest != self.original_runtime_digest)
            || self.budget_observation.allocated_tokens != original.reserve_command.requested_tokens
            || self.budget_observation.remaining_tokens <= 0
            || !positive(self.issued_at_ms)
            || !positive(self.expires_at_ms)
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms - self.issued_at_ms > 30_000
            // Pool Agents: capped by the pool chain's lifetime at issuance (E8).
            || (original.reserve_command.pool_grant.is_none()
                && self.expires_at_ms > original.reserve_command.expires_at_ms)
            || serde_json::to_vec(self).map_or(true, |v| v.len() > MAX_ATTEMPT_BYTES)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptProcessReceipt {
    pub version: u16,
    pub observation_id: String,
    pub command_id: String,
    pub command_digest: String,
    pub assignment_id: String,
    pub agent_reservation_id: String,
    pub process_attempt: i64,
    pub original_runtime_digest: String,
    pub ledger_revision: i64,
    pub process_digest: Option<String>,
    pub state: String,
    pub policy_digest: Option<String>,
}
impl SharedAttemptProcessReceipt {
    pub fn validate_for(&self, c: &SharedAttemptCommand) -> Result<()> {
        key(&self.observation_id)?;
        if self.version != 2
            || self.command_id != c.command_id
            || self.command_digest != allocation_digest(c)?
            || self.assignment_id != c.assignment_id
            || self.agent_reservation_id != c.agent_reservation_id
            || self.process_attempt != c.process_attempt
            || self.original_runtime_digest != c.original_runtime_digest
            || !positive(self.ledger_revision)
            || !matches!(self.state.as_str(), "qualified" | "stopped" | "unknown")
            || self
                .process_digest
                .as_deref()
                .is_some_and(|v| !valid_digest(v))
            || self
                .policy_digest
                .as_deref()
                .is_some_and(|v| !valid_digest(v))
            || self.state == "qualified"
                && (self.process_digest.is_none() || self.policy_digest.is_none())
            || self.state == "stopped" && self.process_digest.is_none()
            || self.state != "qualified" && self.policy_digest.is_some()
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptDelivery {
    pub command: SharedAttemptCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub execution_allowed: bool,
    pub cleanup_allowed: bool,
    pub recorded: Option<SharedAttemptProcessReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptResultInput {
    pub command_digest: String,
    pub token: String,
    pub receipt: SharedAttemptProcessReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedAttemptAcknowledgement {
    pub observation_id: String,
    pub receipt_digest: String,
    pub activity: Option<SharedActivityLease>,
    pub state: String,
}
