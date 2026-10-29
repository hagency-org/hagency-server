//! Separate parent cleanup protocol. No receipt grants execution authority.
use crate::management_allocation_types::ExecutorRecoverInput;
use serde::{Deserialize, Serialize};
pub const PARENT_CAPABILITY: &str = "hagency.parent_settlement.v1";
pub type ParentRecoverInput = ExecutorRecoverInput;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParentCloseInput {
    pub command_id: String,
    pub expected_grant_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantClosureCommand {
    pub version: u16,
    pub command_id: String,
    pub operation: String,
    pub grant_id: String,
    pub grant_receipt_digest: String,
    pub previous_settlement_digest: Option<String>,
    pub reason: String,
    pub requested_at_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeGrantSettlementReceipt {
    pub version: u16,
    pub command_id: String,
    pub command_digest: String,
    pub grant_id: String,
    pub grant_receipt_digest: String,
    pub enrollment_id: String,
    pub period_key: String,
    pub fence_revision: i64,
    pub ledger_revision: i64,
    pub state: String,
    pub retained_tokens: i64,
    pub known_consumed_tokens: i64,
    pub unresolved_tokens: i64,
    pub liability_count: i64,
    pub liability_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentSettlement {
    pub grant_id: String,
    pub grant_revision: i64,
    pub revision: i64,
    pub state: String,
    pub original_granted_tokens: i64,
    pub held_tokens: i64,
    pub known_consumed_tokens: Option<i64>,
    pub unresolved_tokens: Option<i64>,
    pub known_unused_released_tokens: i64,
    pub overrun_tokens: i64,
    pub unreconciled_child_tokens: i64,
    pub can_close: bool,
    pub can_settle: bool,
    pub receipt: Option<NativeGrantSettlementReceipt>,
    pub receipt_digest: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParentResultInput {
    pub command_digest: String,
    pub token: String,
    pub receipt: NativeGrantSettlementReceipt,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentDelivery {
    pub command: GrantClosureCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub execution_allowed: bool,
    pub recorded: Option<NativeGrantSettlementReceipt>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentAcknowledgement {
    pub command_id: String,
    pub command_digest: String,
    pub receipt_digest: String,
    pub settlement: ParentSettlement,
}
