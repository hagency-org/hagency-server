//! Finite runtime qualification only. These receipts grant no work or refund.
use crate::{
    Error, Result, key,
    management_agent_types::ManagedAgentReserveCommand,
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
};
use serde::{Deserialize, Serialize};
pub const AGENT_RUNTIME_CAPABILITY: &str = "hagency.managed_agents.runtime.v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeProvisionCommand {
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
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}
impl ManagedRuntimeProvisionCommand {
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
            || self.capability != AGENT_RUNTIME_CAPABILITY
            || self.operation != "agent.runtime.provision"
            || self.assignment_generation != 1
            || self.policy != "codex_no_tools_v1"
            || self.reserve_command.definition.tool_mode != "none"
            || self.reserve_command_digest != allocation_digest(&self.reserve_command)?
            || !valid_digest(&self.reserve_receipt_digest)
            || !(1..=MAX_EXACT).contains(&self.issued_at_ms)
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms - self.issued_at_ms > 30_000
            || self.expires_at_ms > self.reserve_command.expires_at_ms
        {
            return Err(Error::Invalid("invalid_managed_runtime_command"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeStopCommand {
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub provision_command: ManagedRuntimeProvisionCommand,
    pub provision_command_digest: String,
    pub issued_at_ms: i64,
}
impl ManagedRuntimeStopCommand {
    pub fn validate(&self, server: &str) -> Result<()> {
        key(&self.command_id)?;
        self.provision_command.validate(server)?;
        if self.version != 1
            || self.capability != AGENT_RUNTIME_CAPABILITY
            || self.operation != "agent.runtime.stop"
            || self.provision_command_digest != allocation_digest(&self.provision_command)?
            || !(1..=MAX_EXACT).contains(&self.issued_at_ms)
        {
            return Err(Error::Invalid("invalid_managed_runtime_command"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ManagedRuntimeCommand {
    Provision(ManagedRuntimeProvisionCommand),
    Stop(ManagedRuntimeStopCommand),
}
impl ManagedRuntimeCommand {
    pub fn validate(&self, s: &str) -> Result<()> {
        match self {
            Self::Provision(c) => c.validate(s),
            Self::Stop(c) => c.validate(s),
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Provision(c) => &c.command_id,
            Self::Stop(c) => &c.command_id,
        }
    }
    pub fn original(&self) -> &ManagedRuntimeProvisionCommand {
        match self {
            Self::Provision(c) => c,
            Self::Stop(c) => &c.provision_command,
        }
    }
}
/// This is process evidence, never token settlement. Capacity remains held.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeReceipt {
    pub version: u16,
    pub observation_id: String,
    pub command_id: String,
    pub command_digest: String,
    pub assignment_id: String,
    pub agent_reservation_id: String,
    pub reserve_command_digest: String,
    pub reserve_receipt_digest: String,
    pub effect_id: String,
    pub ledger_revision: i64,
    pub state: String,
    pub process_state: String,
    pub runtime_digest: Option<String>,
}
impl ManagedRuntimeReceipt {
    pub fn validate_for(&self, c: &ManagedRuntimeCommand) -> Result<()> {
        key(&self.observation_id)?;
        let p = c.original();
        let shape = match c {
            ManagedRuntimeCommand::Provision(_) => match self.state.as_str() {
                "qualified" => {
                    self.process_state == "live"
                        && self.runtime_digest.as_deref().is_some_and(valid_digest)
                }
                "failed" => self.process_state == "not_started" && self.runtime_digest.is_none(),
                "unknown" => self.process_state == "unknown" && self.runtime_digest.is_none(),
                _ => false,
            },
            ManagedRuntimeCommand::Stop(_) => {
                matches!(
                    (self.state.as_str(), self.process_state.as_str()),
                    ("stopped", "stopped")
                        | ("not_started", "not_started")
                        | ("unknown", "unknown")
                ) && self.runtime_digest.is_none()
            }
        };
        if self.version != 1
            || self.command_id != c.id()
            || self.command_digest != allocation_digest(c)?
            || self.assignment_id != p.assignment_id
            || self.agent_reservation_id != p.agent_reservation_id
            || self.reserve_command_digest != p.reserve_command_digest
            || self.reserve_receipt_digest != p.reserve_receipt_digest
            || self.effect_id != format!("{}_provision", p.agent_reservation_id)
            || !(1..=MAX_EXACT).contains(&self.ledger_revision)
            || !shape
        {
            return Err(Error::Invalid("invalid_managed_runtime_receipt"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeDelivery {
    pub command: ManagedRuntimeCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub execution_allowed: bool,
    pub recorded: Option<ManagedRuntimeReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeRecoverInput {
    pub command_digest: String,
    #[serde(default)]
    pub observation_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeResultInput {
    pub command_digest: String,
    pub token: String,
    pub receipt: ManagedRuntimeReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeReadiness {
    pub assignment_id: String,
    pub command_id: String,
    pub state: String,
    pub process_state: String,
    pub provision_expires_at_ms: i64,
    pub latest: Option<ManagedRuntimeReceipt>,
    pub work_available: bool,
    pub capacity_released: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRuntimeAcknowledgement {
    pub command_id: String,
    pub command_digest: String,
    pub receipt_digest: String,
    pub runtime: ManagedRuntimeReadiness,
}
