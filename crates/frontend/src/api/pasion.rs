use serde::de::DeserializeOwned;

use crate::api::client::raw_fetch;
use crate::types::*;
use crate::utils::error::{HttpError, MatrixError};
use crate::utils::storage;

fn get_pasion_url() -> Option<String> {
    storage::get_item("pasion_url")
}

/// Format a Pasion JSON:API error body. Pasion returns
/// `{"errors":[{"title":"..."}]}` rather than a Matrix errcode, so we
/// surface the raw text and key off the HTTP status. 503 is treated
/// specially as "server in maintenance mode".
fn format_pasion_error(status: u16, text: &str) -> (String, Option<MatrixError>) {
    if status == 503 {
        return ("Server is in maintenance mode".to_string(), None);
    }
    let title = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|body| {
            body.pointer("/errors/0/title")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    match title {
        Some(title) => (format!("{title} ({status})"), None),
        None => (format!("Pasion error ({status}): {text}"), None),
    }
}

async fn pasion_fetch<T: DeserializeOwned>(
    path: &str,
    method: &str,
    body: Option<String>,
) -> Result<T, HttpError> {
    if storage::get_item("auth_kind").as_deref() != Some("oauth2") {
        return Err(HttpError::from_status(
            403,
            "Pasion account administration requires a Pasion OAuth session.",
        ));
    }
    let base =
        get_pasion_url().ok_or_else(|| HttpError::message("Pasion URL is not configured"))?;
    let url = format!("{base}/api/admin/v1{path}");

    let result = raw_fetch::<T, _>(&url, method, body.clone(), format_pasion_error).await;

    // On any 401, try to refresh the access token and retry once. If
    // refresh fails, the shared handler clears the stored session so
    // AppLayout's `is_authenticated()` heartbeat redirects to /login.
    if let Err(ref err) = result
        && err.status == 401
        && crate::api::auth::handle_unauthorized().await
    {
        return raw_fetch::<T, _>(&url, method, body, format_pasion_error).await;
    }
    // Pasion scopes/roles are separate from Palpo administrator rights.

    result
}

// ── JSON:API envelope helpers ──────────────────────────────────────────────
//
// Pasion's admin API wraps resources in a JSON:API envelope:
//
//   List:   {"meta":{"count":N},"data":[{"type":"...","id":"...","attributes":{...},"links":{...}}, ...]}
//   Single: {"data":{"type":"...","id":"...","attributes":{...},"links":{...}}}
//
// Frontend types (see `src/types/pasion.rs`) are flat structs, so these
// helpers hoist `id` into `attributes` before deserializing into `T`.
// Use `pasion_fetch` directly for endpoints that don't follow this
// envelope shape (e.g. `/connector-health` returns a plain object).

fn flatten_resource<T: DeserializeOwned>(item: &serde_json::Value) -> Result<T, HttpError> {
    let id = item.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let mut attrs = item
        .get("attributes")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = attrs.as_object_mut()
        && !id.is_null()
    {
        obj.insert("id".to_string(), id);
    }
    serde_json::from_value(attrs).map_err(|e| HttpError::message(format!("JSON parse error: {e}")))
}

async fn pasion_fetch_list<T: DeserializeOwned>(
    path: &str,
) -> Result<PasionListResponse<T>, HttpError> {
    let raw: serde_json::Value = pasion_fetch(path, "GET", None).await?;
    let meta: PasionPaginationMeta = raw
        .get("meta")
        .and_then(|m| serde_json::from_value(m.clone()).ok())
        .unwrap_or_default();
    let data_arr = raw
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    let mut data = Vec::with_capacity(data_arr.len());
    for item in &data_arr {
        data.push(flatten_resource::<T>(item)?);
    }
    Ok(PasionListResponse { data, meta })
}

/// Like [`pasion_fetch_list`], but also returns the total count and the
/// cursor of the last item when Pasion reports a next page.
async fn pasion_fetch_page<T: DeserializeOwned>(path: &str) -> Result<PasionPage<T>, HttpError> {
    let raw: serde_json::Value = pasion_fetch(path, "GET", None).await?;
    let count = raw
        .pointer("/meta/count")
        .and_then(serde_json::Value::as_u64);
    let data_arr = raw
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    let has_next = raw
        .pointer("/links/next")
        .is_some_and(|next| !next.is_null());
    let next_cursor = if has_next {
        data_arr
            .last()
            .and_then(|item| item.pointer("/meta/page/cursor"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    } else {
        None
    };
    let mut data = Vec::with_capacity(data_arr.len());
    for item in &data_arr {
        data.push(flatten_resource::<T>(item)?);
    }
    Ok(PasionPage {
        data,
        count,
        next_cursor,
    })
}

async fn pasion_fetch_one<T: DeserializeOwned>(
    path: &str,
    method: &str,
    body: Option<String>,
) -> Result<T, HttpError> {
    let raw: serde_json::Value = pasion_fetch(path, method, body).await?;
    // Single-resource envelope has the resource under `data`; some
    // endpoints return the bare resource (no envelope), so fall back
    // to the raw value in that case.
    let item = raw.get("data").unwrap_or(&raw);
    flatten_resource::<T>(item)
}

// ── Users ──────────────────────────────────────────────────────────────────

/// Filters for `GET /users`. `None` means "don't filter on this".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PasionUserFilter {
    pub search: Option<String>,
    /// `active`, `locked` (includes deactivated) or `deactivated`.
    pub status: Option<String>,
    pub admin: Option<bool>,
}

/// List users. Pasion paginates with cursors (`page[first]` /
/// `page[after]`), not page numbers.
pub async fn pasion_get_users(
    filter: &PasionUserFilter,
    after: Option<&str>,
    page_size: u64,
) -> Result<PasionPage<PasionUser>, HttpError> {
    let mut path = format!("/users?page[first]={page_size}");
    if let Some(after) = after {
        path.push_str(&format!("&page[after]={}", urlencoding::encode(after)));
    }
    if let Some(search) = filter.search.as_deref().filter(|s| !s.is_empty()) {
        path.push_str(&format!("&filter[search]={}", urlencoding::encode(search)));
    }
    if let Some(status) = filter.status.as_deref().filter(|s| !s.is_empty()) {
        path.push_str(&format!("&filter[status]={status}"));
    }
    if let Some(admin) = filter.admin {
        path.push_str(&format!("&filter[admin]={admin}"));
    }
    pasion_fetch_page(&path).await
}

pub async fn pasion_get_user(id: &str) -> Result<PasionUser, HttpError> {
    pasion_fetch_one(&format!("/users/{id}"), "GET", None).await
}

pub async fn pasion_get_user_by_username(username: &str) -> Result<PasionUser, HttpError> {
    pasion_fetch_one(
        &format!("/users/by-username/{}", urlencoding::encode(username)),
        "GET",
        None,
    )
    .await
}

pub async fn pasion_create_user(username: &str) -> Result<PasionUser, HttpError> {
    let body = serde_json::json!({ "username": username });
    pasion_fetch_one("/users", "POST", Some(body.to_string())).await
}

/// `PATCH /users/{id}`. Accepted keys: `display_name`, `avatar_url`,
/// `preferred_locale` (null clears), `admin`, `locked`, `deactivated`,
/// `hs_erase`.
pub async fn pasion_update_user(
    id: &str,
    data: serde_json::Value,
) -> Result<PasionUser, HttpError> {
    pasion_fetch_one(&format!("/users/{id}"), "PATCH", Some(data.to_string())).await
}

pub async fn pasion_set_password(
    id: &str,
    password: &str,
    skip_password_check: bool,
) -> Result<(), HttpError> {
    let body = serde_json::json!({
        "password": password,
        "skip_password_check": skip_password_check,
    });
    let _: serde_json::Value = pasion_fetch(
        &format!("/users/{id}/set-password"),
        "POST",
        Some(body.to_string()),
    )
    .await?;
    Ok(())
}

/// Result of `POST /users/{id}/risk-action`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PasionRiskActionResult {
    #[serde(default)]
    pub sessions_terminated: Option<u64>,
}

pub async fn pasion_risk_action(
    id: &str,
    action: &str,
    reason: Option<&str>,
) -> Result<PasionRiskActionResult, HttpError> {
    // Pasion exposes a single POST /users/{id}/risk-action endpoint that
    // takes `{action, reason}` in the body. Valid actions are `lock`,
    // `force_password_reset` (currently just a lock), and
    // `terminate_sessions`.
    let body = serde_json::json!({ "action": action, "reason": reason });
    pasion_fetch(
        &format!("/users/{id}/risk-action"),
        "POST",
        Some(body.to_string()),
    )
    .await
}

// ── User Emails ────────────────────────────────────────────────────────────

pub async fn pasion_get_user_emails(
    user_id: Option<&str>,
) -> Result<Vec<PasionUserEmail>, HttpError> {
    let path = if let Some(uid) = user_id {
        format!("/user-emails?page[first]=100&filter[user]={uid}")
    } else {
        "/user-emails?page[first]=100".to_string()
    };
    Ok(pasion_fetch_list::<PasionUserEmail>(&path).await?.data)
}

pub async fn pasion_add_user_email(
    user_id: &str,
    email: &str,
) -> Result<PasionUserEmail, HttpError> {
    let body = serde_json::json!({ "user_id": user_id, "email": email });
    pasion_fetch_one("/user-emails", "POST", Some(body.to_string())).await
}

pub async fn pasion_delete_user_email(id: &str) -> Result<(), HttpError> {
    let _: serde_json::Value = pasion_fetch(&format!("/user-emails/{id}"), "DELETE", None).await?;
    Ok(())
}

// ── Browser Sessions ───────────────────────────────────────────────────────
// Pasion exposes these as /user-sessions on the admin side; we keep the
// "browser session" vocabulary in the frontend names because that's what
// users see in the UI.

pub async fn pasion_get_browser_sessions(
    user_id: Option<&str>,
    active_only: bool,
) -> Result<Vec<PasionBrowserSession>, HttpError> {
    let mut path = "/user-sessions?page[first]=100".to_string();
    if let Some(uid) = user_id {
        path.push_str(&format!("&filter[user]={uid}"));
    }
    if active_only {
        path.push_str("&filter[status]=active");
    }
    Ok(pasion_fetch_list::<PasionBrowserSession>(&path).await?.data)
}

pub async fn pasion_finish_browser_session(id: &str) -> Result<(), HttpError> {
    let _: serde_json::Value =
        pasion_fetch(&format!("/user-sessions/{id}/finish"), "POST", None).await?;
    Ok(())
}

// ── OAuth2 Sessions ────────────────────────────────────────────────────────

pub async fn pasion_get_oauth2_sessions(
    user_id: Option<&str>,
) -> Result<Vec<PasionOAuth2Session>, HttpError> {
    let path = if let Some(uid) = user_id {
        format!("/oauth2-sessions?filter[user]={uid}")
    } else {
        "/oauth2-sessions".to_string()
    };
    Ok(pasion_fetch_list::<PasionOAuth2Session>(&path).await?.data)
}

/// Active OAuth2 (client) sessions of one user, e.g. their Matrix clients.
pub async fn pasion_get_active_oauth2_sessions_for_user(
    user_id: &str,
) -> Result<Vec<PasionOAuth2Session>, HttpError> {
    let path =
        format!("/oauth2-sessions?page[first]=100&filter[user]={user_id}&filter[status]=active");
    Ok(pasion_fetch_list::<PasionOAuth2Session>(&path).await?.data)
}

pub async fn pasion_finish_oauth2_session(id: &str) -> Result<(), HttpError> {
    let _: serde_json::Value =
        pasion_fetch(&format!("/oauth2-sessions/{id}/finish"), "POST", None).await?;
    Ok(())
}

// ── Personal Sessions (API Tokens) ─────────────────────────────────────────
// Pasion calls these "personal sessions" internally — what users see as a
// personal access token is implemented as a session with a long-lived
// access token attached.

pub async fn pasion_get_personal_sessions() -> Result<Vec<PasionPersonalSession>, HttpError> {
    Ok(
        pasion_fetch_list::<PasionPersonalSession>("/personal-sessions")
            .await?
            .data,
    )
}

pub async fn pasion_create_personal_session(
    data: serde_json::Value,
) -> Result<PasionPersonalSession, HttpError> {
    pasion_fetch_one("/personal-sessions", "POST", Some(data.to_string())).await
}

pub async fn pasion_regenerate_personal_session(
    id: &str,
) -> Result<PasionPersonalSession, HttpError> {
    pasion_fetch_one(&format!("/personal-sessions/{id}/regenerate"), "POST", None).await
}

pub async fn pasion_revoke_personal_session(id: &str) -> Result<(), HttpError> {
    let _: serde_json::Value =
        pasion_fetch(&format!("/personal-sessions/{id}/revoke"), "POST", None).await?;
    Ok(())
}

// ── Upstream OAuth Providers ───────────────────────────────────────────────

pub async fn pasion_get_upstream_providers() -> Result<Vec<PasionUpstreamProvider>, HttpError> {
    Ok(
        pasion_fetch_list::<PasionUpstreamProvider>("/upstream-oauth-providers")
            .await?
            .data,
    )
}

pub async fn pasion_get_upstream_provider(id: &str) -> Result<PasionUpstreamProvider, HttpError> {
    pasion_fetch_one(&format!("/upstream-oauth-providers/{id}"), "GET", None).await
}

pub async fn pasion_create_upstream_provider(
    data: serde_json::Value,
) -> Result<PasionUpstreamProvider, HttpError> {
    pasion_fetch_one("/upstream-oauth-providers", "POST", Some(data.to_string())).await
}

pub async fn pasion_update_upstream_provider(
    id: &str,
    data: serde_json::Value,
) -> Result<PasionUpstreamProvider, HttpError> {
    pasion_fetch_one(
        &format!("/upstream-oauth-providers/{id}"),
        "PATCH",
        Some(data.to_string()),
    )
    .await
}

pub async fn pasion_delete_upstream_provider(id: &str) -> Result<(), HttpError> {
    let _: serde_json::Value =
        pasion_fetch(&format!("/upstream-oauth-providers/{id}"), "DELETE", None).await?;
    Ok(())
}

pub async fn pasion_disable_upstream_provider(id: &str) -> Result<(), HttpError> {
    let _: serde_json::Value = pasion_fetch(
        &format!("/upstream-oauth-providers/{id}/disable"),
        "POST",
        None,
    )
    .await?;
    Ok(())
}

pub async fn pasion_enable_upstream_provider(id: &str) -> Result<(), HttpError> {
    let _: serde_json::Value = pasion_fetch(
        &format!("/upstream-oauth-providers/{id}/enable"),
        "POST",
        None,
    )
    .await?;
    Ok(())
}

// ── Upstream OAuth Links ───────────────────────────────────────────────────

pub async fn pasion_get_upstream_links(
    user_id: Option<&str>,
    provider_id: Option<&str>,
) -> Result<Vec<PasionUpstreamLink>, HttpError> {
    let mut params = Vec::new();
    if let Some(uid) = user_id {
        params.push(format!("filter[user]={uid}"));
    }
    if let Some(pid) = provider_id {
        params.push(format!("filter[provider]={pid}"));
    }
    let path = if params.is_empty() {
        "/upstream-oauth-links".to_string()
    } else {
        format!("/upstream-oauth-links?{}", params.join("&"))
    };
    Ok(pasion_fetch_list::<PasionUpstreamLink>(&path).await?.data)
}

pub async fn pasion_create_upstream_link(
    data: serde_json::Value,
) -> Result<PasionUpstreamLink, HttpError> {
    pasion_fetch_one("/upstream-oauth-links", "POST", Some(data.to_string())).await
}

pub async fn pasion_delete_upstream_link(id: &str) -> Result<(), HttpError> {
    let _: serde_json::Value =
        pasion_fetch(&format!("/upstream-oauth-links/{id}"), "DELETE", None).await?;
    Ok(())
}

// ── Audit Log ──────────────────────────────────────────────────────────────

pub async fn pasion_get_audit_feed(limit: u64) -> Result<PasionAuditFeedResponse, HttpError> {
    let path = format!("/audit-feed?limit={limit}");
    pasion_fetch(&path, "GET", None).await
}

// ── Notification Channels ──────────────────────────────────────────────────
// Pasion returns `{channels: [{channel, configured}]}` (not JSON:API).

pub async fn pasion_get_notification_channels() -> Result<Vec<PasionNotificationChannel>, HttpError>
{
    let raw: serde_json::Value = pasion_fetch("/notification-channels", "GET", None).await?;
    let channels = raw
        .get("channels")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    serde_json::from_value(channels)
        .map_err(|e| HttpError::message(format!("JSON parse error: {e}")))
}

// ── Notification Templates ─────────────────────────────────────────────────
// Pasion returns `{templates: [{key, description}]}` (not JSON:API).

pub async fn pasion_get_notification_templates()
-> Result<Vec<PasionNotificationTemplate>, HttpError> {
    let raw: serde_json::Value = pasion_fetch("/notification-templates", "GET", None).await?;
    let templates = raw
        .get("templates")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    serde_json::from_value(templates)
        .map_err(|e| HttpError::message(format!("JSON parse error: {e}")))
}

pub async fn pasion_publish_template(data: serde_json::Value) -> Result<(), HttpError> {
    let _: serde_json::Value = pasion_fetch(
        "/notification-templates/publish",
        "POST",
        Some(data.to_string()),
    )
    .await?;
    Ok(())
}

// ── Connector Health ───────────────────────────────────────────────────────

pub async fn pasion_get_connector_health() -> Result<Vec<PasionConnectorHealth>, HttpError> {
    // Endpoint lives at /api/admin/v1/connector-health and returns
    // `{providers: [{provider, homeserver, status, error?}]}` — not the
    // JSON:API envelope, so we fetch raw and pick the inner array.
    let raw: serde_json::Value = pasion_fetch("/connector-health", "GET", None).await?;
    let providers = raw
        .get("providers")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    serde_json::from_value(providers)
        .map_err(|e| HttpError::message(format!("JSON parse error: {e}")))
}
