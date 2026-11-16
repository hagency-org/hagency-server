//! Durable demand descriptions. None of these DTOs is native execution authority.
use crate::{
    Error, Result, key,
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
    management_shared_attempt_types::{
        SharedAttemptAcknowledgement, SharedAttemptResultInput, SharedAttemptStartInput,
        SharedBudgetObservation,
    },
};
/// Retries per event after a provably never-started attempt (ADR-193 amendment).
pub const MAX_DEMAND_RETRIES: i64 = 3;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandOffer {
    pub version: u16,
    pub demand_id: String,
    pub assignment_id: String,
    pub event_id: String,
    pub event_digest: String,
    pub control_revision: i64,
    pub received_at_ms: i64,
    pub predecessor_command_id: String,
    pub predecessor_command_digest: String,
}
impl SharedDemandOffer {
    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.demand_id,
            &self.assignment_id,
            &self.predecessor_command_id,
        ] {
            key(id)?;
        }
        if self.version != 1
            || !self.event_id.starts_with('$')
            || !(2..=255).contains(&self.event_id.len())
            || self.event_id.chars().any(char::is_control)
            || !valid_digest(&self.event_digest)
            || !valid_digest(&self.predecessor_command_digest)
            || !(1..=MAX_EXACT).contains(&self.control_revision)
            || !(1..=MAX_EXACT).contains(&self.received_at_ms)
        {
            return Err(Error::Invalid("invalid_shared_demand"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandStartInput {
    pub demand_digest: String,
    pub expected_control_revision: i64,
    pub start: SharedAttemptStartInput,
    /// A retry names the demand's not_started attempt, not the offer's
    /// predecessor. Omitted for the original start (bytes unchanged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<i64>,
}
impl SharedDemandStartInput {
    pub fn validate_for(&self, o: &SharedDemandOffer) -> Result<()> {
        self.start.validate()?;
        let predecessor = match self.retry {
            None => {
                self.start.predecessor.command_id != o.predecessor_command_id
                    || self.start.predecessor.command_digest != o.predecessor_command_digest
            }
            Some(n) => !(1..=MAX_DEMAND_RETRIES).contains(&n),
        };
        if predecessor
            || self.demand_digest != allocation_digest(o)?
            || self.expected_control_revision != o.control_revision
            || self.start.assignment_id != o.assignment_id
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 16384)
        {
            return Err(Error::Conflict("shared_demand_changed"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandBinding {
    pub version: u16,
    pub demand_id: String,
    pub demand_digest: String,
    pub control_revision: i64,
    pub assignment_id: String,
    pub event_id: String,
    pub received_at_ms: i64,
    pub runtime_command_id: String,
    pub runtime_command_digest: String,
    pub work_id: String,
    pub work_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandStatus {
    pub offer: SharedDemandOffer,
    pub demand_digest: String,
    pub state: String,
    pub runtime_command_id: Option<String>,
    pub binding: Option<SharedDemandBinding>,
    /// The latest retry, once the executor has asked for one. Never present
    /// for an executor that has not (the old client never sees it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<i64>,
}
/// The executor's not_started evidence for the demand's latest attempt, with
/// its request to retry the same event in place. Only the executor proves
/// non-execution; the server never retries on its own.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandRetryInput {
    pub demand_digest: String,
    pub expected_control_revision: i64,
    pub retry: i64,
    pub attempt_id: String,
    pub result: SharedAttemptResultInput,
}
impl SharedDemandRetryInput {
    pub fn validate(&self) -> Result<()> {
        key(&self.attempt_id)?;
        if !valid_digest(&self.demand_digest)
            || !(1..=MAX_EXACT).contains(&self.expected_control_revision)
            || !(1..=MAX_DEMAND_RETRIES + 1).contains(&self.retry)
            || self.result.receipt.state != "not_started"
        {
            return Err(Error::Invalid("invalid_shared_demand_retry"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandRetryReply {
    pub acknowledgement: SharedAttemptAcknowledgement,
    pub demand: SharedDemandStatus,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandPoll {
    pub demand: Option<SharedDemandStatus>,
    pub next_after: Option<String>,
}
/// A demand poll. `retries`: the executor runs retries, so a status may say
/// `retry_offered` and carry `retry` (ADR-193 amendment).
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandPollInput {
    pub after: Option<String>,
    #[serde(default)]
    pub retries: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandRecoverInput {
    pub demand_digest: String,
    /// The executor runs retries; only then may a status say so.
    #[serde(default)]
    pub retries: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandWorkInput {
    pub work_digest: String,
    pub runtime_command_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandWorkBinding {
    pub offer: SharedDemandOffer,
    pub demand_digest: String,
    pub binding: SharedDemandBinding,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDemandBlockInput {
    pub demand_digest: String,
    pub expected_control_revision: i64,
    pub reason: String,
    pub budget: Option<SharedBudgetObservation>,
    #[serde(default)]
    pub retries: bool,
}
/// Role/configuration observations only, never native liveness or free capacity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedRunControlReadiness {
    pub control: crate::management_shared_intake_types::SharedRunControl,
    pub can_run: bool,
    pub can_stop: bool,
    /// Exact original executor-device observer; native Stop must persist its local fence.
    pub requires_local_stop: bool,
    pub run_blocking_reason: Option<String>,
    pub intake_enabled: bool,
    pub automatic_delivery_enabled: bool,
}
