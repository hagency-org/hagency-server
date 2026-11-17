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
    RenewPeriod(Box<ManagedAgentPeriodRenewCommand>),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentExecutorReceipt {
    Reserve(ManagedAgentReservationReceipt),
    Fence(ManagedAgentFenceReceipt),
    RenewPeriod(ManagedAgentPeriodReceipt),
}
impl AgentExecutorReceipt {
    pub(crate) fn revision(&self) -> i64 {
        match self {
            Self::Reserve(r) => r.ledger_revision,
            Self::Fence(r) => r.ledger_revision,
            Self::RenewPeriod(r) => r.ledger_revision,
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

/// The owner ledger database an executor device now numbers its receipts
/// from. The id is chosen by the device when it creates that database.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorLedgerInput {
    pub ledger_id: String,
    /// The device knows this database holds nothing of an earlier one: it was
    /// created empty. A first declaration then also ends what the device
    /// reported before it declared anything. Every later new id does so anyway.
    #[serde(default)]
    pub fresh: bool,
}
impl ExecutorLedgerInput {
    pub(crate) fn validate(&self) -> Result<()> {
        if !valid_digest(&self.ledger_id) {
            return Err(Error::Invalid("invalid_agent_executor_ledger"));
        }
        Ok(())
    }
}
/// Receipt numbers are unique within one epoch. Epoch 0 is a device that has
/// never declared a ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorLedger {
    pub ledger_id: String,
    pub epoch: i64,
}

/// Immutable owner-authorized credit for exactly one calendar period. It is
/// delivered through the enrolled executor queue and never mints central credit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentPeriodRenewCommand {
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub reserve_command: ManagedAgentReserveCommand,
    pub reserve_command_digest: String,
    pub agent_reservation_id: String,
    pub assignment_id: String,
    pub assignment_generation: i64,
    pub current_grant: crate::management_allocation_types::NativeGrantReceipt,
    pub current_grant_digest: String,
    pub period: crate::recurring_quota::Period,
    pub decision_actor_mxid: String,
    pub renewal_mode: crate::recurring_quota::RenewalMode,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}
impl ManagedAgentPeriodRenewCommand {
    pub fn validate(&self, server: &str) -> Result<()> {
        for value in [
            &self.command_id,
            &self.agent_reservation_id,
            &self.assignment_id,
        ] {
            key(value)?;
        }
        self.reserve_command.validate(server)?;
        self.current_grant.validate(server)?;
        let c = &self.reserve_command;
        let g = &self.current_grant;
        let original = c
            .pool_grant
            .as_ref()
            .ok_or(Error::Invalid("invalid_agent_period_command"))?;
        let calendar = original
            .calendar
            .as_ref()
            .ok_or(Error::Invalid("invalid_agent_period_command"))?;
        let expected = calendar
            .period_at(
                self.issued_at_ms
                    .try_into()
                    .map_err(|_| Error::Invalid("invalid_agent_period_command"))?,
            )
            .map_err(|_| Error::Invalid("invalid_agent_period_command"))?;
        if self.version != 1
            || self.capability != "hagency.agent_periods.v1"
            || self.operation != "agent.period.renew"
            || self.reserve_command_digest != allocation_digest(c)?
            || self.current_grant_digest != allocation_digest(g)?
            || self.assignment_generation != 1
            || g.renewal_mode.or(c.renewal_mode) != Some(self.renewal_mode)
            || g.calendar.is_none()
            || g.calendar.as_ref() != Some(calendar)
            || g.contribution_id != original.contribution_id
            || g.owner_mxid != original.owner_mxid
            || g.owner_mxid != self.decision_actor_mxid
            || g.executor_device_id != c.executor_device_id
            || g.executor_generation != c.executor_generation
            || g.resource_id != c.resource_id
            || g.resource_revision != c.resource_revision
            || !g.eligible_managers.contains(&c.manager_mxid)
            || g.request_limit() < c.requested_tokens
            || self.period != expected
            || self.period.start_ms < original.period_end_ms as u64
            || self.period.key != g.period_key
            || self.period.start_ms != g.period_start_ms as u64
            || self.period.end_ms != g.period_end_ms as u64
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms > g.period_end_ms.min(g.delegation_expires_at_ms)
        {
            return Err(Error::Invalid("invalid_agent_period_command"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentPeriodReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub request_id: String,
    pub agent_id: String,
    pub agent_reservation_id: String,
    pub period: crate::recurring_quota::Period,
    pub allocated_tokens: i64,
    pub ledger_revision: i64,
    pub state: String,
    #[serde(default)]
    pub reason: Option<String>,
}
impl ManagedAgentPeriodReceipt {
    pub fn validate_for(&self, c: &ManagedAgentPeriodRenewCommand) -> Result<()> {
        if self.version != 1
            || self.command_id != c.command_id
            || self.command_digest != allocation_digest(c)?
            || self.request_id != c.reserve_command.request_id
            || self.agent_id != c.reserve_command.proposed_agent_id
            || self.agent_reservation_id != c.agent_reservation_id
            || self.period != c.period
            || !(1..=MAX_EXACT).contains(&self.ledger_revision)
            || self.ledger_revision <= c.current_grant.ledger_revision
            || !(self.state == "authorized"
                && self.allocated_tokens == c.reserve_command.requested_tokens
                && self.reason.is_none()
                || self.state == "refused"
                    && self.allocated_tokens == 0
                    && self
                        .reason
                        .as_ref()
                        .is_some_and(|r| !r.is_empty() && r.len() <= 160))
        {
            return Err(Error::Invalid("invalid_agent_period_receipt"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod calendar_wire_tests {
    use super::*;
    #[test]
    fn native_period_renewal_vector_has_identical_server_encoding_and_digest() {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/agent-period-v1.json")).unwrap();
        let command: ManagedAgentPeriodRenewCommand =
            serde_json::from_value(value["command"].clone()).unwrap();
        command.validate("example.test").unwrap();
        assert_eq!(serde_json::to_value(&command).unwrap(), value["command"]);
        assert_eq!(allocation_digest(&command).unwrap(), value["commandDigest"]);
        let receipt: ManagedAgentPeriodReceipt =
            serde_json::from_value(value["receipt"].clone()).unwrap();
        receipt.validate_for(&command).unwrap();
        assert_eq!(serde_json::to_value(&receipt).unwrap(), value["receipt"]);
        assert_eq!(allocation_digest(&receipt).unwrap(), value["receiptDigest"]);
        let mut bad = command.clone();
        bad.period.end_ms -= 1;
        assert!(bad.validate("example.test").is_err());
        let mut bad = receipt.clone();
        bad.ledger_revision = command.current_grant.ledger_revision;
        assert!(bad.validate_for(&command).is_err());
    }
}
