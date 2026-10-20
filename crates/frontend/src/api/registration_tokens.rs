use crate::api::client::*;
use crate::types::*;
use crate::utils::cache::{cached, remove_cached};
use crate::utils::error::HttpError;

const TOKEN_LIST_CACHE_TTL_MS: f64 = 30_000.0;
const TOKEN_LIST_CACHE_KEY: &str = "tokens:list";

pub fn invalidate_token_related_caches() {
    remove_cached(TOKEN_LIST_CACHE_KEY);
}

pub async fn get_registration_tokens() -> Result<Vec<RegistrationTokenRecord>, HttpError> {
    let url = build_url("/_palpo/admin/v1/registration_tokens", &[])?;

    let response: RegistrationTokensResponse = api_client(&url, "GET", None).await?;

    Ok(response
        .registration_tokens
        .into_iter()
        .map(|t| {
            let id = t.token.clone();
            RegistrationTokenRecord { id, token: t }
        })
        .collect())
}

pub async fn get_registration_tokens_cached() -> Result<Vec<RegistrationTokenRecord>, HttpError> {
    cached(
        TOKEN_LIST_CACHE_KEY,
        TOKEN_LIST_CACHE_TTL_MS,
        get_registration_tokens(),
    )
    .await
}

pub async fn create_registration_token(
    token: Option<&str>,
    uses_allowed: Option<u64>,
    expiry_time: Option<u64>,
    length: Option<u64>,
) -> Result<RegistrationTokenRecord, HttpError> {
    let url = build_url("/_palpo/admin/v1/registration_tokens/new", &[])?;

    let mut body = serde_json::Map::new();
    if let Some(token) = token {
        body.insert("token".to_string(), serde_json::json!(token));
    }
    if let Some(uses) = uses_allowed {
        body.insert("uses_allowed".to_string(), serde_json::json!(uses));
    }
    if let Some(expiry) = expiry_time {
        body.insert("expiry_time".to_string(), serde_json::json!(expiry));
    }
    if let Some(len) = length {
        body.insert("length".to_string(), serde_json::json!(len));
    }

    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let t: RegistrationToken = api_client(&url, "POST", Some(body_str)).await?;
    invalidate_token_related_caches();
    Ok(RegistrationTokenRecord {
        id: t.token.clone(),
        token: t,
    })
}

pub async fn delete_registration_token(token: &str) -> Result<(), HttpError> {
    let url = build_url(
        &format!("/_palpo/admin/v1/registration_tokens/{token}"),
        &[],
    )?;
    let _: serde_json::Value = api_client(&url, "DELETE", None).await?;
    invalidate_token_related_caches();
    Ok(())
}
