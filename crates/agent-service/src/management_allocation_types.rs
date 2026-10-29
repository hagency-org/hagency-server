//! Closed wire contract for native-ledger project reservations. These reports
//! are accepted only from the enrolled owner device, never from Splash authority.
use crate::{Error, Result, hash, key};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const ALLOCATION_CAPABILITY: &str = "hagency.project_reservations.v1";
pub const ALLOCATION_FEATURE: &str = "project-reservations-v1";
pub const ALLOCATION_REVIEW_FEATURE: &str = "project-allocation-review-v1";
pub const ADMISSION_BOUND_GRANTS_FEATURE: &str = "admission-bound-grants-v2";
/// A v2 admission accepts one grant per UTC month, so its owner can grant the
/// same pool again once the previous grant's month has ended.
pub const POOL_MONTHLY_REGRANT_FEATURE: &str = "pool-monthly-regrant-v1";
pub const MAX_NATIVE_GRANT_BYTES: usize = 12 * 1024;
pub const MAX_EXACT: i64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeGrantReceipt {
    pub version: u16,
    pub capability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_request_id: Option<String>,
    pub enrollment_id: String,
    pub grant_id: String,
    pub grant_revision: i64,
    pub contribution_id: String,
    pub resource_id: String,
    pub resource_revision: String,
    pub executor_device_id: String,
    pub executor_generation: i64,
    pub owner_mxid: String,
    pub coordinator_mxid: String,
    pub delegation_revision: i64,
    pub delegation_expires_at_ms: i64,
    pub allow_self_approval: bool,
    pub eligible_managers: Vec<String>,
    pub allocated_tokens: i64,
    pub period_key: String,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    pub ledger_revision: i64,
}
impl NativeGrantReceipt {
    pub(crate) fn validate(&self, server: &str) -> Result<()> {
        // Reserve envelope headroom below the 16 KiB management command cap.
        // Native must apply the same limit before committing the parent hold.
        if serde_json::to_vec(self)
            .map_err(|_| Error::Invalid("invalid_resource_grant"))?
            .len()
            > MAX_NATIVE_GRANT_BYTES
        {
            return Err(Error::Invalid("resource_grant_too_large"));
        }
        for value in [
            &self.enrollment_id,
            &self.grant_id,
            &self.contribution_id,
            &self.resource_id,
            &self.executor_device_id,
        ] {
            key(value)?;
        }
        for value in [
            self.grant_revision,
            self.executor_generation,
            self.delegation_revision,
            self.delegation_expires_at_ms,
            self.allocated_tokens,
            self.period_start_ms,
            self.period_end_ms,
            self.ledger_revision,
        ] {
            if !(1..=MAX_EXACT).contains(&value) {
                return Err(Error::Invalid("invalid_allocation_number"));
            }
        }
        for mxid in [&self.owner_mxid, &self.coordinator_mxid]
            .into_iter()
            .chain(&self.eligible_managers)
        {
            crate::management_types::local_user(mxid, server)?;
        }
        if !matches!(
            (self.version, self.admission_request_id.as_ref()),
            (1, None) | (2, Some(_))
        )
            || self.capability != ALLOCATION_CAPABILITY
        {
            return Err(Error::Invalid("unsupported_allocation_capability"));
        }
        if let Some(request) = &self.admission_request_id {
            key(request)?;
        }
        if self.eligible_managers.is_empty()
            || self.eligible_managers.len() > 64
            || self
                .eligible_managers
                .iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .ne(self.eligible_managers.iter())
            || !valid_digest(&self.resource_revision)
            || self.period_key.len() != 7
            || self.period_end_ms <= self.period_start_ms
        {
            return Err(Error::Invalid("invalid_resource_grant"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishGrantInput {
    pub command_id: String,
    pub expected_contribution_revision: i64,
    pub receipt: NativeGrantReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceGrantView {
    pub id: String,
    pub receipt: NativeGrantReceipt,
    pub receipt_digest: String,
    pub state: String,
    pub revision: i64,
    pub observed_at_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAllocation {
    pub id: String,
    pub project_id: String,
    pub project_revision: i64,
    pub grant_id: String,
    pub requested_tokens: i64,
    pub state: String,
    pub revision: i64,
    pub request_id: String,
    pub requester_mxid: String,
    pub coordinator_mxid: String,
    pub period_key: String,
    pub expires_at_ms: i64,
    pub can_decide: bool,
    pub can_release: bool,
    pub blocking_reason: Option<String>,
    pub result: Option<NativeAllocationReceipt>,
    /// Last applied native balance. A refused release does not refund this hold.
    pub balance: Option<NativeAllocationReceipt>,
    /// Current central evidence floor, separate from immutable native receipts.
    #[serde(default)]
    pub observed_usage: Option<AllocationObservedUsage>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AllocationObservedUsage {
    pub known_consumed_tokens: i64,
    pub retained_tokens_floor: i64,
    pub requires_reconciliation: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AllocationCommand {
    pub version: u16,
    pub command_id: String,
    pub operation: String,
    pub allocation_id: String,
    pub project_id: String,
    pub project_revision: i64,
    pub request_id: String,
    pub request_revision: i64,
    pub decision_actor: String,
    pub manager_mxid: String,
    pub project_definition_digest: String,
    pub grant_receipt: NativeGrantReceipt,
    pub requested_tokens: i64,
    pub reserve_command_id: Option<String>,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeAllocationReceipt {
    pub version: u16,
    pub command_id: String,
    pub allocation_id: String,
    pub state: String,
    pub ledger_revision: i64,
    pub reserved_tokens: i64,
    pub retained_tokens: i64,
    pub period_key: String,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AllocationResultInput {
    pub command_digest: String,
    pub token: String,
    pub receipt: NativeAllocationReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorRecoverInput {
    pub command_digest: String,
}
#[derive(Default, Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorPollInput {
    pub after: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorDelivery {
    pub command: AllocationCommand,
    pub command_digest: String,
    pub token: String,
    pub lease_until_ms: i64,
    pub execution_allowed: bool,
    pub recorded: Option<NativeAllocationReceipt>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationAcknowledgement {
    pub command_id: String,
    pub command_digest: String,
    pub receipt_digest: String,
    pub allocation: ProjectAllocation,
}
pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Byte-compatible with hagency_core::canonical for these closed DTOs: keys
/// use JavaScript array-index then UTF-16 ordering; numbers are exact integers.
pub fn allocation_digest<T: Serialize>(value: &T) -> Result<String> {
    Ok(hash(&allocation_encoding(value)?))
}

/// Exact canonical bytes shared by immutable DTO digests and context rendering.
pub fn allocation_encoding<T: Serialize>(value: &T) -> Result<String> {
    fn index(key: &str) -> Option<u32> {
        let n = key.parse::<u32>().ok()?;
        (n < u32::MAX && n.to_string() == key).then_some(n)
    }
    fn write(value: &Value, out: &mut String, depth: usize) -> Result<()> {
        if depth > 64 {
            return Err(Error::Invalid("invalid_allocation_encoding"));
        }
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::String(s) => out.push_str(
                &serde_json::to_string(s)
                    .map_err(|_| Error::Invalid("invalid_allocation_encoding"))?,
            ),
            Value::Number(n) => {
                let n = n
                    .as_i64()
                    .filter(|n| n.unsigned_abs() <= MAX_EXACT as u64)
                    .ok_or(Error::Invalid("invalid_allocation_number"))?;
                out.push_str(&n.to_string());
            }
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(item, out, depth + 1)?;
                }
                out.push(']');
            }
            Value::Object(items) => {
                if items.contains_key("__proto__") {
                    return Err(Error::Invalid("invalid_allocation_encoding"));
                }
                let mut keys = items.keys().collect::<Vec<_>>();
                keys.sort_by(|a, b| match (index(a), index(b)) {
                    (Some(a), Some(b)) => a.cmp(&b),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.encode_utf16().cmp(b.encode_utf16()),
                });
                out.push('{');
                for (i, key) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(
                        &serde_json::to_string(key)
                            .map_err(|_| Error::Invalid("invalid_allocation_encoding"))?,
                    );
                    out.push(':');
                    write(&items[*key], out, depth + 1)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }
    let value =
        serde_json::to_value(value).map_err(|_| Error::Invalid("invalid_allocation_encoding"))?;
    let mut out = String::new();
    write(&value, &mut out, 0)?;
    Ok(out)
}

/// Public human review, separate from the native receipt and execution ticket.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationGrantSummary {
    pub id: String,
    pub revision: i64,
    pub state: String,
    pub owner_mxid: String,
    pub coordinator_mxid: String,
    pub total_tokens: i64,
    pub period_key: String,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    pub delegation_expires_at_ms: i64,
    /// No central read can infer current physical headroom from absent receipts.
    pub native_available_tokens: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAllocationDetail {
    pub allocation: ProjectAllocation,
    pub request: crate::management_types::ManagementRequest,
    pub project_name: String,
    pub resource_label: String,
    pub grant: AllocationGrantSummary,
}
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAllocationsQuery {
    pub after: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAllocationPage {
    pub items: Vec<ProjectAllocationDetail>,
    pub next_after: Option<String>,
}
