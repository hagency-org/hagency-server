//! Executor transport only; reservation/fence receipts grant no model authority.
use crate::{
    Error, Result, key,
    management_agent_types::*,
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
};
use serde::{Deserialize, Serialize};

pub const AGENT_FENCE_CAPABILITY: &str = "hagency.managed_agents.fence.v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentFenceCommand {
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub reserve_command: ManagedAgentReserveCommand,
    pub reserve_command_digest: String,
    pub issued_at_ms: i64,
}
impl ManagedAgentFenceCommand {
    pub fn validate(&self, server: &str) -> Result<()> {
        key(&self.command_id)?;
        self.reserve_command.validate(server)?;
        if self.version != 1
            || self.capability != AGENT_FENCE_CAPABILITY
            || self.operation != "agent.fence"
            || !(1..=MAX_EXACT).contains(&self.issued_at_ms)
            || self.reserve_command_digest != allocation_digest(&self.reserve_command)?
        {
            return Err(Error::Invalid("invalid_agent_fence_command"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentFenceReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub reserve_command_digest: String,
    pub request_id: String,
    pub agent_id: String,
    pub agent_reservation_id: Option<String>,
    pub state: String,
    pub fence_revision: i64,
    pub ledger_revision: i64,
    pub known_consumed_tokens: i64,
    pub retained_tokens: Option<i64>,
    pub usage_complete: bool,
    pub process_state: String,
}
impl ManagedAgentFenceReceipt {
    pub fn validate_for(&self, c: &ManagedAgentFenceCommand) -> Result<()> {
        if let Some(id) = &self.agent_reservation_id {
            key(id)?;
        }
        let amount = (0..=MAX_EXACT).contains(&self.known_consumed_tokens)
            && self
                .retained_tokens
                .is_none_or(|n| (self.known_consumed_tokens..=MAX_EXACT).contains(&n));
        let state = match self.state.as_str() {
            "absent_fenced" => {
                self.agent_reservation_id.is_none()
                    && self.known_consumed_tokens == 0
                    && self.retained_tokens == Some(0)
                    && self.usage_complete
                    && self.process_state == "not_started"
            }
            "fenced" => {
                self.agent_reservation_id.is_some()
                    && matches!(
                        self.process_state.as_str(),
                        "not_started" | "stopped" | "unknown"
                    )
                    && (!self.usage_complete
                        || self.process_state != "unknown" && self.retained_tokens.is_some())
            }
            _ => false,
        };
        if self.version != 1
            || self.command_id != c.command_id
            || self.command_digest != allocation_digest(c)?
            || self.reserve_command_digest != c.reserve_command_digest
            || self.request_id != c.reserve_command.request_id
            || self.agent_id != c.reserve_command.proposed_agent_id
            || !(1..=MAX_EXACT).contains(&self.fence_revision)
            || !(self.fence_revision..=MAX_EXACT).contains(&self.ledger_revision)
            || !amount
            || !state
        {
            return Err(Error::Invalid("invalid_agent_fence_receipt"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentExecutorCommand {
    Reserve(ManagedAgentReserveCommand),
    Fence(ManagedAgentFenceCommand),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentExecutorReceipt {
    Reserve(ManagedAgentReservationReceipt),
    Fence(ManagedAgentFenceReceipt),
}
impl AgentExecutorReceipt {
    pub(crate) fn revision(&self) -> i64 {
        match self {
            Self::Reserve(r) => r.ledger_revision,
            Self::Fence(r) => r.ledger_revision,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentExecutorResultInput {
    pub command_digest: String,
    pub token: String,
    pub receipt: AgentExecutorReceipt,
}
impl AgentExecutorResultInput {
    pub(crate) fn validate(&self) -> Result<()> {
        if !valid_digest(&self.command_digest) || self.token.len() > 512 {
            return Err(Error::Invalid("invalid_agent_executor_result"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentExecutorDelivery {
    pub command: AgentExecutorCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub execution_allowed: bool,
    pub recorded: Option<AgentExecutorReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingManagedAssignment {
    pub id: String,
    pub request_id: String,
    pub agent_id: String,
    pub manager_mxid: String,
    pub executor_device_id: String,
    pub executor_generation: i64,
    pub agent_reservation_id: String,
    pub reserve_receipt_digest: String,
    pub state: String,
    pub generation: i64,
    pub execution_available: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentExecutorAcknowledgement {
    pub command_id: String,
    pub command_digest: String,
    pub receipt_digest: String,
    pub assignment: Option<PendingManagedAssignment>,
}
