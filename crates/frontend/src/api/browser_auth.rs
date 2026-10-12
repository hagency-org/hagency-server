//! Browser authentication and the restricted Agent management BFF.
use crate::utils::{error::HttpError, storage};
use gloo_net::http::Request;
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::RefCell;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub user_id: String,
    pub csrf: String,
    pub is_admin: bool,
    pub server_name: String,
}

thread_local! { static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) }; }

pub fn has_session() -> bool {
    SESSION.with(|v| v.borrow().is_some())
}

pub fn clear_session() {
    SESSION.with(|v| *v.borrow_mut() = None);
}

pub async fn bind_current_token() -> Result<Session, HttpError> {
    let token =
        storage::get_item("access_token").ok_or_else(|| HttpError::message("Sign in first."))?;
    let value = fetch(
        "/hagency/api/login/token",
        "POST",
        Some(json!({})),
        None,
        Some(&token),
    )
    .await?;
    let session: Session =
        serde_json::from_value(value).map_err(|e| HttpError::message(e.to_string()))?;
    storage::set_item("user_id", &session.user_id);
    SESSION.with(|v| *v.borrow_mut() = Some(session.clone()));
    Ok(session)
}

fn error(status: u16, body: &Value) -> HttpError {
    HttpError {
        status,
        message: body["error"]
            .as_str()
            .unwrap_or("The operation failed.")
            .to_owned(),
        body: None,
        request_id: None,
    }
}

async fn fetch(
    path: &str,
    method: &str,
    body: Option<Value>,
    csrf: Option<&str>,
    token: Option<&str>,
) -> Result<Value, HttpError> {
    let mut request = match method {
        "POST" => Request::post(path),
        "PUT" => Request::put(path),
        "PATCH" => Request::patch(path),
        "DELETE" => Request::delete(path),
        _ => Request::get(path),
    }
    .credentials(web_sys::RequestCredentials::SameOrigin)
    .header("Accept", "application/json");
    if let Some(csrf) = csrf {
        request = request.header("X-CSRF-Token", csrf);
    }
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let request = if let Some(body) = body {
        request
            .header("Content-Type", "application/json")
            .body(body.to_string())
    } else {
        request.build()
    }
    .map_err(|e| HttpError::message(e.to_string()))?;
    let response = request
        .send()
        .await
        .map_err(|e| HttpError::message(e.to_string()))?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|e| HttpError::message(e.to_string()))?;
    if !response.ok() {
        return Err(error(status, &value));
    }
    Ok(value)
}

/// Revalidate the browser session, rebuilding it after a host restart if needed.
pub async fn ensure_session() -> Result<Session, HttpError> {
    let value = match fetch("/hagency/api/session", "GET", None, None, None).await {
        Ok(value) => value,
        Err(e) if e.status == 401 => {
            let token = storage::get_item("access_token").ok_or(e)?;
            fetch(
                "/hagency/api/login/token",
                "POST",
                Some(json!({})),
                None,
                Some(&token),
            )
            .await?
        }
        Err(e) => return Err(e),
    };
    let session: Session =
        serde_json::from_value(value).map_err(|e| HttpError::message(e.to_string()))?;
    if storage::get_item("user_id").is_some_and(|id| id != session.user_id) {
        // An account switch in another tab must not issue operations as that user.
        clear_session();
        return Err(HttpError::message(
            "The browser account changed. Sign in again.",
        ));
    }
    storage::set_item("user_id", &session.user_id);
    SESSION.with(|v| *v.borrow_mut() = Some(session.clone()));
    Ok(session)
}

pub async fn call(path: &str, method: &str, body: Option<Value>) -> Result<Value, HttpError> {
    let cached = SESSION.with(|v| v.borrow().clone());
    let session = match cached {
        Some(s) => s,
        None => ensure_session().await?,
    };
    let result = fetch(
        &format!("/hagency/api/browser/v1{path}"),
        method,
        body.clone(),
        Some(&session.csrf),
        None,
    )
    .await;
    if matches!(&result, Err(e) if e.status == 401) {
        clear_session();
        let session = ensure_session().await?;
        return fetch(
            &format!("/hagency/api/browser/v1{path}"),
            method,
            body,
            Some(&session.csrf),
            None,
        )
        .await;
    }
    result
}

pub async fn logout() {
    if let Ok(session) = ensure_session().await {
        let _ = fetch(
            "/hagency/api/logout",
            "POST",
            Some(json!({})),
            Some(&session.csrf),
            None,
        )
        .await;
    }
    clear_session();
}

pub fn operation_id() -> String {
    format!(
        "h_{}_{}",
        js_sys::Date::now() as u64,
        crate::api::client::generate_request_id()
    )
}
