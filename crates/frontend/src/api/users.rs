use crate::api::client::*;
use crate::types::*;
use crate::utils::cache::{cached, invalidate_cached_prefix, remove_cached};
use crate::utils::error::HttpError;
use crate::utils::mxid::return_mxid;

const USER_LIST_CACHE_TTL_MS: f64 = 30_000.0;
const USER_LIST_CACHE_PREFIX: &str = "users:list:";
const DASHBOARD_USER_COUNT_CACHE_KEY: &str = "dashboard_user_count";
const DASHBOARD_ACTIVE_USER_COUNT_CACHE_KEY: &str = "dashboard_active_user_count";

/// Page size used when fetching every user for the CSV export. The export
/// loops over pages until the backend's reported `total` is exhausted rather
/// than relying on one oversized request.
pub const EXPORT_PAGE_SIZE: u64 = 500;

/// Upper bound on the number of devices we fetch in one request when wiping a
/// user's devices. A single user is never expected to have anywhere near this
/// many sessions, so one page suffices.
const ALL_DEVICES_LIMIT: u64 = 10_000;

fn user_list_cache_key(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search_term: &str,
) -> String {
    let encoded_search = urlencoding::encode(search_term);
    format!("{USER_LIST_CACHE_PREFIX}{page}:{per_page}:{order_by}:{order}:{encoded_search}")
}

fn invalidate_user_related_caches() {
    invalidate_cached_prefix(USER_LIST_CACHE_PREFIX);
    remove_cached(DASHBOARD_USER_COUNT_CACHE_KEY);
    remove_cached(DASHBOARD_ACTIVE_USER_COUNT_CACHE_KEY);
}

fn map_user(user: User) -> UserRecord {
    // Palpo returns creation_ts in milliseconds (Matrix spec).
    let creation_ts_ms = user.creation_ts;
    let id = return_mxid(&user.name);
    let avatar_src = user.avatar_url.clone();
    UserRecord {
        id,
        avatar_src,
        creation_ts_ms,
        user,
    }
}

pub async fn get_user_count() -> Result<u64, HttpError> {
    let url = build_url("/_palpo/admin/v2/users", &[("limit", "1")])?;
    let response: UsersListResponse = api_client(&url, "GET", None).await?;
    Ok(response.total)
}

pub async fn get_active_user_count() -> Result<u64, HttpError> {
    let url = build_url(
        "/_palpo/admin/v2/users",
        &[
            ("limit", "1"),
            ("guests", "false"),
            ("deactivated", "false"),
        ],
    )?;
    let response: UsersListResponse = api_client(&url, "GET", None).await?;
    Ok(response.total)
}

pub async fn get_users(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search_term: &str,
) -> Result<ListResponse<UserRecord>, HttpError> {
    let from = (page - 1) * per_page;
    let from_str = from.to_string();
    let limit_str = per_page.to_string();
    let dir = get_search_order(order);

    let mut params = vec![
        ("from", from_str.as_str()),
        ("limit", limit_str.as_str()),
        ("order_by", order_by),
        ("dir", dir),
    ];

    // `/v2/users` filters on `name` (the localpart); it has no `search_term`.
    if !search_term.is_empty() {
        params.push(("name", search_term));
    }

    let url = build_url("/_palpo/admin/v2/users", &params)?;
    let response: UsersListResponse = api_client(&url, "GET", None).await?;

    Ok(ListResponse {
        data: response.users.into_iter().map(map_user).collect(),
        total: response.total,
    })
}

/// Fetch every user by paging through the admin API until the reported
/// `total` is exhausted. Used by the CSV export so we never depend on a single
/// oversized request returning all rows.
pub async fn get_all_users_for_export(
    order_by: &str,
    order: &str,
) -> Result<Vec<UserRecord>, HttpError> {
    let mut all = Vec::new();
    let mut page = 1u64;

    loop {
        let response = get_users(page, EXPORT_PAGE_SIZE, order_by, order, "").await?;
        let fetched = response.data.len() as u64;
        all.extend(response.data);

        // Stop once we've collected the reported total, or the backend
        // returned a short/empty page (defensive against a missing total).
        if fetched == 0 || all.len() as u64 >= response.total {
            break;
        }
        page += 1;
    }

    Ok(all)
}

pub async fn get_users_cached(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search_term: &str,
) -> Result<ListResponse<UserRecord>, HttpError> {
    let cache_key = user_list_cache_key(page, per_page, order_by, order, search_term);
    cached(
        &cache_key,
        USER_LIST_CACHE_TTL_MS,
        get_users(page, per_page, order_by, order, search_term),
    )
    .await
}

