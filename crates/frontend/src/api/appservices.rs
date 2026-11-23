//! Palpo admin appservice management.
//!
//! Wraps the `/_palpo/admin/v1/appservices` endpoints added in palpo PR #119.
//! These endpoints are mounted under the homeserver itself (not the separate
//! palpo_admin sidecar), so we use the standard `api_client` which targets the
//! same-origin Nginx proxy and reuses the user's Matrix access token.

use crate::api::client::api_client;
use crate::types::{AppserviceRegistration, AppserviceSummary, ListAppservicesResponse};
use crate::utils::error::HttpError;

const BASE: &str = "/_palpo/admin/v1/appservices";

pub async fn list_appservices() -> Result<Vec<AppserviceSummary>, HttpError> {
    let resp: ListAppservicesResponse = api_client(BASE, "GET", None).await?;
    Ok(resp.appservices)
}

pub async fn get_appservice(id: &str) -> Result<AppserviceRegistration, HttpError> {
    let path = format!("{BASE}/{}", urlencoding::encode(id));
    api_client(&path, "GET", None).await
}

pub async fn register_appservice(
    reg: &AppserviceRegistration,
) -> Result<serde_json::Value, HttpError> {
    let body = serde_json::to_string(reg)
        .map_err(|e| HttpError::message(format!("Failed to serialize registration: {e}")))?;
    api_client(BASE, "POST", Some(body)).await
}

/// The running server supports callback-URL compare-and-swap, not arbitrary registration replacement.
pub async fn update_appservice_url(
    id: &str,
    url: &str,
    expected_url: Option<&str>,
) -> Result<serde_json::Value, HttpError> {
    let path = format!("{BASE}/{}/url", urlencoding::encode(id));
    api_client(
        &path,
        "PUT",
        Some(url_update_body(url, expected_url).to_string()),
    )
    .await
}
fn url_update_body(url: &str, expected_url: Option<&str>) -> serde_json::Value {
    serde_json::json!({"url":url,"expected_url":expected_url})
}
pub async fn delete_appservice(id: &str) -> Result<(), HttpError> {
    let path = format!("{BASE}/{}", urlencoding::encode(id));
    let _: serde_json::Value = api_client(&path, "DELETE", None).await?;
    Ok(())
}

pub async fn disable_appservice(id: &str) -> Result<(), HttpError> {
    let path = format!("{BASE}/{}/disable", urlencoding::encode(id));
    let _: serde_json::Value = api_client(&path, "POST", None).await?;
    Ok(())
}

pub async fn enable_appservice(id: &str) -> Result<(), HttpError> {
    let path = format!("{BASE}/{}/enable", urlencoding::encode(id));
    let _: serde_json::Value = api_client(&path, "POST", None).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_cas_carries_the_original_value_even_when_null() {
        assert_eq!(
            url_update_body("https://bridge.example", None),
            serde_json::json!({"url":"https://bridge.example","expected_url":null})
        );
        assert_eq!(
            url_update_body("https://new.example", Some("https://old.example"))["expected_url"],
            "https://old.example"
        );
        assert_eq!(
            url_update_body("https://new.example", None)
                .as_object()
                .unwrap()
                .len(),
            2
        );
    }
}
