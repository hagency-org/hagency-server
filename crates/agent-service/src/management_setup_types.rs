//! Owner-created Matrix setup. Incoming room IDs are candidates, never proof.
use serde::{Deserialize, Serialize};

pub const PROJECT_SETUP_FEATURE: &str = "project-setup-v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignProjectAdmin {
    pub command_id: String,
    pub expected_authority_revision: i64,
    pub administrator_mxid: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BeginProjectSetup {
    pub command_id: String,
    /// Omitted only when replaying an exact pre-binding command receipt. New
    /// setup requires the selected allocation identity, not just its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allocation_id: Option<String>,
    pub expected_project_revision: i64,
    pub expected_allocation_revision: i64,
    pub expected_authority_revision: i64,
    pub mode: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmProjectSetup {
    pub command_id: String,
    pub expected_revision: i64,
    pub space_id: String,
    pub room_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelProjectSetup {
    pub command_id: String,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RebaseProjectSetup {
    pub command_id: String,
    pub expected_revision: i64,
    pub allocation_id: String,
    pub expected_project_revision: i64,
    pub expected_allocation_revision: i64,
    pub expected_authority_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupBinding {
    pub revision: i64,
    pub allocation_id: String,
    pub allocation_receipt_digest: String,
    pub project_revision: i64,
    pub authority_revision: i64,
    pub administrator_mxid: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAdministrator {
    pub mxid: String,
    pub revision: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EligibleSetupAllocation {
    pub id: String,
    pub revision: i64,
    pub project_revision: i64,
    pub receipt_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSetup {
    pub id: String,
    pub project_id: String,
    pub allocation_id: String,
    pub allocation_receipt_digest: String,
    pub project_revision: i64,
    pub authority_revision: i64,
    pub definition_digest: String,
    pub mode: String,
    pub state: String,
    pub revision: i64,
    pub name: String,
    pub manager_mxid: String,
    pub administrator_mxid: String,
    pub space_creation_command_id: String,
    pub room_creation_command_id: String,
    pub space_id: Option<String>,
    pub room_id: Option<String>,
    pub domain_project_id: Option<String>,
    pub verified_at_ms: Option<i64>,
    /// Separate from the immutable original creation definition above. Old
    /// command receipts omit this field; current projections always include it.
    #[serde(default)]
    pub current_binding: Option<SetupBinding>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReadiness {
    pub project_id: String,
    pub administrator: Option<ProjectAdministrator>,
    pub setup: Option<ProjectSetup>,
    /// Exact current candidate for begin/rebase, independently of the setup's
    /// pinned original/current binding. A read projection grants no execution.
    #[serde(default)]
    pub eligible_allocation: Option<EligibleSetupAllocation>,
    pub capacity_ready: bool,
    /// Last verified room setup, never a claim of currently observed membership.
    pub matrix_ready: bool,
    pub can_assign_administrator: bool,
    pub can_begin: bool,
    pub can_confirm: bool,
    #[serde(default)]
    pub can_cancel: bool,
    #[serde(default)]
    pub can_rebase: bool,
    pub blocking_reason: Option<String>,
    pub observed_at_ms: i64,
}
