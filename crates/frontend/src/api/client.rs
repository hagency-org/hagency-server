use gloo_net::http::{Request, RequestBuilder};
use serde::de::DeserializeOwned;

use crate::utils::error::{HttpError, MatrixError, display_error};
use crate::utils::perf;
use crate::utils::storage;

/// Generate a short 8-character hex request ID from the current timestamp and a random value.
pub fn generate_request_id() -> String {
    let now = js_sys::Date::now() as u64;
    let rand = (js_sys::Math::random() * 0xFFFF as f64) as u64;
    let combined = now.wrapping_mul(31).wrapping_add(rand);
    format!("{:08x}", combined & 0xFFFF_FFFF)
}

/// Shared raw fetch primitive used by every backend client.
///
/// Handles header construction, body serialization, 204/4xx responses,
/// latency tracking, and JSON parsing. The caller supplies `format_error`,
/// which is invoked on any `status >= 400` to produce a (message, body)
/// tuple matching that backend's error envelope (Matrix errcode vs. Pasion
/// JSON:API text). 401 retry is the caller's responsibility — layer it on
/// top of this primitive.
pub async fn raw_fetch<T, F>(
    url: &str,
    method: &str,
    body: Option<String>,
    format_error: F,
) -> Result<T, HttpError>
where
    T: DeserializeOwned,
    F: Fn(u16, &str) -> (String, Option<MatrixError>),
{
    let start_time = js_sys::Date::now();

    let rid = Some(generate_request_id());
    let token = storage::get_item("access_token");

    let mut builder: RequestBuilder = match method {
        "POST" => Request::post(url),
        "PUT" => Request::put(url),
        "PATCH" => Request::patch(url),
        "DELETE" => Request::delete(url),
        _ => Request::get(url),
    }
    .header("Accept", "application/json");

    if let Some(ref token) = token {
        builder = builder.header("Authorization", &format!("Bearer {token}"));
    }

    if body.is_some() {
        builder = builder.header("Content-Type", "application/json");
    }

    let request = if let Some(body) = body {
        builder.body(body).map_err(|e| HttpError {
            message: e.to_string(),
            status: 0,
            body: None,
            request_id: rid.clone(),
        })?
    } else {
        builder.build().map_err(|e| HttpError {
            message: e.to_string(),
            status: 0,
            body: None,
            request_id: rid.clone(),
        })?
    };

    let response = request.send().await.map_err(|e| HttpError {
        message: e.to_string(),
        status: 0,
        body: None,
        request_id: rid.clone(),
    })?;

    let status = response.status();
    let duration_ms = js_sys::Date::now() - start_time;
    perf::record_api_call(duration_ms);

    if status == 204 {
        // Return default for 204 No Content
        let empty = serde_json::from_str::<T>("{}").or_else(|_| serde_json::from_str::<T>("null"));
        return empty.map_err(|e| HttpError {
            message: e.to_string(),
            status,
            body: None,
            request_id: rid.clone(),
        });
    }

    let text = response.text().await.map_err(|e| HttpError {
        message: e.to_string(),
        status,
        body: None,
        request_id: rid.clone(),
    })?;

    if status >= 400 {
        let (message, error_body) = format_error(status, &text);
        return Err(HttpError {
            message,
            status,
            body: error_body,
            request_id: rid,
        });
    }

    serde_json::from_str(&text).map_err(|e| HttpError {
        message: format!("JSON parse error: {e}"),
        status,
        body: None,
        request_id: rid,
    })
}

/// Format a Matrix-style error body (`{"errcode": "...", "error": "..."}`).
pub fn format_matrix_error(status: u16, text: &str) -> (String, Option<MatrixError>) {
    let error_body: Option<MatrixError> = serde_json::from_str(text).ok();
    let message = if let Some(ref eb) = error_body {
        display_error(&eb.errcode, status, eb.error.as_deref().unwrap_or(""))
    } else {
        display_error("M_INVALID", status, text)
    };
    (message, error_body)
}

pub async fn api_client<T: DeserializeOwned>(
    url: &str,
    method: &str,
    body: Option<String>,
) -> Result<T, HttpError> {
    let result = raw_fetch::<T, _>(url, method, body.clone(), format_matrix_error).await;

    // Any 401 means "this token isn't valid" — the specific errcode
    // varies by backend (palpo uses `M_UNKNOWN_TOKEN`, pasion uses a
    // JSON:API envelope). Try to refresh and retry once; if that
    // fails, the shared handler clears session state and AppLayout's
    // heartbeat redirects to the login page on its next tick.
    if let Err(ref err) = result {
        if err.status == 401 && crate::api::auth::handle_unauthorized().await {
            return raw_fetch::<T, _>(url, method, body, format_matrix_error).await;
        }
        if err.status == 403 {
            crate::api::auth::handle_forbidden();
        }
    }

    result
}

pub fn get_base_url() -> Result<String, HttpError> {
    // The Rust host mounts all component APIs on the same origin.
    // Return empty string so paths like "/_palpo/admin/..." are relative.
    Ok(String::new())
}

pub fn get_home_server() -> Result<String, HttpError> {
    storage::get_item("home_server")
        .ok_or_else(|| HttpError::message("Home server not set. Please log in first."))
}

pub fn build_url(path: &str, params: &[(&str, &str)]) -> Result<String, HttpError> {
    let mut url = path.to_string();

    let query_params: Vec<String> = params
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect();

    if !query_params.is_empty() {
        url.push('?');
        url.push_str(&query_params.join("&"));
    }

    Ok(url)
}

pub fn get_search_order(order: &str) -> &'static str {
    match order.to_lowercase().as_str() {
        "desc" => "b",
        _ => "f",
    }
}
