use serde::Deserialize;
use std::collections::BTreeMap;

use super::client::api_client;
use crate::utils::{cache::cached, error::HttpError};

#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
pub struct AdminCapabilities {
    #[serde(default)]
    capabilities: BTreeMap<String, bool>,
}

impl AdminCapabilities {
    pub fn supports(&self, name: &str) -> bool {
        self.capabilities.get(name).copied().unwrap_or(false)
    }
}

pub async fn get() -> Result<AdminCapabilities, HttpError> {
    cached("palpo:admin-capabilities", 30_000.0, async {
        match api_client("/_palpo/admin/v1/capabilities", "GET", None).await {
            Err(error) if matches!(error.status, 404 | 405) => Ok(AdminCapabilities::default()),
            result => result,
        }
    })
    .await
}

pub async fn require(name: &str) -> Result<(), HttpError> {
    if get().await?.supports(name) {
        Ok(())
    } else {
        Err(HttpError::message(format!(
            "This server does not advertise the {name} administration capability. Update the server to use this operation."
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_and_missing_flags_fail_closed() {
        let value: AdminCapabilities = serde_json::from_str(
            r#"{"capabilities":{"room_aliases":true,"make_room_admin":false}}"#,
        )
        .unwrap();
        assert!(value.supports("room_aliases"));
        assert!(!value.supports("make_room_admin"));
        assert!(!value.supports("media_keep_profiles"));
    }
}