pub async fn get_user(id: &str) -> Result<UserRecord, HttpError> {
    let encoded = urlencoding::encode(id);
    let url = build_url(&format!("/_palpo/admin/v2/users/{encoded}"), &[])?;
    let user: User = api_client(&url, "GET", None).await?;
    Ok(map_user(user))
}

pub async fn create_user(id: &str, data: CreateUserRequest) -> Result<UserRecord, HttpError> {
    let user_id = return_mxid(id);
    let encoded = urlencoding::encode(&user_id);
    let url = build_url(&format!("/_palpo/admin/v2/users/{encoded}"), &[])?;
    let body = serde_json::to_string(&data).map_err(|e| HttpError::message(e.to_string()))?;
    let user: User = api_client(&url, "PUT", Some(body)).await?;
    invalidate_user_related_caches();
    Ok(map_user(user))
}

pub async fn update_user(id: &str, data: serde_json::Value) -> Result<UserRecord, HttpError> {
    let encoded = urlencoding::encode(id);
    let url = build_url(&format!("/_palpo/admin/v2/users/{encoded}"), &[])?;
    let body = serde_json::to_string(&data).map_err(|e| HttpError::message(e.to_string()))?;
    let user: User = api_client(&url, "PUT", Some(body)).await?;
    invalidate_user_related_caches();
    Ok(map_user(user))
}

pub async fn deactivate_user(id: &str, erase: bool) -> Result<(), HttpError> {
    let mxid = return_mxid(id);
    let encoded = urlencoding::encode(&mxid);
    let url = build_url(&format!("/_palpo/admin/v1/deactivate/{encoded}"), &[])?;
    let body = serde_json::json!({ "erase": erase }).to_string();
    let _: serde_json::Value = api_client(&url, "POST", Some(body)).await?;
    invalidate_user_related_caches();
    Ok(())
}

pub async fn set_user_deactivated(id: &str, deactivated: bool) -> Result<UserRecord, HttpError> {
    let mxid = return_mxid(id);
    let encoded = urlencoding::encode(&mxid);
    let url = build_url(&format!("/_palpo/admin/v2/users/{encoded}"), &[])?;
    let body = serde_json::json!({ "deactivated": deactivated }).to_string();
    let user: User = api_client(&url, "PUT", Some(body)).await?;
    invalidate_user_related_caches();
    Ok(map_user(user))
}

/// Which service carried out a deactivation or reactivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountOwner {
    /// Pasion owns the account: the change is made there and Pasion applies
    /// it to the homeserver (deactivation runs as a background job).
    Pasion,
    /// The account only exists on the homeserver (bots, appservice users, or
    /// deployments without Pasion) and was changed there directly.
    Homeserver,
}

/// The Pasion account behind a Matrix user, if Pasion is configured and has
/// one with the same username. Only a 404 means "no account"; any other error
/// is returned so a failed lookup never silently bypasses Pasion.
async fn pasion_account_for(user_id: &str) -> Result<Option<PasionUser>, HttpError> {
    if crate::utils::storage::get_item("pasion_url").is_none() {
        return Ok(None);
    }
    let mxid = return_mxid(user_id);
    let localpart = mxid
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or_default();
    match crate::api::pasion::pasion_get_user_by_username(localpart).await {
        Ok(account) => Ok(Some(account)),
        Err(e) if e.status == 404 => Ok(None),
        Err(e) => Err(e),
    }
}

/// Deactivate a local Matrix account through the service that owns it.
///
/// Deactivating only the homeserver side of a Pasion account leaves Pasion
/// signing the user in to a Matrix account that rejects every request, so
/// Pasion accounts are deactivated in Pasion. Accounts without one, and a
/// Pasion account that is already deactivated there, are fully deactivated on
/// the homeserver (sessions, devices and room memberships).
pub async fn deactivate_account(id: &str, erase: bool) -> Result<AccountOwner, HttpError> {
    if let Some(account) = pasion_account_for(id).await?
        && account.deactivated_at.is_none()
    {
        crate::api::pasion::pasion_update_user(
            &account.id,
            serde_json::json!({ "deactivated": true, "hs_erase": erase }),
        )
        .await?;
        invalidate_user_related_caches();
        return Ok(AccountOwner::Pasion);
    }
    deactivate_user(id, erase).await?;
    Ok(AccountOwner::Homeserver)
}

