//! Direct resource-pool requests. No project, project administrator or setup
//! identity is accepted from the caller.
use crate::{Error, Result, key, management_agent_types::AgentDefinition};
use serde::{Deserialize, Serialize};

pub const POOL_AGENTS_FEATURE: &str = "resource-pool-agents-v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestPoolAgent {
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolAgentRequest {
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
}
