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

use futures::{
    FutureExt,
    future::{LocalBoxFuture, Shared},
};
type Probe = Shared<LocalBoxFuture<'static, Result<Session, HttpError>>>;
thread_local! {
    static SESSION: RefCell<Option<(u64, Session)>> = const { RefCell::new(None) };
    static PROBE: RefCell<Option<(u64, bool, Probe)>> = const { RefCell::new(None) };
}

pub fn has_session() -> bool {
    SESSION.with(|v| {
        v.borrow()
            .as_ref()
            .is_some_and(|(generation, _)| *generation == storage::session_generation())
    })
}

pub fn take_session() -> Option<Session> {
    SESSION.with(|v| {
        v.borrow_mut().take().and_then(|(generation, session)| {
            (generation == storage::session_generation()).then_some(session)
        })
    })
}

pub fn clear_session() {
    SESSION.with(|v| *v.borrow_mut() = None);
    PROBE.with(|v| *v.borrow_mut() = None);
}

pub async fn bind_current_token() -> Result<Session, HttpError> {
    probe_session(true).await
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
    probe_session(false).await
}

async fn probe_session(bind: bool) -> Result<Session, HttpError> {
    if !storage::session_state().allows_browser_session() {
        return Err(changed_session());
    }
    // An explicit new OAuth login must establish its own cookie before any GET
    // can accept the previous account's still-live HttpOnly cookie.
    let bind =
        bind || (!has_session() && storage::session_state().snapshot().access_token.is_some());
    let generation = storage::session_generation();
    let existing = PROBE.with(|p| {
        p.borrow()
            .as_ref()
            .filter(|(g, force, _)| *g == generation && *force == bind)
            .map(|(_, _, f)| f.clone())
    });
    let probe = if let Some(existing) = existing {
        existing
    } else {
        let future = async move {
            let result = probe_session_once(bind, generation).await;
            PROBE.with(|p| {
                if p.borrow()
                    .as_ref()
                    .is_some_and(|(g, force, _)| *g == generation && *force == bind)
                {
                    *p.borrow_mut() = None;
                }
            });
            result
        }
        .boxed_local()
        .shared();
        PROBE.with(|p| *p.borrow_mut() = Some((generation, bind, future.clone())));
        future
    };
    probe.await
}

fn changed_session() -> HttpError {
    HttpError::from_status(401, "The browser session changed. Sign in again.")
}

async fn probe_session_once(bind: bool, generation: u64) -> Result<Session, HttpError> {
    let value = if bind {
        None
    } else {
        match fetch("/hagency/api/session", "GET", None, None, None).await {
            Ok(value) => Some(value),
            Err(e) if e.status == 401 => None,
            Err(e) => return Err(e),
        }
    };
    if storage::session_generation() != generation {
        return Err(changed_session());
    }
    let value = match value {
        Some(value) => value,
        None => {
            crate::api::auth::authenticated_request(|snapshot| async move {
                let token = snapshot
                    .access_token
                    .ok_or_else(|| HttpError::from_status(401, "Sign in first."))?;
                fetch(
                    "/hagency/api/login/token",
                    "POST",
                    Some(json!({})),
                    None,
                    Some(&token),
                )
                .await
            })
            .await?
        }
    };
    if storage::session_generation() != generation {
        return Err(changed_session());
    }
    let session: Session =
        serde_json::from_value(value).map_err(|e| HttpError::message(e.to_string()))?;
    if !crate::api::auth::accept_session_actor(generation, &session.user_id) {
        return Err(changed_session());
    }
    storage::set_item("user_id", &session.user_id);
    SESSION.with(|v| *v.borrow_mut() = Some((generation, session.clone())));
    Ok(session)
}

pub async fn call(path: &str, method: &str, body: Option<Value>) -> Result<Value, HttpError> {
    let generation = storage::session_generation();
    let cached = SESSION.with(|v| {
        v.borrow()
            .as_ref()
            .filter(|(g, _)| *g == generation)
            .map(|(_, s)| s.clone())
    });
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
    if storage::session_generation() != generation {
        return Err(changed_session());
    }
    if matches!(&result, Err(error) if error.status == 403) {
        // Read solely to compare with the actor captured at call entry. Another
        // tab may have changed both the shared cookie and localStorage identity.
        if let Ok(value) = fetch("/hagency/api/session", "GET", None, None, None).await
            && let Ok(observed) = serde_json::from_value::<Session>(value)
            && !crate::api::auth::check_session_actor(
                generation,
                &session.user_id,
                &observed.user_id,
            )
        {
            return Err(changed_session());
        }
        if storage::session_generation() != generation {
            return Err(changed_session());
        }
    }
    if matches!(&result, Err(e) if e.status == 401) {
        // A late 401 must not discard a replacement session from another call.
        SESSION.with(|v| {
            if v.borrow()
                .as_ref()
                .is_some_and(|(g, s)| *g == generation && s.csrf == session.csrf)
            {
                *v.borrow_mut() = None;
            }
        });
        let original_actor = session.user_id;
        let session = ensure_session().await?;
        if !crate::api::auth::check_session_actor(generation, &original_actor, &session.user_id) {
            return Err(changed_session());
        }
        let rejected = storage::session_state().snapshot();
        let result = fetch(
            &format!("/hagency/api/browser/v1{path}"),
            method,
            body,
            Some(&session.csrf),
            None,
        )
        .await;
        if storage::session_generation() != generation {
            return Err(changed_session());
        }
        if matches!(&result, Err(error) if error.status == 401) {
            crate::api::auth::reject_session(&rejected);
        }
        return result;
    }
    result
}

pub async fn logout_session(csrf: &str, generation: u64) {
    // Only the original tab's captured CSRF can revoke its Hagency session.
    // Reading a replacement shared cookie here could log out another account.
    if storage::session_generation() == generation {
        let _ = fetch(
            "/hagency/api/logout",
            "POST",
            Some(json!({})),
            Some(csrf),
            None,
        )
        .await;
    }
}

pub fn operation_id() -> String {
    format!(
        "h_{}_{}",
        js_sys::Date::now() as u64,
        crate::api::client::generate_request_id()
    )
}
