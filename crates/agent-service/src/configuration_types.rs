//! Strict non-secret wire contract for the execution device configuration queue.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_BYTES: usize = 65_536;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationOperation {
    Environment,
    AgentModel,
    SaveAgentModel,
    AgentPolicy,
    SaveAgentPolicy,
    PrepareWorkspace,
    ProviderStatus,
    ProviderModels,
    ProviderConnect,
}
impl ConfigurationOperation {
    pub fn mutation(&self) -> bool {
        matches!(
            self,
            Self::SaveAgentModel
                | Self::SaveAgentPolicy
                | Self::PrepareWorkspace
                | Self::ProviderConnect
        )
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationInput {
    pub idempotency_key: String,
    pub expected_generation: i64,
    pub operation: ConfigurationOperation,
    pub input: Value,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPoll {
    pub accept_work: bool,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationComplete {
    pub state: String,
    #[serde(deserialize_with = "required_nullable")]
    pub result: Option<Value>,
    #[serde(deserialize_with = "required_nullable")]
    pub error_code: Option<String>,
}
fn required_nullable<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationRequest {
    pub id: String,
    pub agent_id: String,
    pub agent_generation: i64,
    pub execution_device_id: String,
    pub device_generation: i64,
    pub operation: ConfigurationOperation,
    pub input: Value,
    pub state: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub claimed_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub result: Option<Value>,
    pub error_code: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationStatus {
    pub agent_id: String,
    pub agent_generation: i64,
    pub execution_device_id: Option<String>,
    pub device_generation: Option<i64>,
    pub online: bool,
    pub last_seen_at_ms: Option<i64>,
    pub observed_at_ms: Option<i64>,
    pub cached: bool,
    pub environment: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationModelProfile {
    pub model: String,
    pub credential_ref: String,
    pub workspace_root: String,
    pub reasoning_effort: String,
    #[serde(default = "default_web_search")]
    pub web_search: bool,
}
fn default_web_search() -> bool {
    true
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPolicy {
    pub budget: ConfigurationBudget,
    pub requests: RequestPolicy,
    pub high_risk: ToolPolicy,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationBudget {
    pub limit: ConfigurationLimit,
    pub period: ConfigurationPeriod,
}
#[derive(Debug, Serialize, Deserialize)]
pub enum ConfigurationLimit {
    Unset,
    Unlimited,
    Tokens(u64),
}
#[derive(Debug, Serialize, Deserialize)]
pub enum ConfigurationPeriod {
    Lifetime,
    UtcDay,
    UtcMonth,
}
#[derive(Debug, Serialize, Deserialize)]
pub enum RequestPolicy {
    Allow,
    Deny,
    AskOwner,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ToolPolicy {
    Deny,
    AskOwner,
    AllowWithRules {
        tools: Vec<String>,
        directories: Vec<String>,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPolicyVersion {
    pub revision: i64,
    pub policy: ConfigurationPolicy,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationUsage {
    pub spent: u64,
    pub held: u64,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConfiguration {
    pub agent_id: String,
    pub policy: ConfigurationPolicyVersion,
    pub usage: ConfigurationUsage,
    pub model_profile: Option<ConfigurationModelProfile>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationProvider {
    pub runtime: String,
    pub installed: bool,
    pub version: Option<String>,
    pub version_supported: bool,
    pub state: String,
    pub authenticated: bool,
    pub account_verified: bool,
    pub credential_ref: Option<String>,
    pub credential_source: String,
    pub shared: bool,
    pub strict_token_cap: bool,
    pub native_tools: bool,
    pub models: Option<Vec<ConfigurationProviderModel>>,
    pub default_model: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationProviderModel {
    pub id: String,
    pub model: String,
    pub display_name: String,
    pub description: String,
    pub is_default: bool,
    pub default_reasoning_effort: String,
    pub supported_reasoning_efforts: Vec<ConfigurationEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationEffort {
    pub reasoning_effort: String,
    pub description: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationEnvironment {
    pub providers: Vec<ConfigurationProvider>,
    pub configuration: AgentConfiguration,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelInput {
    profile: ConfigurationModelProfile,
    #[serde(
        default,
        rename = "expectedProfileDigest",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_profile_digest"
    )]
    expected_profile_digest: Option<String>,
}

fn present_profile_digest<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Omission preserves older requests. An explicitly null digest must not
    // silently turn a reviewed compare-and-set into an unconditional update.
    String::deserialize(deserializer).map(Some)
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyInput {
    expected_revision: i64,
    policy: ConfigurationPolicy,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderInput {
    runtime: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceResult {
    agent_id: String,
    workspace: String,
}

fn invalid() -> Error {
    Error::Invalid("invalid_configuration_payload")
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| invalid())
}
fn encode<T: Serialize>(v: T) -> Result<Value> {
    serde_json::to_value(v).map_err(|_| invalid())
}
fn text(v: &str, max: usize, empty: bool) -> Result<()> {
    if (!empty && v.is_empty()) || v.len() > max || v.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}
fn runtime(v: &str) -> Result<()> {
    if !["codex", "claude", "octos"].contains(&v) {
        return Err(invalid());
    }
    Ok(())
}
fn effort(v: &str) -> Result<()> {
    if ![
        "", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
    ]
    .contains(&v)
    {
        return Err(invalid());
    }
    Ok(())
}
fn reference(v: &str) -> Result<()> {
    text(v, 512, false)?;
    if ![
        "keychain:",
        "codex-managed:",
        "claude-managed:",
        "octos-managed:",
    ]
    .iter()
    .any(|p| v.starts_with(p))
        || v.contains("://")
    {
        return Err(invalid());
    }
    Ok(())
}
fn profile(v: &ConfigurationModelProfile, saved: bool) -> Result<()> {
    text(&v.model, 512, false)?;
    reference(&v.credential_ref)?;
    text(&v.workspace_root, 4096, !saved)?;
    effort(&v.reasoning_effort)
}
fn policy(v: &ConfigurationPolicy) -> Result<()> {
    if let ConfigurationLimit::Tokens(n) = v.budget.limit
        && n > i64::MAX as u64
    {
        return Err(invalid());
    }
    if let ToolPolicy::AllowWithRules { tools, directories } = &v.high_risk {
        if tools.len() > 100 || directories.len() > 100 {
            return Err(invalid());
        }
        for t in tools {
            text(t, 128, false)?;
        }
        for d in directories {
            text(d, 4096, false)?;
        }
    }
    Ok(())
}
fn agent(v: &AgentConfiguration, expected: &str) -> Result<()> {
    if v.agent_id != expected
        || v.policy.revision < 0
        || v.usage.spent > i64::MAX as u64
        || v.usage.held > i64::MAX as u64
    {
        return Err(invalid());
    }
    policy(&v.policy.policy)?;
    if let Some(p) = &v.model_profile {
        profile(p, true)?;
    }
    Ok(())
}
fn provider(v: &ConfigurationProvider) -> Result<()> {
    runtime(&v.runtime)?;
    if !["ready", "disconnected", "unavailable", "unsupported"].contains(&v.state.as_str()) {
        return Err(invalid());
    }
    if let Some(t) = &v.version {
        text(t, 128, false)?;
    }
    if let Some(t) = &v.credential_ref {
        reference(t)?;
    }
    text(&v.credential_source, 128, true)?;
    if let Some(t) = &v.default_model {
        text(t, 512, false)?;
    }
    if let Some(models) = &v.models {
        if models.len() > 100 {
            return Err(invalid());
        }
        for m in models {
            text(&m.id, 512, false)?;
            text(&m.model, 512, false)?;
            text(&m.display_name, 512, true)?;
            text(&m.description, 2048, true)?;
            effort(&m.default_reasoning_effort)?;
            if let Some(t) = &m.credential_ref {
                reference(t)?;
            }
            if m.supported_reasoning_efforts.len() > 9 {
                return Err(invalid());
            }
            for e in &m.supported_reasoning_efforts {
                effort(&e.reasoning_effort)?;
                text(&e.description, 2048, true)?;
            }
        }
    }
    Ok(())
}
pub fn validate_input(op: &ConfigurationOperation, value: Value) -> Result<Value> {
    if serde_json::to_vec(&value).map_err(|_| invalid())?.len() > MAX_BYTES {
        return Err(invalid());
    }
    match op {
        ConfigurationOperation::SaveAgentModel => {
            let v: ModelInput = decode(value)?;
            profile(&v.profile, false)?;
            if let Some(digest) = &v.expected_profile_digest
                && (digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
            {
                return Err(invalid());
            }
            encode(v)
        }
        ConfigurationOperation::SaveAgentPolicy => {
            let v: PolicyInput = decode(value)?;
            if v.expected_revision < 0 {
                return Err(invalid());
            }
            policy(&v.policy)?;
            encode(v)
        }
        ConfigurationOperation::ProviderStatus
        | ConfigurationOperation::ProviderModels
        | ConfigurationOperation::ProviderConnect => {
            let v: ProviderInput = decode(value)?;
            runtime(&v.runtime)?;
            encode(v)
        }
        _ => encode(decode::<Empty>(value)?),
    }
}
pub fn validate_result(
    op: &ConfigurationOperation,
    input: &Value,
    expected_agent: &str,
    value: Value,
) -> Result<Value> {
    if serde_json::to_vec(&value).map_err(|_| invalid())?.len() > MAX_BYTES {
        return Err(invalid());
    }
    match op {
        ConfigurationOperation::Environment => {
            let v: ConfigurationEnvironment = decode(value)?;
            if v.providers.len() > 3 {
                return Err(invalid());
            }
            let mut seen = std::collections::BTreeSet::new();
            for p in &v.providers {
                provider(p)?;
                if !seen.insert(&p.runtime) {
                    return Err(invalid());
                }
            }
            agent(&v.configuration, expected_agent)?;
            encode(v)
        }
        ConfigurationOperation::ProviderStatus
        | ConfigurationOperation::ProviderModels
        | ConfigurationOperation::ProviderConnect => {
            let v: ConfigurationProvider = decode(value)?;
            provider(&v)?;
            if input["runtime"] != v.runtime {
                return Err(invalid());
            }
            encode(v)
        }
        ConfigurationOperation::PrepareWorkspace => {
            let v: WorkspaceResult = decode(value)?;
            if v.agent_id != expected_agent {
                return Err(invalid());
            }
            text(&v.workspace, 4096, false)?;
            encode(v)
        }
        _ => {
            let v: AgentConfiguration = decode(value)?;
            agent(&v, expected_agent)?;
            encode(v)
        }
    }
}
pub fn failure_code(code: &str) -> bool {
    // Only stable application codes, never upstream messages or shell output.
    matches!(
        code,
        "invalid_arguments"
            | "invalid_local_policy"
            | "invalid_configuration_payload"
            | "configuration_runtime_busy"
            | "configuration_request_expired"
            | "configuration_target_changed"
            | "configuration_operation_unsupported"
            | "configuration_environment_unavailable"
            | "configuration_model_unavailable"
            | "configuration_provider_unavailable"
            | "configuration_validation_failed"
            | "local_state_unavailable"
            | "local_policy_conflict"
            | "owner_scope_required"
            | "execution_device_required"
            | "agent_execution_device_required"
            | "owner_authorization_required"
            | "sign_in_required"
            | "provider_directory_unavailable"
            | "provider_choice_unavailable"
            | "provider_unavailable"
            | "provider_timeout"
            | "codex_binary_unavailable"
            | "claude_binary_unavailable"
            | "octos_binary_unavailable"
            | "claude_account_unavailable"
            | "octos_profiles_unavailable"
            | "local_provider_not_signed_in"
            | "local_codex_account_not_signed_in"
            | "local_provider_login_managed_locally"
            | "runtime_profile_unavailable"
            | "runtime_active"
            | "policy_conflict"
            | "local_policy_revision_conflict"
            | "local_revision_conflict"
            | "configuration_local_state_unavailable"
            | "configuration_authority_changed"
    )
}

#[cfg(test)]
mod web_search_tests {
    #[test]
    fn configuration_web_search_defaults_on_and_keeps_explicit_off() {
        let mut value = serde_json::json!({"model":"m","credential_ref":"ref","workspace_root":"/work","reasoning_effort":"low"});
        let profile: super::ConfigurationModelProfile =
            serde_json::from_value(value.clone()).unwrap();
        assert!(profile.web_search);
        value["web_search"] = false.into();
        let profile: super::ConfigurationModelProfile =
            serde_json::from_value(value.clone()).unwrap();
        assert!(!profile.web_search);
        assert_eq!(serde_json::to_value(profile).unwrap(), value);
        value["web_search"] = "false".into();
        assert!(serde_json::from_value::<super::ConfigurationModelProfile>(value).is_err());
    }
}