/// Reactivate a local Matrix account through the service that owns it.
/// Pasion reactivates the homeserver account itself; an account Pasion still
/// considers active (for example one deactivated on the homeserver only) is
/// reactivated on the homeserver directly.
pub async fn reactivate_account(id: &str) -> Result<AccountOwner, HttpError> {
    if let Some(account) = pasion_account_for(id).await?
        && account.deactivated_at.is_some()
    {
        crate::api::pasion::pasion_update_user(
            &account.id,
            serde_json::json!({ "deactivated": false }),
        )
        .await?;
        invalidate_user_related_caches();
        return Ok(AccountOwner::Pasion);
    }
    set_user_deactivated(id, false).await?;
    Ok(AccountOwner::Homeserver)
}

pub async fn get_user_devices(
    user_id: &str,
    page: u64,
    per_page: u64,
) -> Result<ListResponse<DeviceRecord>, HttpError> {
    let from = (page - 1) * per_page;
    let encoded = urlencoding::encode(user_id);
    let from_str = from.to_string();
    let limit_str = per_page.to_string();
    let url = build_url(
        &format!("/_palpo/admin/v2/users/{encoded}/devices"),
        &[("from", &from_str), ("limit", &limit_str)],
    )?;

    #[derive(serde::Deserialize)]
    struct Resp {
        devices: Vec<Device>,
        total: u64,
    }

    let response: Resp = api_client(&url, "GET", None).await?;

    Ok(ListResponse {
        data: response
            .devices
            .into_iter()
            .map(|d| {
                let id = d.device_id.clone();
                DeviceRecord { id, device: d }
            })
            .collect(),
        total: response.total,
    })
}

pub async fn delete_user_device(user_id: &str, device_id: &str) -> Result<(), HttpError> {
    let (path, body) = device_delete_request(user_id, device_id);
    let url = build_url(&path, &[])?;
    let _: serde_json::Value = api_client(&url, "POST", Some(body.to_string())).await?;
    Ok(())
}

pub async fn delete_all_user_devices(user_id: &str) -> Result<u64, HttpError> {
    let devices = get_user_devices(user_id, 1, ALL_DEVICES_LIMIT).await?;
    let deleted = delete_devices_with(
        devices.data.into_iter().map(|device| device.id).collect(),
        |id| async move { delete_user_device(user_id, &id).await },
    )
    .await?;
    // Confirm the postcondition even against older servers that may acknowledge
    // an operation without exposing a lower-level deletion error.
    let remaining = get_user_devices(user_id, 1, ALL_DEVICES_LIMIT).await?;
    if !remaining.data.is_empty() || remaining.total != 0 {
        return Err(HttpError::message(format!(
            "The server still reports {} Matrix devices after removal. Refresh and retry; Pasion grants are managed separately.",
            remaining.total
        )));
    }
    Ok(deleted)
}

fn device_delete_request(user_id: &str, device_id: &str) -> (String, serde_json::Value) {
    // A Device ID is opaque: even percent-encoded dot segments are normalized
    // by browser URL parsing. Keep it in the admin endpoint's JSON body.
    (
        format!(
            "/_palpo/admin/v2/users/{}/delete_devices",
            urlencoding::encode(user_id)
        ),
        serde_json::json!({"devices": [device_id]}),
    )
}

async fn delete_devices_with<F, Fut>(ids: Vec<String>, mut remove: F) -> Result<u64, HttpError>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), HttpError>>,
{
    let total = ids.len();
    let mut deleted = 0u64;
    let mut failed = Vec::new();
    for id in ids {
        match remove(id.clone()).await {
            Ok(()) => deleted += 1,
            Err(error) => failed.push(format!("{id}: {}", error.message)),
        }
    }
    if !failed.is_empty() {
        return Err(HttpError::message(format!(
            "Removed {deleted} of {total} Matrix devices. Failed: {}",
            failed.join("; ")
        )));
    }
    Ok(deleted)
}

pub async fn delete_user_threepid(
    user_id: &str,
    medium: &str,
    address: &str,
) -> Result<(), HttpError> {
    // Fetch current user, filter out the target 3PID, PUT back
    let user = get_user(user_id).await?;
    let remaining: Vec<serde_json::Value> = user
        .user
        .threepids
        .iter()
        .filter(|tp| !(tp.medium == medium && tp.address == address))
        .map(|tp| serde_json::json!({ "medium": tp.medium, "address": tp.address }))
        .collect();
    let data = serde_json::json!({ "threepids": remaining });
    update_user(user_id, data).await?;
    Ok(())
}

pub async fn get_user_joined_rooms(
    user_id: &str,
    page: u64,
    per_page: u64,
) -> Result<ListResponse<String>, HttpError> {
    let from = (page - 1) * per_page;
    let encoded = urlencoding::encode(user_id);
    let from_str = from.to_string();
    let limit_str = per_page.to_string();
    let url = build_url(
        &format!("/_palpo/admin/v1/users/{encoded}/joined_rooms"),
        &[("from", &from_str), ("limit", &limit_str)],
    )?;

    #[derive(serde::Deserialize)]
    struct Resp {
        joined_rooms: Vec<String>,
        total: u64,
    }

    let response: Resp = api_client(&url, "GET", None).await?;

    Ok(ListResponse {
        data: response.joined_rooms,
        total: response.total,
    })
}

