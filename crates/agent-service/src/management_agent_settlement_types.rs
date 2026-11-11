//! Fresh observation after an immutable fence. This cannot issue work, increase
//! a budget, release a project/grant or rewrite the original cleanup receipt.
use crate::{
    Error, Result, key,
    management_agent_executor_types::ManagedAgentFenceCommand,
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
};
use serde::{Deserialize, Serialize};
fn exact(n: i64) -> Result<()> {
    if !(1..=MAX_EXACT).contains(&n) {
        return Err(Error::Invalid("invalid_agent_settlement_integer"));
    }
    Ok(())
}
fn hash(value: &str) -> Result<()> {
    if !valid_digest(value) {
        return Err(Error::Invalid("invalid_agent_settlement_digest"));
    }
    Ok(())
}

pub const AGENT_SETTLEMENT_CAPABILITY: &str = "hagency.managed_agents.settlement.v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentSettlementCommand {
    pub version: u16,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub fence_command: ManagedAgentFenceCommand,
    pub fence_receipt_digest: String,
    pub issued_at_ms: i64,
}
impl ManagedAgentSettlementCommand {
    pub fn validate(&self, server: &str) -> Result<()> {
        key(&self.command_id)?;
        hash(&self.fence_receipt_digest)?;
        exact(self.issued_at_ms)?;
        self.fence_command.validate(server)?;
        if self.version != 1
            || self.capability != AGENT_SETTLEMENT_CAPABILITY
            || self.operation != "agent.settle"
            || self.command_id == self.fence_command.command_id
            || self.command_id == self.fence_command.reserve_command.command_id
            || self.issued_at_ms < self.fence_command.issued_at_ms
            || serde_json::to_vec(self)
                .map_err(|_| Error::Invalid("invalid_agent_settlement_command"))?
                .len()
                > 20 * 1024
        {
            return Err(Error::Invalid("invalid_agent_settlement_command"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentSettlementReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub fence_receipt_digest: String,
    pub reserve_command_digest: String,
    pub request_id: String,
    pub agent_id: String,
    pub agent_reservation_id: Option<String>,
    pub period_key: String,
    pub fence_revision: i64,
    pub ledger_revision: i64,
    pub state: String,
    pub process_state: String,
    pub known_consumed_tokens: i64,
    pub retained_tokens: i64,
    pub unresolved_tokens: i64,
    pub usage_complete: bool,
    pub budget_revision: Option<i64>,
    pub budget_digest: Option<String>,
    pub effective_ceiling_tokens: Option<i64>,
    pub liability_digest: String,
}
impl ManagedAgentSettlementReceipt {
    pub fn validate_for(&self, c: &ManagedAgentSettlementCommand) -> Result<()> {
        let original = &c.fence_command.reserve_command;
        for n in [self.fence_revision, self.ledger_revision] {
            exact(n)?;
        }
        for n in [
            self.known_consumed_tokens,
            self.retained_tokens,
            self.unresolved_tokens,
        ] {
            if !(0..=MAX_EXACT).contains(&n) {
                return Err(Error::Invalid("invalid_agent_settlement_amount"));
            }
        }
        hash(&self.liability_digest)?;
        let budget_valid = match (
            &self.agent_reservation_id,
            self.budget_revision,
            &self.budget_digest,
            self.effective_ceiling_tokens,
        ) {
            (Some(child), Some(revision), Some(digest), Some(ceiling)) => {
                key(child)?;
                exact(revision)?;
                hash(digest)?;
                exact(ceiling)?;
                ceiling >= original.requested_tokens
            }
            (None, None, None, None) => {
                self.usage_complete
                    && self.retained_tokens == 0
                    && self.process_state == "not_started"
            }
            _ => false,
        };
        if self.version != 1
            || !budget_valid
            || self.command_id != c.command_id
            || self.command_digest != allocation_digest(c)?
            || self.fence_receipt_digest != c.fence_receipt_digest
            || self.reserve_command_digest != c.fence_command.reserve_command_digest
            || self.request_id != original.request_id
            || self.agent_id != original.proposed_agent_id
            || self.period_key != original.period_key
            || self.ledger_revision <= self.fence_revision
            || self.retained_tokens < self.known_consumed_tokens
            || self.unresolved_tokens != self.retained_tokens - self.known_consumed_tokens
            || !matches!(
                self.process_state.as_str(),
                "unknown" | "stopped" | "not_started"
            )
            || (self.process_state == "not_started" && self.known_consumed_tokens != 0)
            || if self.usage_complete {
                self.state != "settled"
                    || self.unresolved_tokens != 0
                    || self.process_state == "unknown"
            } else {
                self.state != "unresolved"
            }
        {
            return Err(Error::Invalid("invalid_agent_settlement_receipt"));
        }
        Ok(())
    }
}

pub const AGENT_SETTLEMENT_FEATURE: &str = "agent-settlement-v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestAgentSettlement {
    pub command_id: String,
    pub expected_request_revision: i64,
    pub expected_fence_receipt_digest: String,
}
impl RequestAgentSettlement {
    pub fn validate(&self) -> Result<()> {
        key(&self.command_id)?;
        exact(self.expected_request_revision)?;
        hash(&self.expected_fence_receipt_digest)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSettlementRequested {
    pub request_id: String,
    pub command_id: String,
    pub command_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSettlementProgress {
    pub request_id: String,
    pub request_revision: i64,
    pub fence_receipt_digest: Option<String>,
    pub pending_command_id: Option<String>,
    pub latest_receipt: Option<ManagedAgentSettlementReceipt>,
    pub settlement_current: bool,
    pub can_request: bool,
    pub blocking_reason: Option<String>,
    pub known_consumed_tokens: Option<i64>,
    pub retained_tokens_floor: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSettlementDelivery {
    pub command: ManagedAgentSettlementCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub execution_allowed: bool,
    pub recorded: Option<ManagedAgentSettlementReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSettlementResult {
    pub command_digest: String,
    pub token: String,
    pub receipt: ManagedAgentSettlementReceipt,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settlement_wire_fixture_is_canonical_and_rejects_credit_or_unknown_fields() {
        let f: serde_json::Value =
            serde_json::from_str(include_str!("management-agent-settlement-v1.json")).unwrap();
        let c: ManagedAgentSettlementCommand =
            serde_json::from_value(f["command"].clone()).unwrap();
        c.validate("example.test").unwrap();
        let r: ManagedAgentSettlementReceipt =
            serde_json::from_value(f["receipt"].clone()).unwrap();
        r.validate_for(&c).unwrap();
        assert_eq!(allocation_digest(&c).unwrap(), f["commandDigest"]);
        assert_eq!(allocation_digest(&r).unwrap(), f["receiptDigest"]);
        let mut bad = f["command"].clone();
        bad["ownerApproved"] = true.into();
        assert!(serde_json::from_value::<ManagedAgentSettlementCommand>(bad).is_err());
        let mut bad = f["receipt"].clone();
        bad["remainingTokens"] = 600.into();
        assert!(serde_json::from_value::<ManagedAgentSettlementReceipt>(bad).is_err());
    }
    #[test]
    fn settlement_amounts_and_complete_proof_fail_closed() {
        let f: serde_json::Value =
            serde_json::from_str(include_str!("management-agent-settlement-v1.json")).unwrap();
        let c: ManagedAgentSettlementCommand =
            serde_json::from_value(f["command"].clone()).unwrap();
        let r: ManagedAgentSettlementReceipt =
            serde_json::from_value(f["receipt"].clone()).unwrap();
        for n in [-1, MAX_EXACT + 1] {
            let mut bad = r.clone();
            bad.retained_tokens = n;
            assert!(bad.validate_for(&c).is_err());
            let mut bad = r.clone();
            bad.known_consumed_tokens = n;
            assert!(bad.validate_for(&c).is_err());
            let mut bad = r.clone();
            bad.unresolved_tokens = n;
            assert!(bad.validate_for(&c).is_err());
        }
        let mut bad = r.clone();
        bad.process_state = "unknown".into();
        assert!(bad.validate_for(&c).is_err());
        let mut bad = r.clone();
        bad.retained_tokens = 399;
        assert!(bad.validate_for(&c).is_err());
        let mut bad = r.clone();
        bad.budget_digest = None;
        assert!(bad.validate_for(&c).is_err());
        let mut bad = r.clone();
        bad.budget_revision = Some(0);
        assert!(bad.validate_for(&c).is_err());
        let mut bad = r.clone();
        bad.period_key = "2026-11".into();
        assert!(bad.validate_for(&c).is_err());
        let mut bad = r.clone();
        bad.ledger_revision = bad.fence_revision;
        assert!(bad.validate_for(&c).is_err());
        let mut bad = r;
        bad.state = "credited".into();
        assert!(bad.validate_for(&c).is_err());
    }
}
