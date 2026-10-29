//! Versioned budget-aware descriptions. Native custody is never deserialized.
use crate::management_shared_tool_policy::SharedAgentToolPolicy;
use crate::{
    Error, Result, key,
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
    management_shared_attempt_types::{SharedAttemptPredecessor, SharedBudgetObservation},
    management_shared_demand_types::{SharedDemandBinding, SharedDemandOffer},
    management_shared_execution_types::{SharedRuntimeCommand, SharedWorkItem},
};
use serde::{Deserialize, Serialize};

pub const SHARED_BUDGET_CAPABILITY: &str = "hagency.managed_agents.execution.v3";
pub const SHARED_BUDGET_DISCOVERY: &str = "shared-budget-execution-v1";
fn invalid() -> Error {
    Error::Invalid("invalid_shared_budget_execution")
}
// An empty processDigest is only meaningful with a verified v3 not_started
// receipt. V2 continues to require an actual-process digest.
fn validate_budget_predecessor(p: &SharedAttemptPredecessor) -> Result<()> {
    key(&p.command_id)?;
    if !positive(p.process_attempt) || !valid_digest(&p.command_digest)
        || !valid_digest(&p.stopped_receipt_digest)
        || (!p.process_digest.is_empty() && !valid_digest(&p.process_digest)) {return Err(invalid())}
    Ok(())
}
fn positive(n: i64) -> bool {
    (1..=MAX_EXACT).contains(&n)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedExecutionBudget {
    pub version: u16,
    pub original_reserve_receipt_digest: String,
    pub budget_revision: i64,
    pub budget_digest: String,
    pub effective_ceiling_tokens: i64,
    pub applied_command_id: String,
    pub applied_command_digest: String,
    pub applied_receipt_digest: String,
}
impl SharedExecutionBudget {
    pub fn validate(&self) -> Result<()> {
        key(&self.applied_command_id)?;
        if self.version != 1
            || !(2..=MAX_EXACT).contains(&self.budget_revision)
            || !positive(self.effective_ceiling_tokens)
            || [
                &self.original_reserve_receipt_digest,
                &self.budget_digest,
                &self.applied_command_digest,
                &self.applied_receipt_digest,
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
pub struct SharedBudgetAttemptStartInput {
    /// Explicit owner opt-in, pinned in this successor start and command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web_search: bool,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub tool_policy: Option<SharedAgentToolPolicy>,
    pub start_intent_id: String,
    pub assignment_id: String,
    pub predecessor: SharedAttemptPredecessor,
    pub execution_budget: SharedExecutionBudget,
    pub budget_observation: SharedBudgetObservation,
}
impl SharedBudgetAttemptStartInput {
    pub fn validate(&self) -> Result<()> {
        if let Some(p)=&self.tool_policy {p.validate()?;}
        key(&self.start_intent_id)?;
        key(&self.assignment_id)?;
        validate_budget_predecessor(&self.predecessor)?;
        self.execution_budget.validate()?;
        self.budget_observation.validate()?;
        if self.budget_observation.allocated_tokens
            != self.execution_budget.effective_ceiling_tokens
            || self.budget_observation.remaining_tokens <= 0
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 16 * 1024)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptCommand {
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
    pub execution_budget: SharedExecutionBudget,
    pub budget_observation: SharedBudgetObservation,
    pub start_intent_id: String,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}
impl SharedBudgetAttemptCommand {
    pub fn validate(&self, server: &str) -> Result<()> {
        if let Some(p)=&self.tool_policy {p.validate()?;}
        for id in [
            &self.command_id,
            &self.assignment_id,
            &self.agent_reservation_id,
            &self.start_intent_id,
        ] {
            key(id)?;
        }
        self.original_runtime_command.validate(server)?;
        validate_budget_predecessor(&self.predecessor)?;
        self.execution_budget.validate()?;
        self.budget_observation.validate()?;
        let original = &self.original_runtime_command;
        if self.version != 3
            || self.capability != SHARED_BUDGET_CAPABILITY
            || self.operation != "agent.execution.provision"
            || self.assignment_id != original.assignment_id
            || self.assignment_generation != original.assignment_generation
            || self.agent_reservation_id != original.agent_reservation_id
            || self.original_runtime_digest != allocation_digest(original)?
            || !(2..=MAX_EXACT).contains(&self.process_attempt)
            || self.predecessor.process_attempt != self.process_attempt - 1
            || self.command_id == self.predecessor.command_id
            || self.command_id == original.command_id
            || self.process_attempt == 2
                && (self.predecessor.command_id != original.command_id
                    || self.predecessor.command_digest != self.original_runtime_digest)
            || self.execution_budget.original_reserve_receipt_digest
                != original.reserve_receipt_digest
            || self.execution_budget.effective_ceiling_tokens
                <= original.reserve_command.requested_tokens
            || self.budget_observation.allocated_tokens
                != self.execution_budget.effective_ceiling_tokens
            || self.budget_observation.remaining_tokens <= 0
            || !positive(self.issued_at_ms)
            || !positive(self.expires_at_ms)
            || self.expires_at_ms <= self.issued_at_ms
            || self.expires_at_ms - self.issued_at_ms > 30_000
            || self.expires_at_ms > original.reserve_command.expires_at_ms
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 32 * 1024)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptProcessReceipt {
    pub version: u16,
    pub observation_id: String,
    pub command_id: String,
    pub command_digest: String,
    pub assignment_id: String,
    pub agent_reservation_id: String,
    pub process_attempt: i64,
    pub original_runtime_digest: String,
    pub budget_revision: i64,
    pub budget_digest: String,
    pub ledger_revision: i64,
    pub process_digest: Option<String>,
    pub state: String,
    pub policy_digest: Option<String>,
}
impl SharedBudgetAttemptProcessReceipt {
    pub fn validate_for(&self, c: &SharedBudgetAttemptCommand) -> Result<()> {
        key(&self.observation_id)?;
        if self.version != 3
            || self.command_id != c.command_id
            || self.command_digest != allocation_digest(c)?
            || self.assignment_id != c.assignment_id
            || self.agent_reservation_id != c.agent_reservation_id
            || self.process_attempt != c.process_attempt
            || self.original_runtime_digest != c.original_runtime_digest
            || self.budget_revision != c.execution_budget.budget_revision
            || self.budget_digest != c.execution_budget.budget_digest
            || !positive(self.ledger_revision)
            || !matches!(self.state.as_str(), "qualified" | "stopped" | "unknown" | "not_started")
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
            || self.state == "not_started" && self.process_digest.is_some()
            || self.state != "qualified" && self.policy_digest.is_some()
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptDelivery {
    pub command: SharedBudgetAttemptCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub execution_allowed: bool,
    pub cleanup_allowed: bool,
    pub recorded: Option<SharedBudgetAttemptProcessReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetAttemptResultInput {
    pub command_digest: String,
    pub token: String,
    pub receipt: SharedBudgetAttemptProcessReceipt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedOriginalBudgetBlock {
    pub reason: String,
    pub payload_digest: String,
}
impl SharedOriginalBudgetBlock {
    pub fn validate(&self) -> Result<()> {
        if !matches!(
            self.reason.as_str(),
            "budget_blocked" | "usage_unrepresentable" | "head_changed"
        ) || !valid_digest(&self.payload_digest)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetDemandStartInput {
    pub demand_digest: String,
    pub expected_control_revision: i64,
    pub original_block: Option<SharedOriginalBudgetBlock>,
    pub start: SharedBudgetAttemptStartInput,
}
impl SharedBudgetDemandStartInput {
    pub fn validate_for(&self, o: &SharedDemandOffer) -> Result<()> {
        o.validate()?;
        self.start.validate()?;
        if let Some(b) = &self.original_block {
            b.validate()?;
            if b.reason != "budget_blocked" {
                return Err(invalid());
            }
        }
        if self.demand_digest != allocation_digest(o)?
            || self.expected_control_revision != o.control_revision
            || self.start.assignment_id != o.assignment_id
            || self.start.predecessor.command_id != o.predecessor_command_id
            || self.start.predecessor.command_digest != o.predecessor_command_digest
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 16 * 1024)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetDemandStatus {
    pub offer: SharedDemandOffer,
    pub demand_digest: String,
    pub state: String,
    pub execution_budget: SharedExecutionBudget,
    pub original_block: Option<SharedOriginalBudgetBlock>,
    pub runtime_command_id: Option<String>,
    pub binding: Option<SharedDemandBinding>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetDemandPoll {
    pub demand: Option<SharedBudgetDemandStatus>,
    pub next_after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetDemandBlockInput {
    pub demand_digest: String,
    pub expected_control_revision: i64,
    pub original_block: Option<SharedOriginalBudgetBlock>,
    pub execution_budget: SharedExecutionBudget,
    pub reason: String,
    pub budget: Option<SharedBudgetObservation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetWorkRefusalInput {
    pub runtime_command_digest: String,
    pub work_digest: String,
    pub input_digest: String,
}
impl SharedBudgetWorkRefusalInput {
    pub fn validate(&self) -> Result<()> {
        if [
            &self.runtime_command_digest,
            &self.work_digest,
            &self.input_digest,
        ]
        .iter()
        .any(|v| !valid_digest(v))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
/// Server evidence that this exact work was permanently fenced before any
/// accepted start. Native must separately prove its exact call was unissued.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetWorkRefusal {
    pub version: u16,
    pub refusal_id: String,
    pub work_id: String,
    pub work_digest: String,
    pub runtime_command_id: String,
    pub runtime_command_digest: String,
    pub input_digest: String,
    pub execution_budget: SharedExecutionBudget,
    pub current_budget_digest: String,
    pub state: String,
    pub reason: String,
    pub refused_at_ms: i64,
}
impl SharedBudgetWorkRefusal {
    pub fn validate_for(
        &self,
        c: &SharedBudgetAttemptCommand,
        w: &SharedWorkItem,
        input_digest: &str,
    ) -> Result<()> {
        w.validate()?;
        self.execution_budget.validate()?;
        key(&self.refusal_id)?;
        if self.version != 1
            || self.refusal_id != format!("sbr_{}", w.id)
            || self.work_id != w.id
            || self.work_digest != allocation_digest(w)?
            || w.assignment_id != c.assignment_id
            || w.runtime_command_digest != allocation_digest(c)?
            || self.runtime_command_id != c.command_id
            || self.runtime_command_digest != w.runtime_command_digest
            || self.input_digest != input_digest
            || !valid_digest(input_digest)
            || self.execution_budget != c.execution_budget
            || !valid_digest(&self.current_budget_digest)
            || self.current_budget_digest == self.execution_budget.budget_digest
            || self.state != "not_started"
            || self.reason != "budget_head_changed"
            || !positive(self.refused_at_ms)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedBudgetWorkRefusalRecord {
    pub refusal: SharedBudgetWorkRefusal,
    pub refusal_digest: String,
}

#[cfg(test)]
#[path = "management_shared_budget_type_tests.rs"]
mod tests;