pub async fn check_username_available(username: &str) -> Result<bool, HttpError> {
    let user_id = return_mxid(username);
    let encoded = urlencoding::encode(&user_id);
    let url = build_url(&format!("/_palpo/admin/v2/users/{encoded}"), &[])?;
    match api_client::<User>(&url, "GET", None).await {
        Ok(_) => Ok(false),                    // user exists => not available
        Err(e) if e.status == 404 => Ok(true), // not found => available
        Err(e) => Err(e),
    }
}

pub async fn reset_password(id: &str, new_password: &str) -> Result<(), HttpError> {
    let encoded = urlencoding::encode(id);
    let url = build_url(&format!("/_palpo/admin/v1/reset_password/{encoded}"), &[])?;
    let body = serde_json::json!({
        "new_password": new_password,
        "logout_devices": true,
    })
    .to_string();
    let _: serde_json::Value = api_client(&url, "POST", Some(body)).await?;
    Ok(())
}

pub async fn whois_user(user_id: &str) -> Result<crate::types::Whois, HttpError> {
    let encoded = urlencoding::encode(user_id);
    let url = build_url(&format!("/_palpo/admin/v1/whois/{encoded}"), &[])?;
    let whois: crate::types::Whois = api_client(&url, "GET", None).await?;
    Ok(whois)
}

pub async fn send_server_notice(user_id: &str, body: &str) -> Result<String, HttpError> {
    let mxid = return_mxid(user_id);
    let url = build_url("/_palpo/admin/v1/send_server_notice", &[])?;
    let req_body = serde_json::json!({
        "user_id": mxid,
        "content": {
            "msgtype": "m.text",
            "body": body,
        }
    })
    .to_string();

    #[derive(serde::Deserialize)]
    struct Resp {
        event_id: String,
    }

    let response: Resp = api_client(&url, "POST", Some(req_body)).await?;
    Ok(response.event_id)
}

/// Account-wide moderation uses the dedicated vendor administration endpoints.
pub async fn set_shadow_banned(user: &str, banned: bool) -> Result<(), HttpError> {
    let encoded = urlencoding::encode(user);
    let url = build_url(&format!("/_palpo/admin/v1/users/{encoded}/shadow_ban"), &[])?;
    let _: serde_json::Value =
        api_client(&url, if banned { "POST" } else { "DELETE" }, None).await?;
    invalidate_user_related_caches();
    Ok(())
}

pub async fn set_suspended(user: &str, suspended: bool) -> Result<(), HttpError> {
    super::admin_capabilities::require("user_suspended").await?;
    let encoded = urlencoding::encode(user);
    let url = build_url(&format!("/_palpo/admin/v1/suspend/{encoded}"), &[])?;
    let _: serde_json::Value = api_client(
        &url,
        "PUT",
        Some(serde_json::json!({"suspend": suspended}).to_string()),
    )
    .await?;
    invalidate_user_related_caches();
    Ok(())
}

#[cfg(test)]
mod device_deletion_tests {
    use super::*;
    #[test]
    fn partial_failure_is_reported_and_later_devices_are_still_attempted() {
        let seen = std::cell::RefCell::new(Vec::new());
        let result = futures::executor::block_on(delete_devices_with(
            vec!["one".into(), "failed".into(), "three".into()],
            |id| {
                seen.borrow_mut().push(id.clone());
                std::future::ready(if id == "failed" {
                    Err(HttpError::message("denied"))
                } else {
                    Ok(())
                })
            },
        ));
        let error = result.unwrap_err();
        assert!(error.message.contains("Removed 2 of 3"));
        assert!(error.message.contains("failed: denied"));
        assert_eq!(seen.into_inner(), vec!["one", "failed", "three"]);
    }
    #[test]
    fn opaque_device_ids_are_json_data_and_never_url_path_segments() {
        for id in [
            ".",
            "..",
            "/",
            "?",
            "#",
            "%2e%2e",
            "opaque/with?query#fragment",
            "quote\"\\设备",
        ] {
            let (path, body) = device_delete_request("@user:server", id);
            assert_eq!(
                path,
                "/_palpo/admin/v2/users/%40user%3Aserver/delete_devices"
            );
            let wire: serde_json::Value = serde_json::from_str(&body.to_string()).unwrap();
            assert_eq!(wire, serde_json::json!({"devices": [id]}));
        }
    }
}
