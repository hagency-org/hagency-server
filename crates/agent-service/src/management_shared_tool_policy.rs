//! Explicit resource-owner permissions bound to an immutable successor start.
use serde::{Deserialize,Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct SharedAgentToolPolicy { pub mode:String, pub revision:i64 }
impl SharedAgentToolPolicy {
    pub fn validate(&self)->crate::Result<()> {
        if !matches!(self.mode.as_str(),"standard"|"yolo") || !(1..=crate::management_allocation_types::MAX_EXACT).contains(&self.revision) { return Err(crate::Error::Invalid("invalid_shared_tool_policy")); }
        Ok(())
    }
}
