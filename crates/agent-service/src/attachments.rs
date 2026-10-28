//! Immutable attachment descriptors from canonical Appservice messages. The
//! device endpoint exposes them only for its exact currently running request.
use crate::{Error, Result};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const CAPABILITY: &str = "agent-attachment-metadata-v1";
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attachment {
    pub descriptor_digest: String,
    pub filename: String,
    pub message_type: String,
    pub content_type: Option<String>,
    pub declared_size: Option<u64>,
    pub source: Value,
}
impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attachment")
            .field("descriptor_digest", &self.descriptor_digest)
            .field("filename", &self.filename)
            .field("source", &"[private media locator/key]")
            .finish()
    }
}
fn invalid() -> Error {
    Error::Invalid("invalid_agent_attachment")
}
fn text(value: &str, max: usize) -> Result<()> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}
fn mxc(value: &str) -> Result<()> {
    if value.len() > 4096 || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid());
    }
    let uri = url::Url::parse(value).map_err(|_| invalid())?;
    if uri.scheme() != "mxc"
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.host_str().is_none()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return Err(invalid());
    }
    let path = value
        .strip_prefix("mxc://")
        .and_then(|r| r.split_once('/'))
        .map(|(_, p)| p)
        .ok_or_else(invalid)?;
    if path.is_empty()
        || path.contains('/')
        || path.contains('%')
        || path.contains('\\')
        || matches!(path, "." | "..")
    {
        return Err(invalid());
    }
    Ok(())
}
fn decode(value: &Value, field: &str, len: usize, url: bool) -> Result<()> {
    let text = value[field].as_str().ok_or_else(invalid)?;
    let bytes = if url {
        URL_SAFE_NO_PAD.decode(text)
    } else {
        STANDARD_NO_PAD.decode(text)
    }
    .map_err(|_| invalid())?;
    if bytes.len() != len {
        return Err(invalid());
    }
    Ok(())
}
fn source(content: &Value) -> Result<Value> {
    if let Some(file) = content.get("file") {
        if content.get("url").is_some()
            || file["v"] != "v2"
            || file["key"]["kty"] != "oct"
            || file["key"]["alg"] != "A256CTR"
            || file["key"]["ext"] != true
            || !file["key"]["key_ops"].as_array().is_some_and(|v| {
                v.len() == 2
                    && v.contains(&Value::String("encrypt".into()))
                    && v.contains(&Value::String("decrypt".into()))
            })
        {
            return Err(invalid());
        }
        mxc(file["url"].as_str().ok_or_else(invalid)?)?;
        decode(&file["key"], "k", 32, true)?;
        decode(file, "iv", 16, false)?;
        decode(&file["hashes"], "sha256", 32, false)?;
        Ok(
            serde_json::json!({"kind":"encrypted","file":{"url":file["url"],"v":"v2","key":{"kty":"oct","key_ops":file["key"]["key_ops"],"alg":"A256CTR","k":file["key"]["k"],"ext":true},"iv":file["iv"],"hashes":{"sha256":file["hashes"]["sha256"]}}}),
        )
    } else {
        let url = content["url"].as_str().ok_or_else(invalid)?;
        mxc(url)?;
        Ok(serde_json::json!({"kind":"plain","url":url}))
    }
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .map(|(k, v)| (k.clone(), canonical(v)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}
impl Attachment {
    pub fn from_content(content: &Value) -> Result<Self> {
        let kind = content["msgtype"].as_str().ok_or_else(invalid)?;
        if !matches!(kind, "m.file" | "m.image" | "m.audio" | "m.video") {
            return Err(invalid());
        }
        let filename = content
            .get("filename")
            .unwrap_or(&content["body"])
            .as_str()
            .ok_or_else(invalid)?;
        if filename.is_empty() || filename.len() > 65536 || filename.contains('\0') {
            return Err(invalid());
        }
        if content.get("info").is_some_and(|v| !v.is_object()) {
            return Err(invalid());
        }
        let content_type = content["info"]
            .get("mimetype")
            .map(|v| v.as_str().ok_or_else(invalid).map(str::to_owned))
            .transpose()?;
        if let Some(mime) = &content_type {
            text(mime, 255)?;
            if !mime.is_ascii() || !mime.contains('/') || mime.chars().any(char::is_control) {
                return Err(invalid());
            }
        }
        let declared_size = content["info"]
            .get("size")
            .map(|v| {
                v.as_u64()
                    .filter(|n| *n <= 9_007_199_254_740_991)
                    .ok_or_else(invalid)
            })
            .transpose()?;
        let source = source(content)?;
        let encoded = serde_json::to_value((filename, kind, &content_type, declared_size, &source))
            .map_err(|_| invalid())?;
        let digest =
            crate::hash(&serde_json::to_string(&canonical(&encoded)).map_err(|_| invalid())?);
        Ok(Self {
            descriptor_digest: digest,
            filename: filename.into(),
            message_type: kind.into(),
            content_type,
            declared_size,
            source,
        })
    }
    pub fn validate(&self) -> Result<()> {
        let mut content = serde_json::json!({"msgtype":self.message_type,"filename":self.filename,"body":self.filename,"info":{}});
        if let Some(mime) = &self.content_type {
            content["info"]["mimetype"] = mime.clone().into();
        }
        if let Some(size) = self.declared_size {
            content["info"]["size"] = size.into();
        }
        match self.source["kind"].as_str() {
            Some("plain") => content["url"] = self.source["url"].clone(),
            Some("encrypted") => content["file"] = self.source["file"].clone(),
            _ => return Err(invalid()),
        };
        if Self::from_content(&content)? != *self {
            return Err(invalid());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests;
