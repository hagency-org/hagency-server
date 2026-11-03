//! Direct resource-pool requests. No project, project administrator or setup
//! identity is accepted from the caller.
use crate::{Error, Result, key, management_agent_types::AgentDefinition};
use serde::{Deserialize, Serialize};

pub const POOL_AGENTS_FEATURE: &str = "resource-pool-agents-v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestPoolAgent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewal_mode: Option<crate::recurring_quota::RenewalMode>,
    pub command_id: String,
    pub room_id: String,
    pub expected_pool_revision: i64,
    pub expected_grant_revision: i64,
    pub definition: AgentDefinition,
    pub requested_tokens: i64,
}
impl RequestPoolAgent {
    pub(crate) fn validate(&self) -> Result<()> {
        key(&self.command_id)?;
        if !self.room_id.starts_with('!')
            || !self.room_id.contains(':')
            || self.room_id.len() > 255
            || self.room_id.chars().any(char::is_whitespace)
        {
            return Err(Error::Invalid("invalid_pool_agent_room"));
        }
        self.definition.validate()?;
        for n in [
            self.expected_pool_revision,
            self.expected_grant_revision,
            self.requested_tokens,
        ] {
            if !(1..=crate::management_allocation_types::MAX_EXACT).contains(&n) {
                return Err(Error::Invalid("invalid_pool_agent_request"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecidePoolAgent {
    pub command_id: String,
    pub expected_revision: i64,
    pub decision: crate::management_types::Decision,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoolAgentQuery {
    pub after: Option<String>,
    pub pool_id: Option<String>,
    #[serde(default)]
    pub needs_my_action: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcePool {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_request_tokens: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<crate::recurring_quota::Calendar>,
    pub id: String,
    pub label: String,
    pub revision: i64,
    pub owner_mxid: String,
    pub runtime: crate::management_types::Runtime,
    pub capabilities: std::collections::BTreeSet<String>,
    pub grant_revision: Option<i64>,
    pub published_tokens: Option<i64>,
    /// Only the original native ledger can confirm current free capacity.
    pub native_available_tokens: Option<i64>,
    pub can_request: bool,
    pub blocking_reason: Option<String>,
    /// End of the effective grant's UTC month.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_period_end_ms: Option<i64>,
    /// The effective grant's scheduled renewal for the next UTC month.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewal: Option<PoolRenewal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolRenewal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_request_tokens: Option<i64>,

    pub period_key: String,
    pub period_start_ms: i64,
    pub published_tokens: i64,
    pub grant_revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolAgentRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<crate::recurring_quota::Calendar>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewal_mode: Option<crate::recurring_quota::RenewalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_period: Option<PoolAgentPeriod>,
    #[serde(default)]
    pub can_renew: bool,
    #[serde(default)]
    pub can_reallocate: bool,
    #[serde(default)]
    pub rooms_requiring_reinvite: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_grant_revision: Option<i64>,
    pub id: String,
    pub pool_id: String,
    pub pool_label: String,
    pub resource_owner_mxid: String,
    pub manager_mxid: String,
    pub agent_id: String,
    pub room_id: String,
    pub definition: AgentDefinition,
    pub assignment_id: Option<String>,
    pub membership_state: Option<String>,
    pub runtime_state: Option<String>,
    pub reservation_state: Option<String>,
    pub reservation_reason: Option<String>,
    pub requested_tokens: i64,
    pub period_key: String,
    pub state: String,
    pub revision: i64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub can_decide: bool,
    pub can_cancel: bool,
    pub can_stop_supply: bool,
    pub blocking_reason: Option<String>,
    /// When a reserved Agent's authority ends: the end of its pool's capacity
    /// chain with chain lifetimes on, else its reserve expiry. Absent when the
    /// Agent is not reserved or no longer authorized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_until_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenewPoolAgent {
    pub command_id: String,
    pub expected_revision: i64,
    pub expected_period_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolAgentPeriod {
    pub key: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub tokens_per_period: i64,
    pub consumed_tokens: i64,
    pub remaining_tokens: Option<i64>,
    pub state: String,
    pub next_reset_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReallocatePoolAgent {
    pub command_id: String,
    pub expected_revision: i64,
    pub expected_grant_revision: i64,
    pub renewal_mode: crate::recurring_quota::RenewalMode,
}
