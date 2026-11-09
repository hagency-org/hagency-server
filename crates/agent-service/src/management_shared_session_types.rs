//! Read-only routing before a native CLI chooses its persisted conversation.
//! This is not work, call admission, a resume permission or a process lease.
use crate::{
    Error, Result, key,
    management_allocation_types::valid_digest,
    management_shared_execution_types::{SharedRoomBinding, SharedWorkItem},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedSessionRouteInput {
    pub command_digest: String,
    pub token: String,
}
impl SharedSessionRouteInput {
    pub fn validate(&self) -> Result<()> {
        if !valid_digest(&self.command_digest) || !valid_digest(&self.token) {
            return Err(Error::Invalid("invalid_shared_session_route"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedSessionRoute {
    pub version: u16,
    pub runtime_command_id: String,
    pub runtime_command_digest: String,
    pub assignment_id: String,
    pub event_id: String,
    pub body_digest: String,
    pub room_id: String,
    pub requester_mxid: String,
    pub thread_root: String,
    pub room_binding: Option<SharedRoomBinding>,
    pub encrypted: bool,
}
impl SharedSessionRoute {
    pub fn validate(&self) -> Result<()> {
        key(&self.runtime_command_id)?;
        key(&self.assignment_id)?;
        let matrix = |id: &str, prefix| {
            id.starts_with(prefix)
                && (2..=255).contains(&id.len())
                && !id.chars().any(char::is_control)
        };
        if self.version != 1
            || !valid_digest(&self.runtime_command_digest)
            || !valid_digest(&self.body_digest)
            || !matrix(&self.event_id, '$')
            || !matrix(&self.room_id, '!')
            || !matrix(&self.requester_mxid, '@')
            || !matrix(&self.thread_root, '$')
        {
            return Err(Error::Invalid("invalid_shared_session_route"));
        }
        if let Some(binding) = &self.room_binding {
            key(&binding.id)?;
            if binding.generation <= 0
                || binding.assignment_id != self.assignment_id
                || binding.room_id != self.room_id
                || !matrix(&binding.space_id, '!')
            {
                return Err(Error::Invalid("invalid_shared_session_route"));
            }
        }
        Ok(())
    }
    /// The host must compare the separately admitted original work before
    /// sending any model input. A stale route must never select another user.
    pub fn matches_work(&self, work: &SharedWorkItem) -> bool {
        self.assignment_id == work.assignment_id
            && self.runtime_command_digest == work.runtime_command_digest
            && self.event_id == work.event_id
            && self.body_digest == work.body_digest
            && self.room_id == work.room_id
            && self.requester_mxid == work.requester_mxid
            && self.thread_root == work.thread_root
            && self.room_binding == work.room_binding
            && self.encrypted == work.encrypted
    }
}
