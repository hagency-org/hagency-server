//! Disabled managed-agent admission contract. A decision is not a native hold.
use crate::{
    Error, Result, key,
    management_allocation_types::{MAX_EXACT, allocation_digest, valid_digest},
};
use serde::{Deserialize, Serialize};

pub const AGENT_CAPABILITY: &str = "hagency.managed_agents.v1";
/// Human admission discovery only, independent of native execution protocols.
pub const AGENT_ADMISSION_FEATURE: &str = "agent-admission-v1";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRequestQuery {
    pub after: Option<String>,
    #[serde(default)]
    pub needs_my_action: bool,
    pub project_id: Option<String>,
    /// Compatibility with the former shared query DTO; never widens visibility.
    #[serde(default)]
    pub include_projects: bool,
    #[serde(default)]
    pub include_allocations: bool,
}
impl AgentRequestQuery {
    pub(crate) fn validate(&self) -> Result<()> {
        for value in [&self.after, &self.project_id].into_iter().flatten() {
            key(value)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRequestContext {
    pub setup_id: String,
    pub setup_revision: i64,
    pub setup_binding_revision: i64,
    pub authority_revision: i64,
    pub allocation_id: String,
    pub allocation_revision: i64,
    pub allocation_receipt_digest: String,
    /// Original confirmed project reservation; NOT current free native tokens.
    pub requested_token_limit: i64,
    pub native_available_tokens: Option<i64>,
    pub period_key: String,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    pub capabilities: Vec<String>,
    pub tool_modes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRequestReadiness {
    pub protocol: String,
    pub observed_at_ms: i64,
    pub project_id: String,
    pub project_name: String,
    pub manager_mxid: String,
    pub administrator_mxid: Option<String>,
    pub can_view_requests: bool,
    pub can_request: bool,
    pub blocking_reason: Option<String>,
    pub request_context: Option<AgentRequestContext>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDefinition {
    pub name: String,
    pub capability: String,
    pub description: String,
    pub tool_mode: String,
}
impl AgentDefinition {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.name.len() > 160
            || self.name.chars().any(char::is_control)
            || self.description.len() > 2000
            || self
                .description
                .chars()
                .any(|c| c.is_control() && c != '\n')
            || self.capability.is_empty()
            || self.capability.len() > 80
            || !self
                .capability
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-/".contains(&b))
            || self.tool_mode != "none"
        {
            return Err(Error::Invalid("invalid_managed_agent_definition"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestManagedAgent {
    pub command_id: String,
    pub expected_setup_revision: i64,
    pub expected_setup_binding_revision: i64,
    pub expected_authority_revision: i64,
    pub expected_allocation_revision: i64,
    pub definition: AgentDefinition,
    pub requested_tokens: i64,
}
impl RequestManagedAgent {
    pub(crate) fn validate(&self) -> Result<()> {
        key(&self.command_id)?;
        self.definition.validate()?;
        for n in [
            self.expected_setup_revision,
            self.expected_setup_binding_revision,
            self.expected_authority_revision,
            self.expected_allocation_revision,
            self.requested_tokens,
        ] {
            positive(n)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecideManagedAgent {
    pub command_id: String,
    pub expected_revision: i64,
    pub expected_authority_revision: i64,
    pub decision: crate::management_types::Decision,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAgentRequest {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub project_name: Option<String>,
    pub proposed_agent_id: String,
    pub manager_mxid: String,
    pub administrator_mxid: String,
    pub authority_revision: i64,
    pub definition: AgentDefinition,
    pub definition_digest: String,
    pub requested_tokens: i64,
    pub period_key: String,
    pub state: String,
    pub revision: i64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub can_decide: bool,
    pub can_approve: bool,
    pub can_reject: bool,
    pub can_cancel: bool,
    pub blocking_reason: Option<String>,
    /// Always false in admission-only checkpoint. No executable delivery route.
    pub execution_available: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentReserveCommand {
    pub version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_grant: Option<crate::management_allocation_types::NativeGrantReceipt>,
    pub capability: String,
    pub operation: String,
    pub command_id: String,
    pub request_id: String,
    pub request_revision: i64,
    pub proposed_agent_id: String,
    pub manager_mxid: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    #[serde(default, skip_serializing_if = "zero_revision")]
    pub project_revision: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub setup_id: String,
    #[serde(default, skip_serializing_if = "zero_revision")]
    pub setup_binding_revision: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain_project_id: String,
    pub room_id: String,
    pub authority_revision: i64,
    pub decision_actor_mxid: String,
    pub decision_digest: String,
    pub allocation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub allocation_receipt_digest: String,
    pub grant_receipt_digest: String,
    pub enrollment_id: String,
    pub executor_device_id: String,
    pub executor_generation: i64,
    pub resource_id: String,
    pub resource_revision: String,
    pub definition: AgentDefinition,
    pub definition_digest: String,
    pub requested_tokens: i64,
    pub period_key: String,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}
fn zero_revision(value: &i64) -> bool {
    *value == 0
}
impl ManagedAgentReserveCommand {
    pub fn validate(&self, server: &str) -> Result<()> {
        for value in [
            &self.command_id,
            &self.request_id,
            &self.proposed_agent_id,
            &self.allocation_id,
            &self.enrollment_id,
            &self.executor_device_id,
            &self.resource_id,
        ] {
            key(value)?;
        }
        for value in [
            self.request_revision,
            self.authority_revision,
            self.executor_generation,
            self.requested_tokens,
            self.issued_at_ms,
            self.expires_at_ms,
        ] {
            positive(value)?;
        }
        for value in [&self.manager_mxid, &self.decision_actor_mxid] {
            crate::management_types::local_user(value, server)?;
        }
        self.definition.validate()?;
        match (self.version, &self.pool_grant) {
            (1, None) => {
                if self.capability != AGENT_CAPABILITY {
                    return Err(Error::Invalid("invalid_agent_command"));
                }
                for value in [&self.project_id, &self.setup_id, &self.domain_project_id] {
                    key(value)?;
                }
                for value in [self.project_revision, self.setup_binding_revision] {
                    positive(value)?;
                }
                if !valid_digest(&self.allocation_receipt_digest) || self.room_id.is_empty() {
                    return Err(Error::Invalid("invalid_agent_command"));
                }
            }
            (2, Some(grant)) => {
                grant.validate(server)?;
                if self.room_id.is_empty() || self.capability != "hagency.pool_agents.v1"
                    || !self.project_id.is_empty()
                    || !self.setup_id.is_empty()
                    || !self.domain_project_id.is_empty()
                    || !self.allocation_receipt_digest.is_empty()
                    || self.project_revision != 0
                    || self.setup_binding_revision != 0
                    || !matches!(grant.version, 2 | 3)
                    || grant.admission_request_id.is_none()
                    || grant.owner_mxid != grant.coordinator_mxid
                    || self.decision_actor_mxid != grant.owner_mxid
                    || !grant.eligible_managers.contains(&self.manager_mxid)
                    || self.allocation_id != self.request_id
                    || self.enrollment_id != grant.enrollment_id
                    || self.executor_device_id != grant.executor_device_id
                    || self.executor_generation != grant.executor_generation
                    || self.resource_id != grant.resource_id
                    || self.resource_revision != grant.resource_revision
                    || self.authority_revision != grant.grant_revision
                    || self.period_key != grant.period_key
                    || self.requested_tokens > grant.allocated_tokens
                    || self.issued_at_ms < grant.period_start_ms
                    || self.expires_at_ms > grant.period_end_ms.min(grant.delegation_expires_at_ms)
                    || allocation_digest(grant)? != self.grant_receipt_digest
                {
                    return Err(Error::Invalid("invalid_pool_agent_authority"));
                }
            }
            _ => return Err(Error::Invalid("invalid_agent_command")),
        }
        if self.operation != "agent.reserve"
            || self.expires_at_ms <= self.issued_at_ms
            || !period(&self.period_key)
            || (!self.room_id.is_empty()
                && (!self.room_id.starts_with('!')
                    || self.room_id.len() > 255
                    || !self.room_id.contains(':')
                    || self
                        .room_id
                        .chars()
                        .any(|c| c.is_control() || c.is_whitespace())))
            || [
                &self.definition_digest,
                &self.decision_digest,
                &self.grant_receipt_digest,
                &self.resource_revision,
            ]
            .iter()
            .any(|value| !valid_digest(value))
            || allocation_digest(&self.definition)? != self.definition_digest
            || serde_json::to_vec(self)
                .map_err(|_| Error::Invalid("invalid_agent_command"))?
                .len()
                > crate::management_types::MAX_BYTES
        {
            return Err(Error::Invalid("invalid_agent_command"));
        }
        Ok(())
    }
}

/// Future native adapter wire fixture; this checkpoint accepts no such result.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAgentReservationReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub request_id: String,
    pub agent_id: String,
    pub allocation_id: String,
    pub agent_reservation_id: Option<String>,
    pub definition_digest: String,
    pub state: String,
    pub budget_revision: i64,
    pub ledger_revision: i64,
    pub allocated_tokens: i64,
    pub retained_tokens: i64,
    pub period_key: String,
    pub reason: Option<String>,
}
impl ManagedAgentReservationReceipt {
    /// Validation only; no server route currently accepts or applies this DTO.
    pub fn validate_for(&self, command: &ManagedAgentReserveCommand) -> Result<()> {
        positive(self.ledger_revision)?;
        for n in [
            self.budget_revision,
            self.allocated_tokens,
            self.retained_tokens,
        ] {
            if !(0..=MAX_EXACT).contains(&n) {
                return Err(Error::Invalid("invalid_agent_receipt"));
            }
        }
        if let Some(id) = &self.agent_reservation_id {
            key(id)?;
        }
        let reserved = self.state == "reserved"
            && self.agent_reservation_id.is_some()
            && self.budget_revision > 0
            && self.allocated_tokens == command.requested_tokens
            && self.retained_tokens == command.requested_tokens
            && self.reason.is_none();
        let refused = self.state == "refused"
            && self.agent_reservation_id.is_none()
            && self.budget_revision == 0
            && self.allocated_tokens == 0
            && self.retained_tokens == 0
            && self.reason.as_ref().is_some_and(|r| {
                !r.is_empty()
                    && r.len() <= 80
                    && r.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            });
        if self.version != 1
            || self.command_id != command.command_id
            || self.command_digest != allocation_digest(command)?
            || self.request_id != command.request_id
            || self.agent_id != command.proposed_agent_id
            || self.allocation_id != command.allocation_id
            || self.definition_digest != command.definition_digest
            || self.period_key != command.period_key
            || !(reserved || refused)
        {
            return Err(Error::Invalid("invalid_agent_receipt"));
        }
        Ok(())
    }
}
fn positive(n: i64) -> Result<()> {
    if (1..=MAX_EXACT).contains(&n) {
        Ok(())
    } else {
        Err(Error::Invalid("invalid_agent_number"))
    }
}
fn period(p: &str) -> bool {
    p.len() == 7
        && p.as_bytes()[4] == b'-'
        && p.bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || b.is_ascii_digit())
        && ("01"..="12").contains(&&p[5..])
}

#[cfg(test)]
mod pool_contract_tests {
    use super::*;
    #[test]
    fn direct_pool_agent_command_has_owner_approval_without_project_authority() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/pool-agent-v1.json")).unwrap();
        let command: ManagedAgentReserveCommand =
            serde_json::from_value(fixture["command"].clone()).unwrap();
        command.validate("example.test").unwrap();
        assert_eq!(
            allocation_digest(&command).unwrap(),
            fixture["commandDigest"]
        );
        assert!(
            serde_json::to_value(&command)
                .unwrap()
                .get("projectId")
                .is_none()
        );
        for field in [
            "decisionActorMxid",
            "managerMxid",
            "resourceId",
            "allocationId",
            "projectId",
        ] {
            let mut invalid = fixture["command"].clone();
            invalid[field] = serde_json::json!(if field.ends_with("Mxid") {
                "@unapproved:example.test"
            } else {
                "different"
            });
            assert!(
                serde_json::from_value::<ManagedAgentReserveCommand>(invalid)
                    .unwrap()
                    .validate("example.test")
                    .is_err(),
                "{field}"
            );
        }
        let mut invalid = command;
        invalid.version = 1;
        assert!(invalid.validate("example.test").is_err());
    }
}
