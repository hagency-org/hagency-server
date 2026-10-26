//! Device-staged original-request file delivery. Blob custody, upload and send
//! are separate irreversible stages; unknown work is never reclaimed/retried.
use crate::{Error, Result, hash};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub const CAPABILITY: &str = "agent-file-delivery-v1";
pub const HISTORY_CAPABILITY: &str = "agent-file-history-v1";
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const CHUNK_BYTES: usize = 64 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub filename: String,
    pub caption: Option<String>,
    pub bytes: u64,
    pub sha256: String,
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if self.filename.is_empty()
            || self.filename.len() > 255
            || self.filename.chars().any(char::is_control)
            || self.filename.contains(['/', '\\'])
            || matches!(self.filename.as_str(), "." | "..")
            || self
                .caption
                .as_ref()
                .is_some_and(|s| s.len() > 1000 || s.contains('\0'))
            || self.bytes > MAX_BYTES as u64
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid("invalid_file_manifest"));
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(hash(
            &serde_json::to_string(&(&self.filename, &self.caption, self.bytes, &self.sha256))
                .map_err(|_| Error::Invalid("invalid_file_manifest"))?,
        ))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub dispatch_id: String,
    pub execution_id: String,
    pub file_id: String,
}
impl Target {
    pub fn validate(&self) -> Result<()> {
        for key in [&self.dispatch_id, &self.execution_id, &self.file_id] {
            crate::key(key)?;
        }
        Ok(())
    }
}
/// Original preparation identity, including when its response/file ID was lost.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileLookup {
    pub dispatch_id: String,
    pub execution_id: String,
    pub request_id: String,
}
impl FileLookup {
    pub fn validate(&self) -> Result<()> {
        for key in [&self.dispatch_id, &self.execution_id, &self.request_id] {
            crate::key(key)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileObservation {
    pub request: FileLookup,
    pub delivery: Option<Delivery>,
    /// Read under the same fence as preparation; no future preparation is legal
    /// for this exact execution. Does not cancel or settle any existing delivery.
    pub preparation_closed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delivery {
    pub id: String,
    pub dispatch_id: String,
    pub execution_id: String,
    pub request_id: String,
    pub created_at_ms: i64,
    pub manifest: Manifest,
    pub manifest_digest: String,
    pub received_bytes: u64,
    pub state: String,
    pub upload_accepted: bool,
    pub matrix_event_id: Option<String>,
}
pub(crate) fn content(
    filename: &str,
    caption: Option<&str>,
    bytes: u64,
    mxc: &str,
    id: &str,
    room: &str,
    thread: &str,
) -> Value {
    let mut content = json!({"msgtype":"m.file","body":caption.filter(|s|!s.is_empty()).unwrap_or(filename),"filename":filename,"url":mxc,"info":{"mimetype":"application/octet-stream","size":bytes},"org.hagency.file_delivery_id":id});
    if thread != room {
        content["m.relates_to"] =
            json!({"rel_type":"m.thread","event_id":thread,"is_falling_back":false});
    }
    content
}
#[cfg(test)]
mod tests;
