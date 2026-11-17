//! Matrix membership only. No runtime or work authority is represented.
use crate::{
    Error, Result, key,
    management_agent_types::ManagedAgentReserveCommand,
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
};
use serde::{Deserialize, Serialize};
pub const AGENT_MATRIX_CAPABILITY: &str = "hagency.managed_agents.matrix.v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentMatrixChallenge {
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
    pub space_id: String,
    pub puppet_mxid: String,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}
impl ManagedAgentMatrixChallenge {
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
            || self.capability != AGENT_MATRIX_CAPABILITY
            || self.operation != "agent.matrix.check"
            || self.assignment_generation != 1
            || self.reserve_command_digest != allocation_digest(&self.reserve_command)?
            || !valid_digest(&self.reserve_receipt_digest)
            || if self.reserve_command.pool_grant.is_some() { !self.space_id.is_empty() || self.reserve_command.room_id.is_empty() } else { !self.space_id.starts_with('!') }
            || self.space_id.len() > 255
            || !self.puppet_mxid.starts_with("@_hagency_")
            || !self.puppet_mxid.ends_with(&format!(":{server}"))
            || self.puppet_mxid.len() > 255
            || !(1..=MAX_EXACT).contains(&self.issued_at_ms)
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms - self.issued_at_ms > 30_000
            // Pool Agents: capped by the pool chain's lifetime at issuance (E8).
            || (self.reserve_command.pool_grant.is_none()
                && self.expires_at_ms > self.reserve_command.expires_at_ms)
        {
            return Err(Error::Invalid("invalid_agent_matrix_challenge"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentMatrixObservation {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub assignment_id: String,
    pub agent_reservation_id: String,
    pub reserve_command_digest: String,
    pub reserve_receipt_digest: String,
    pub observed_ledger_revision: i64,
    pub eligible: bool,
    pub reason: Option<String>,
    pub valid_until_ms: i64,
}
impl ManagedAgentMatrixObservation {
    pub fn validate_for(&self, c: &ManagedAgentMatrixChallenge) -> Result<()> {
        if self.version != 1
            || self.command_id != c.command_id
            || self.command_digest != allocation_digest(c)?
            || self.assignment_id != c.assignment_id
            || self.agent_reservation_id != c.agent_reservation_id
            || self.reserve_command_digest != c.reserve_command_digest
            || self.reserve_receipt_digest != c.reserve_receipt_digest
            || !(1..=MAX_EXACT).contains(&self.observed_ledger_revision)
            || self.valid_until_ms != c.expires_at_ms
            || if self.eligible {
                self.reason.is_some()
            } else {
                self.reason.as_deref() != Some("native_reservation_unavailable")
            }
        {
            return Err(Error::Invalid("invalid_agent_matrix_observation"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMatrixDelivery {
    pub command: ManagedAgentMatrixChallenge,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub execution_allowed: bool,
    pub recorded: Option<ManagedAgentMatrixObservation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMatrixResultInput {
    pub command_digest: String,
    pub token: String,
    pub observation: ManagedAgentMatrixObservation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMatrixReadiness {
    pub assignment_id: String,
    pub request_id: String,
    pub agent_id: String,
    pub puppet_mxid: String,
    pub room_id: String,
    pub state: String,
    pub verified_at_ms: Option<i64>,
    pub cleanup_observed_at_ms: Option<i64>,
    pub blocking_reason: Option<String>,
    pub runtime_state: String,
    pub work_available: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMatrixAcknowledgement {
    pub command_id: String,
    pub command_digest: String,
    pub observation_digest: String,
    pub membership: AgentMatrixReadiness,
}
