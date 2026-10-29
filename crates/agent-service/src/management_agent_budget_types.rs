//! Additive child-budget evidence. This phase cannot authorize increased-budget execution.
use crate::{
    Error, Result, key,
    management_agent_types::{ManagedAgentReservationReceipt, ManagedAgentReserveCommand},
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
    management_types::Decision,
};
use serde::{Deserialize, Serialize};
pub const BUDGET_CAPABILITY: &str = "hagency.managed_agents.budget.v1";
fn invalid() -> Error {
    Error::Invalid("invalid_agent_budget")
}
fn positive(n: i64) -> bool {
    (1..=MAX_EXACT).contains(&n)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAgentTopUp {
    /// Explicit owner-only, Agent-specific capacity outside the published pool.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub owner_funded: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approve_immediately: bool,
    pub command_id: String,
    pub expected_request_revision: i64,
    pub expected_assignment_generation: i64,
    pub expected_budget_revision: i64,
    pub expected_budget_digest: String,
    pub add_tokens: i64,
    pub purpose: String,
}
impl RequestAgentTopUp {
    pub fn validate(&self) -> Result<()> {
        key(&self.command_id)?;
        if ![
            self.expected_request_revision,
            self.expected_assignment_generation,
            self.expected_budget_revision,
            self.add_tokens,
        ]
        .into_iter()
        .all(positive)
            || (self.approve_immediately && !self.owner_funded)
            || !valid_digest(&self.expected_budget_digest)
            || self.purpose.trim().is_empty()
            || self.purpose.len() > 2048
            || self.purpose.chars().any(|c| c.is_control() && c != '\n')
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecideAgentTopUp {
    pub command_id: String,
    pub expected_revision: i64,
    pub expected_authority_revision: i64,
    pub decision: Decision,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudget {
    #[serde(default)]
    pub can_owner_fund: bool,
    #[serde(default)]
    pub can_owner_fund_direct: bool,
    pub request_id: String,
    pub request_revision: i64,
    pub assignment_generation: i64,
    pub assignment_id: String,
    pub agent_reservation_id: String,
    pub original_ceiling_tokens: i64,
    pub applied_delta_tokens: i64,
    pub effective_ceiling_tokens: i64,
    pub budget_revision: i64,
    pub budget_digest: String,
    pub known_consumed_tokens: i64,
    pub conservative_commitment_tokens: i64,
    pub native_retained_tokens: Option<i64>,
    pub native_remaining_tokens: Option<i64>,
    pub usage_complete: Option<bool>,
    pub execution_supported: bool,
    pub can_request: bool,
    pub blocking_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTopUp {
    #[serde(default)]
    pub owner_funded: bool,
    pub id: String,
    pub authority_revision: i64,
    pub request_id: String,
    pub assignment_id: String,
    pub add_tokens: i64,
    pub purpose: String,
    pub expected_budget_revision: i64,
    pub expected_budget_digest: String,
    pub expected_ceiling_tokens: i64,
    pub revision: i64,
    pub state: String,
    pub native_state: String,
    pub executor_command_id: Option<String>,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub can_decide: bool,
    pub can_cancel: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTopUpAck {
    pub command_id: String,
    pub operation: String,
    pub top_up_id: String,
    pub applied_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTopUpReply {
    pub receipt: AgentTopUpAck,
    pub top_up: AgentTopUp,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudgetCommand {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub owner_funded: bool,
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub top_up_id: String,
    pub top_up_request_digest: String,
    pub approval_command_id: String,
    pub approval_digest: String,
    pub approving_administrator_mxid: String,
    pub authority_revision: i64,
    pub assignment_id: String,
    pub assignment_generation: i64,
    pub reserve_command: ManagedAgentReserveCommand,
    pub reserve_command_digest: String,
    pub reserve_receipt: ManagedAgentReservationReceipt,
    pub reserve_receipt_digest: String,
    pub expected_budget_revision: i64,
    pub expected_budget_digest: String,
    pub expected_ceiling_tokens: i64,
    pub add_tokens: i64,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}
impl AgentBudgetCommand {
    pub fn validate(&self, server: &str) -> Result<()> {
        for id in [
            &self.command_id,
            &self.top_up_id,
            &self.assignment_id,
            &self.approval_command_id,
        ] {
            key(id)?
        }
        self.reserve_command.validate(server)?;
        self.reserve_receipt.validate_for(&self.reserve_command)?;
        crate::management_types::local_user(&self.approving_administrator_mxid, server)?;
        if self.version != if self.owner_funded { 2 } else { 1 }
            || self.owner_funded && self.reserve_command.pool_grant.is_none()
            || self.capability != BUDGET_CAPABILITY
            || self.operation != "agent.budget.increase"
            || self.reserve_receipt.state != "reserved"
            || self.reserve_command_digest != allocation_digest(&self.reserve_command)?
            || self.reserve_receipt_digest != allocation_digest(&self.reserve_receipt)?
            || self.reserve_command.pool_grant.as_ref().is_some_and(|g|
                self.approving_administrator_mxid != g.owner_mxid
                    || self.authority_revision != g.grant_revision)
            || ![
                self.authority_revision,
                self.assignment_generation,
                self.expected_budget_revision,
                self.expected_ceiling_tokens,
                self.add_tokens,
                self.issued_at_ms,
                self.expires_at_ms,
            ]
            .into_iter()
            .all(positive)
            || self.assignment_generation != 1
            || self.expected_budget_revision >= MAX_EXACT
            || self.expected_ceiling_tokens < self.reserve_command.requested_tokens
            || self
                .expected_ceiling_tokens
                .checked_add(self.add_tokens)
                .is_none_or(|v| v > MAX_EXACT)
            || self.expires_at_ms > self.reserve_command.expires_at_ms
            || self.expires_at_ms <= self.issued_at_ms
            || [
                &self.top_up_request_digest,
                &self.approval_digest,
                &self.expected_budget_digest,
            ]
            .into_iter()
            .any(|d| !valid_digest(d))
            || serde_json::to_vec(self).map_or(true, |v| v.len() > 16384)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudgetReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub top_up_id: String,
    pub agent_reservation_id: String,
    pub assignment_id: String,
    pub original_reserve_receipt_digest: String,
    pub state: String,
    pub expected_budget_revision: i64,
    pub previous_budget_digest: String,
    pub budget_revision: i64,
    pub budget_digest: String,
    pub previous_ceiling_tokens: i64,
    pub applied_delta_tokens: i64,
    pub effective_ceiling_tokens: i64,
    pub ledger_revision: i64,
    pub period_key: String,
    pub reason: Option<String>,
}
/// No receipt-digest cycle: chain head binds previous head + exact command + effect revision.
pub fn next_budget_digest(c: &AgentBudgetCommand, ledger_revision: i64) -> Result<String> {
    let revision = c
        .expected_budget_revision
        .checked_add(1)
        .filter(|v| positive(*v))
        .ok_or_else(invalid)?;
    let ceiling = c
        .expected_ceiling_tokens
        .checked_add(c.add_tokens)
        .filter(|v| positive(*v))
        .ok_or_else(invalid)?;
    if !positive(ledger_revision) {
        return Err(invalid());
    }
    allocation_digest(
        &serde_json::json!({"version":1,"previousBudgetDigest":c.expected_budget_digest,"commandDigest":allocation_digest(c)?,"budgetRevision":revision,"effectiveCeilingTokens":ceiling,"ledgerRevision":ledger_revision}),
    )
}

impl AgentBudgetReceipt {
    pub fn validate_for(&self, c: &AgentBudgetCommand) -> Result<()> {
        let applied = self.state == "applied";
        let revision = c
            .expected_budget_revision
            .checked_add(i64::from(applied))
            .filter(|v| positive(*v))
            .ok_or_else(invalid)?;
        let ceiling = c
            .expected_ceiling_tokens
            .checked_add(if applied { c.add_tokens } else { 0 })
            .filter(|v| positive(*v))
            .ok_or_else(invalid)?;
        if self.version != 1
            || self.command_id != c.command_id
            || self.command_digest != allocation_digest(c)?
            || self.top_up_id != c.top_up_id
            || Some(&self.agent_reservation_id) != c.reserve_receipt.agent_reservation_id.as_ref()
            || self.assignment_id != c.assignment_id
            || self.original_reserve_receipt_digest != c.reserve_receipt_digest
            || self.expected_budget_revision != c.expected_budget_revision
            || self.previous_budget_digest != c.expected_budget_digest
            || self.previous_ceiling_tokens != c.expected_ceiling_tokens
            || self.period_key != c.reserve_command.period_key
            || !positive(self.ledger_revision)
            || self.ledger_revision <= c.reserve_receipt.ledger_revision
            || !matches!(
                self.state.as_str(),
                "applied" | "refused" | "cancelled_before_apply"
            )
            || self.budget_revision != revision
            || self.applied_delta_tokens != if applied { c.add_tokens } else { 0 }
            || self.effective_ceiling_tokens != ceiling
            || self.budget_digest
                != if applied {
                    next_budget_digest(c, self.ledger_revision)?
                } else {
                    c.expected_budget_digest.clone()
                }
            || applied && self.reason.is_some()
            || !applied
                && self
                    .reason
                    .as_ref()
                    .is_none_or(|s| s.is_empty() || s.len() > 80)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudgetDelivery {
    pub command: AgentBudgetCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub application_allowed: bool,
    pub cancellation_requested: bool,
    pub recorded: Option<AgentBudgetReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudgetResult {
    pub command_digest: String,
    pub token: String,
    pub receipt: AgentBudgetReceipt,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTopUpQuery {
    pub after: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTopUpPage {
    pub items: Vec<AgentTopUp>,
    pub next_after: Option<String>,
}
