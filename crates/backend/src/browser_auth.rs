//! Browser-only authentication bridge; no Hagency domain storage or enrollment.
use crate::config::Config;
use salvo::{http::header, prelude::*};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use url::Url;

#[derive(Debug)]
struct AuthError {
    status: u16,
    code: &'static str,
    message: String,
}
type Result<T> = std::result::Result<T, AuthError>;
fn err(status: u16, code: &'static str, message: &'static str) -> AuthError {
    AuthError {
        status,
        code,
        message: message.into(),
    }
}
fn millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn secret() -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}
#[derive(Clone)]
struct Session {
    token: String,
    user_id: String,
    csrf: String,
    expires: i64,
}
#[derive(Clone)]
pub struct BrowserAuth {
    client: reqwest::Client,
    internal_origin: Url,
    public_origin: Url,
    server_name: String,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    rates: Arc<Mutex<HashMap<String, (u32, i64)>>>,
    handover: Arc<Mutex<()>>,
    session_ttl: i64,
    read_timeout: Duration,
    oauth_revocation: Option<(Url, String)>,
}
impl BrowserAuth {
    pub async fn new(conf: &Config) -> anyhow::Result<Self> {
        anyhow::ensure!(
            conf.pasion.as_ref().is_some_and(|p| p.delegate_matrix_auth),
            "Pasion authorization is required"
        );
        let ttl = conf.session_ttl_ms.unwrap_or(1_800_000);
        anyhow::ensure!(
            (1_000..=86_400_000).contains(&ttl),
            "browser session TTL must be between one second and one day"
        );
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(12))
                .build()?,
            internal_origin: conf.internal_origin(),
            public_origin: conf.public_origin.clone(),
            server_name: conf.matrix.server_name.to_string(),
            sessions: Default::default(),
            rates: Default::default(),
            handover: Default::default(),
            session_ttl: ttl as i64,
            read_timeout: Duration::from_millis(conf.read_timeout_ms.unwrap_or(8_000)),
            oauth_revocation: Some((
                conf.internal_origin().join("auth/oauth2/revoke")?,
                conf.matrix
                    .admin
                    .mas_secret
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("missing Pasion shared secret"))?,
            )),
        })
    }
    pub fn router(&self) -> Router {
        Router::new()
            .push(Router::with_path("hagency/api/login/token").post(self.clone()))
            .push(Router::with_path("hagency/api/session").get(self.clone()))
            .push(Router::with_path("hagency/api/logout").post(self.clone()))
            .push(Router::with_path("hagency/api/browser/v1/{**path}").goal(self.clone()))
    }
    async fn matrix(&self, path: &str, token: &str, post: bool) -> Result<Value> {
        let url = self
            .internal_origin
            .join(path)
            .map_err(|_| err(500, "invalid_upstream", "Invalid configured upstream."))?;
        let req = if post {
            self.client.post(url).json(&json!({}))
        } else {
            self.client.get(url)
        };
        let response = req
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| err(502, "upstream_unreachable", "Matrix did not respond."))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(err(
                status,
                "upstream_error",
                "Matrix authorization request failed.",
            ));
        }
        response.json().await.map_err(|_| {
            err(
                502,
                "upstream_invalid",
                "Matrix returned an invalid response.",
            )
        })
    }
    async fn identity(&self, token: &str) -> Result<(String, bool)> {
        let me = self
            .matrix("/_matrix/client/v3/account/whoami", token, false)
            .await?;
        let user = me["user_id"]
            .as_str()
            .ok_or_else(|| err(502, "identity_invalid", "Matrix returned no identity."))?;
        let (local, server) = user.split_once(':').unwrap_or(("", ""));
        if me["is_guest"] == true
            || !local.starts_with('@')
            || local.len() < 2
            || local.starts_with("@_hagency_")
            || server != self.server_name
            || user.len() > 255
            || user.chars().any(char::is_whitespace)
        {
            return Err(err(
                403,
                "identity_mismatch",
                "A personal local Matrix identity is required.",
            ));
        }
        let admin = match self
            .matrix("/_palpo/admin/v1/appservices", token, false)
            .await
        {
            Ok(_) => true,
            Err(e) if e.status == 403 => false,
            Err(e) => return Err(e),
        };
        Ok((user.into(), admin))
    }
    async fn rate(&self, key: String, max: u32, window: i64) -> Result<()> {
        let mut rates = self.rates.lock().await;
        rates.retain(|_, (_, until)| *until > millis());
        if rates.len() >= 10_000 && !rates.contains_key(&key) {
            return Err(err(429, "rate_limit", "Too many authorization requests."));
        }
        let rate = rates.entry(key).or_insert((0, millis() + window));
        rate.0 += 1;
        if rate.0 > max {
            Err(err(
                429,
                "rate_limit",
                "Too many attempts. Try again later.",
            ))
        } else {
            Ok(())
        }
    }
    fn cookie(&self, id: &str, clear: bool) -> String {
        format!(
            "palpo_admin={id}; Path=/; HttpOnly; SameSite=Strict{}; Max-Age={}",
            if self.public_origin.scheme() == "https" {
                "; Secure"
            } else {
                ""
            },
            if clear { 0 } else { self.session_ttl / 1000 }
        )
    }
    async fn browser_session(
        &self,
        req: &Request,
        res: &mut Response,
        token: String,
        user: String,
        is_admin: bool,
    ) -> Result<Value> {
        let id = secret();
        let csrf = secret();
        let previous = req
            .headers()
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|cookies| {
                cookies
                    .split(';')
                    .find_map(|p| p.trim().strip_prefix("palpo_admin="))
            });
        let old = if let Some(previous) = previous {
            self.sessions.lock().await.get(previous).cloned()
        } else {
            None
        };
        if let (Some((endpoint, shared_secret)), Some(old)) = (&self.oauth_revocation, old)
            && old.token != token
        {
            // Revoke only after the replacement token is fully verified. Calling
            // Matrix logout here would delete a device shared by both grants.
            let revoked = self
                .client
                .post(endpoint.clone())
                .timeout(Duration::from_secs(10))
                .bearer_auth(shared_secret)
                .form(&[
                    ("token", old.token),
                    ("token_type_hint", "access_token".into()),
                ])
                .send()
                .await
                .map_err(|_| {
                    err(
                        502,
                        "authorization_handover_failed",
                        "Could not replace your previous authorization. Retry sign-in.",
                    )
                })?;
            if !revoked.status().is_success() {
                return Err(err(
                    502,
                    "authorization_handover_failed",
                    "Could not replace your previous authorization. Retry sign-in.",
                ));
            }
        }
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, s| s.expires > millis());
        // Replace the cookie session instead of leaving a previous identity active.
        if let Some(previous) = previous {
            sessions.remove(previous);
        }
        if sessions.len() >= 10_000 {
            return Err(err(
                503,
                "session_capacity",
                "Browser session capacity is exhausted.",
            ));
        }
        sessions.insert(
            id.clone(),
            Session {
                token,
                user_id: user.clone(),
                csrf: csrf.clone(),
                expires: millis() + self.session_ttl,
            },
        );
        drop(sessions);
        res.add_header(header::SET_COOKIE, self.cookie(&id, false), true)
            .unwrap();
        Ok(json!({"userId":user,"csrf":csrf,"isAdmin":is_admin,"serverName":self.server_name}))
    }
    async fn native(
        &self,
        path: &str,
        method: reqwest::Method,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Value> {
        let url = self
            .internal_origin
            .join(path)
            .map_err(|_| err(500, "invalid_upstream", "Invalid configured service route."))?;
        let host = &self.public_origin[url::Position::BeforeHost..url::Position::AfterPort];
        let mut request = self.client.request(method, url).header("Host", host);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| err(502, "service_unreachable", "Agent service did not respond."))?;
        let status = response.status().as_u16();
        let body: Value = response.json().await.map_err(|_| {
            err(
                502,
                "service_invalid",
                "Agent service returned an invalid response.",
            )
        })?;
        if !(200..300).contains(&status) {
            return Err(AuthError {
                status,
                code: "service_error",
                message: body["error"]
                    .as_str()
                    .or_else(|| body["code"].as_str())
                    .unwrap_or("Agent service rejected the operation.")
                    .chars()
                    .take(512)
                    .collect(),
            });
        }
        Ok(body)
    }
    async fn browser_domain(&self, req: &mut Request, session: &Session) -> Result<Value> {
        let relative = req
            .uri()
            .path()
            .strip_prefix("/hagency/api/browser/v1/")
            .ok_or_else(|| err(404, "not_found", "Unknown browser operation."))?;
        if !allowed_operation(req.method().as_str(), relative) || req.uri().query().is_some() {
            return Err(err(404, "not_found", "Unknown browser operation."));
        }
        if req.method() != salvo::http::Method::GET
            && !same(
                req.headers()
                    .get("x-csrf-token")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(""),
                &session.csrf,
            )
        {
            return Err(err(403, "csrf_forbidden", "Refresh your browser session."));
        }
        let path = format!("/hagency/api/v1/{relative}");
        let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes())
            .map_err(|_| err(400, "invalid_method", "Unsupported operation."))?;
        let body = if req.method() == salvo::http::Method::GET
            || req.method() == salvo::http::Method::DELETE
        {
            None
        } else if relative.ends_with("/pause-service") || relative.ends_with("/clear-service-pause")
        {
            req.set_secure_max_size(1024);
            if !req
                .payload()
                .await
                .map_err(|_| err(400, "invalid_arguments", "Empty body required."))?
                .is_empty()
            {
                return Err(err(400, "invalid_arguments", "Empty body required."));
            }
            None
        } else {
            if req
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                != Some("application/json")
            {
                return Err(err(415, "json_required", "Use application/json."));
            }
            req.set_secure_max_size(16_384);
            let bytes = req
                .payload()
                .await
                .map_err(|_| err(413, "body_too_large", "Request body is too large."))?;
            let value: Value = serde_json::from_slice(bytes)
                .map_err(|_| err(400, "invalid_arguments", "A JSON object is required."))?;
            if !value.is_object() {
                return Err(err(400, "invalid_arguments", "A JSON object is required."));
            }
            Some(value)
        };
        // The verifier derives subject/MXID from this stored personal OAuth grant.
        // The browser cannot submit another user's identity or a service credential.
        let grant = self
            .native(
                "/hagency/api/v1/sessions/pasion",
                reqwest::Method::POST,
                None,
                Some(&json!({"accessToken":session.token})),
            )
            .await?;
        let token = grant["token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                err(
                    502,
                    "service_invalid",
                    "Agent service returned no session credential.",
                )
            })?;
        if grant["mxid"] != session.user_id {
            let _ = self
                .native(
                    "/hagency/api/v1/sessions/current",
                    reqwest::Method::DELETE,
                    Some(token),
                    None,
                )
                .await;
            return Err(err(
                403,
                "identity_mismatch",
                "Authorization identity changed.",
            ));
        }
        if session.expires <= millis() {
            let _ = self
                .native(
                    "/hagency/api/v1/sessions/current",
                    reqwest::Method::DELETE,
                    Some(token),
                    None,
                )
                .await;
            return Err(err(
                401,
                "sign_in_required",
                "Your browser session expired.",
            ));
        }
        let this = self.clone();
        let token = token.to_owned();
        // Once issued, authorization cleanup survives a cancelled browser request.
        tokio::spawn(async move {
            let result = this
                .native(&path, method, Some(&token), body.as_ref())
                .await;
            if this
                .native(
                    "/hagency/api/v1/sessions/current",
                    reqwest::Method::DELETE,
                    Some(&token),
                    None,
                )
                .await
                .is_err()
            {
                tracing::warn!("browser authorization cleanup deferred to short session expiry");
            }
            result
        })
        .await
        .map_err(|_| {
            err(
                502,
                "service_unavailable",
                "Agent operation was interrupted.",
            )
        })?
    }
    async fn body(req: &mut Request) -> Result<()> {
        if req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            != Some("application/json")
        {
            return Err(err(415, "json_required", "Use application/json."));
        }
        req.set_secure_max_size(16_384);
        let bytes = req
            .payload()
            .await
            .map_err(|_| err(413, "body_too_large", "Request body is too large."))?;
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|_| err(400, "invalid_json", "An empty JSON object is required."))?;
        if value.as_object().is_none_or(|v| !v.is_empty()) {
            return Err(err(
                400,
                "invalid_arguments",
                "An empty JSON object is required.",
            ));
        }
        Ok(())
    }
    async fn route(&self, req: &mut Request, res: &mut Response) -> Result<Value> {
        let path = req.uri().path().to_owned();
        if req.headers().get_all(header::HOST).iter().count() != 1
            || req.headers().get_all(header::AUTHORIZATION).iter().count() > 1
            || req.headers().get_all(header::ORIGIN).iter().count() > 1
            || req.headers().contains_key("x-forwarded-host")
        {
            return Err(err(
                403,
                "origin_forbidden",
                "Use the configured browser origin.",
            ));
        }
        // The listener may be reached through its internal origin, but browser requests
        // must name the configured public host. Never trust forwarded Host/Origin.
        let expected =
            self.public_origin[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
        if req
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            != Some(expected.as_str())
        {
            return Err(err(
                403,
                "host_forbidden",
                "Use the configured public host.",
            ));
        }
        if req.headers().get(header::ORIGIN).is_some_and(|v| {
            v.to_str().ok() != Some(self.public_origin.origin().ascii_serialization().as_str())
        }) {
            return Err(err(
                403,
                "origin_forbidden",
                "Cross-origin authorization is forbidden.",
            ));
        }
        if path == "/hagency/api/login/token" && req.method() == salvo::http::Method::POST {
            self.rate(format!("login:{}", req.remote_addr()), 120, 900_000)
                .await?;
            Self::body(req).await?;
            let token = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .filter(|s| !s.is_empty() && s.len() <= 16_384)
                .ok_or_else(|| {
                    err(
                        401,
                        "sign_in_required",
                        "A Matrix access token is required.",
                    )
                })?
                .to_owned();
            let _guard = self.handover.lock().await;
            let (user, admin) = self.identity(&token).await?;
            return self.browser_session(req, res, token, user, admin).await;
        }
        let sid = req
            .headers()
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .find_map(|v| v.trim().strip_prefix("palpo_admin="))
            .unwrap_or("")
            .to_owned();
        let session = {
            let mut sessions = self.sessions.lock().await;
            sessions.retain(|_, v| v.expires > millis());
            sessions
                .get(&sid)
                .cloned()
                .ok_or_else(|| err(401, "sign_in_required", "Sign in again."))?
        };
        if path == "/hagency/api/logout" && req.method() == salvo::http::Method::POST {
            let _guard = self.handover.lock().await;
            if !same(
                req.headers()
                    .get("x-csrf-token")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(""),
                &session.csrf,
            ) {
                return Err(err(403, "csrf_forbidden", "Refresh your browser session."));
            }
            Self::body(req).await?;
            // A revoked Matrix token must not keep the browser cookie alive.
            let result = self
                .matrix("/_matrix/client/v3/logout", &session.token, true)
                .await;
            self.sessions.lock().await.remove(&sid);
            res.add_header(header::SET_COOKIE, self.cookie("", true), true)
                .unwrap();
            match result {
                Ok(_) => {}
                Err(e) if e.status == 401 => {}
                Err(e) => return Err(e),
            }
            return Ok(json!({}));
        }
        if path.starts_with("/hagency/api/browser/v1/") {
            let (user, _) = self.identity(&session.token).await?;
            if user != session.user_id
                || self
                    .sessions
                    .lock()
                    .await
                    .get(&sid)
                    .is_none_or(|s| s.expires <= millis())
            {
                return Err(err(
                    401,
                    "sign_in_required",
                    "Your browser session expired.",
                ));
            }
            return self.browser_domain(req, &session).await;
        }
        if path != "/hagency/api/session" || req.method() != salvo::http::Method::GET {
            return Err(err(
                404,
                "not_found",
                "Unknown browser authentication endpoint.",
            ));
        }
        let (user, admin) = match self.identity(&session.token).await {
            Ok(value) => value,
            Err(e) => {
                if e.status == 401 || e.status == 403 {
                    self.sessions.lock().await.remove(&sid);
                }
                return Err(e);
            }
        };
        if user != session.user_id {
            self.sessions.lock().await.remove(&sid);
            return Err(err(
                403,
                "identity_mismatch",
                "The Matrix identity changed.",
            ));
        }
        if self
            .sessions
            .lock()
            .await
            .get(&sid)
            .is_none_or(|s| s.expires <= millis())
        {
            return Err(err(
                401,
                "sign_in_required",
                "Your browser session expired.",
            ));
        }
        Ok(json!({"userId":user,"csrf":session.csrf,"isAdmin":admin,"serverName":self.server_name}))
    }
}
#[async_trait]
impl Handler for BrowserAuth {
    async fn handle(
        &self,
        req: &mut Request,
        _depot: &mut Depot,
        res: &mut Response,
        _ctrl: &mut FlowCtrl,
    ) {
        res.add_header("cache-control", "no-store", true).unwrap();
        res.add_header("x-content-type-options", "nosniff", true)
            .unwrap();
        res.add_header("referrer-policy", "no-referrer", true)
            .unwrap();
        let result = tokio::time::timeout(self.read_timeout, self.route(req, res))
            .await
            .unwrap_or_else(|_| {
                Err(err(
                    504,
                    "authorization_timeout",
                    "Authorization verification timed out.",
                ))
            });
        match result {
            Ok(value) => {
                res.status_code(StatusCode::OK);
                res.render(Json(value));
            }
            Err(e) => {
                res.status_code(StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY));
                res.render(Json(json!({"code":e.code,"error":e.message})));
            }
        }
    }
}

/// A fixed capability list; never a generic authenticated HTTP proxy.
fn allowed_operation(method: &str, path: &str) -> bool {
    let parts = path.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        if parts.len() == 5 && parts[0] == "projects" && parts[2] == "rooms" && index == 3 {
            let Ok(room) = percent_encoding::percent_decode_str(part).decode_utf8() else {
                return false;
            };
            if !room.starts_with('!')
                || !room.contains(':')
                || room.len() > 512
                || room.chars().any(|c| {
                    c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | '?' | '#' | '%')
                })
            {
                return false;
            }
        } else if part.is_empty()
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return false;
        }
    }
    match parts.as_slice() {
        ["projects"] => method == "GET",
        ["projects", "adopt"] => method == "POST",
        ["projects", _, "creation-policy"] => method == "PUT",
        ["projects", _, "service-state"] => method == "GET",
        ["projects", _, "rooms"] => method == "GET",
        ["projects", _, "rooms", _, "agents"] => method == "GET",
        ["projects", _, "pause-service" | "clear-service-pause"] => method == "POST",
        ["projects", _, "rooms", _, "creation-policy"] => method == "PUT",
        ["projects", _, "rooms", _, "service-state"] => method == "GET",
        [
            "projects",
            _,
            "rooms",
            _,
            "pause-service" | "clear-service-pause",
        ] => method == "POST",
        ["projects", _, "rooms", "adopt"] => method == "POST",
        ["devices"] => method == "GET",
        ["agents", _, "execution-device"] => method == "PUT",
        ["agents", _, "owner-direct"] => method == "GET" || method == "POST",
        ["agents"] => method == "GET" || method == "POST",
        ["agents", _] => method == "GET" || method == "DELETE",
        ["agents", _, "bindings"] => method == "GET" || method == "POST",
        ["agents", _, "pause" | "resume"] => method == "POST",
        ["bindings", _] => method == "GET" || method == "DELETE",
        ["bindings", _, "pause" | "resume"] => method == "POST",
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_browser_capabilities() {
        for (method, path) in [
            ("GET", "projects/p_1/rooms"),
            ("GET", "projects/p_1/rooms/%21r%3Aserver/agents"),
            ("GET", "projects/p_1/service-state"),
            ("POST", "projects/p_1/pause-service"),
            ("POST", "projects/p_1/clear-service-pause"),
            ("PUT", "projects/p_1/rooms/%21r%3Aserver/creation-policy"),
            ("GET", "projects/p_1/rooms/%21r%3Aserver/service-state"),
            ("POST", "projects/p_1/rooms/%21r%3Aserver/pause-service"),
            (
                "POST",
                "projects/p_1/rooms/%21r%3Aserver/clear-service-pause",
            ),
            ("GET", "devices"),
            ("PUT", "agents/a_1/execution-device"),
            ("GET", "agents/a_1/owner-direct"),
            ("POST", "agents/a_1/owner-direct"),
            ("GET", "agents"),
            ("POST", "agents"),
            ("PUT", "projects/p_1/creation-policy"),
            ("DELETE", "bindings/b_1"),
            ("POST", "projects/p_1/rooms/adopt"),
        ] {
            assert!(allowed_operation(method, path));
        }
        for path in [
            "projects/p_1/rooms/%21r%2Fdevices%3As/pause-service",
            "projects/p_1/rooms/%2521r%253As/pause-service",
            "projects/p_1/rooms/%21r%3As/execution",
            "fleets",
            "operations/call",
            "execution/events",
            "devices",
            "sessions/pasion",
            "agents/../devices",
            "agents/a%2Fdevices",
            "agents//pause",
            "projects/p_1/creation-policy/extra",
        ] {
            assert!(!allowed_operation("POST", path));
        }
        assert!(!allowed_operation("POST", "agents/a_1"));
        assert!(!allowed_operation("POST", "agents/a_1/execution-device"));
        assert!(!allowed_operation("PUT", "agents/a_1/owner-direct"));
        assert!(!allowed_operation("GET", "devices/d_1/token"));
        assert!(!allowed_operation(
            "POST",
            "agents/a_1/execution-device/extra"
        ));
    }
    async fn fixture() -> (
        BrowserAuth,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let admin = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let current = admin.clone();
        let worker = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let admin = current.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    loop {
                        let mut chunk = [0; 4096];
                        let n = stream.read(&mut chunk).await.unwrap();
                        if n == 0 {
                            break;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                        assert!(raw.len() <= 32_768);
                        if let Some(end) = raw.windows(4).position(|v| v == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&raw[..end]);
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            if raw.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    let request = String::from_utf8_lossy(&raw);
                    let (status, body) = if request.contains("/_matrix/client/v3/account/whoami") {
                        (
                            200,
                            json!({"user_id":"@owner:example.test","is_guest":false}),
                        )
                    } else if request.contains("/_palpo/admin/v1/appservices") {
                        if admin.load(std::sync::atomic::Ordering::SeqCst) {
                            (200, json!([]))
                        } else {
                            (403, json!({"errcode":"M_FORBIDDEN"}))
                        }
                    } else if request.contains("/sessions/pasion") {
                        assert!(!request.to_ascii_lowercase().contains("cookie:"));
                        assert!(!request.to_ascii_lowercase().contains("origin:"));
                        (
                            200,
                            json!({"token":"a".repeat(64),"mxid":"@owner:example.test"}),
                        )
                    } else if request.contains("/hagency/api/v1/agents") {
                        assert!(request.contains("Bearer "));
                        (200, json!({"agents":[]}))
                    } else {
                        (200, json!({}))
                    };
                    let body = body.to_string();
                    let response = format!(
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        let auth = BrowserAuth {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            internal_origin: origin,
            public_origin: Url::parse("http://example.test/").unwrap(),
            server_name: "example.test".into(),
            sessions: Default::default(),
            rates: Default::default(),
            handover: Default::default(),
            session_ttl: 60_000,
            read_timeout: Duration::from_secs(3),
            oauth_revocation: None,
        };
        (auth, admin, worker)
    }
    #[tokio::test]
    async fn cookie_identity_live_role_and_closed_routes() {
        use salvo::test::{ResponseExt, TestClient};
        let (auth, admin, worker) = fixture().await;
        let service = Service::new(auth.router());
        for path in [
            "api/fleets",
            "api/hafleets",
            "api/operations/call",
            "api/login",
            "api/login/token",
            "api/session",
            "api/logout",
            "api/browser/hagency/v1/agents",
            "_hagency/client/v1/fleets",
            "_palpo/miniapp/v1/call",
        ] {
            let response = TestClient::post(format!("http://example.test/{path}"))
                .json(&json!({}))
                .send(&service)
                .await;
            assert_eq!(response.status_code, Some(StatusCode::NOT_FOUND), "{path}");
        }
        let mut login = TestClient::post("http://example.test/hagency/api/login/token")
            .add_header("host", "example.test", true)
            .add_header("authorization", "Bearer personal-token", true)
            .json(&json!({}))
            .send(&service)
            .await;
        assert_eq!(login.status_code, Some(StatusCode::OK));
        let cookie = login
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        assert!(
            login
                .headers()
                .get(header::SET_COOKIE)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("HttpOnly; SameSite=Strict")
        );
        let session: Value = login.take_json().await.unwrap();
        assert_eq!(session["isAdmin"], true);
        assert!(session.get("outboundAvailable").is_none());
        admin.store(false, std::sync::atomic::Ordering::SeqCst);
        let mut check = TestClient::get("http://example.test/hagency/api/session")
            .add_header("host", "example.test", true)
            .add_header("cookie", &cookie, true)
            .send(&service)
            .await;
        assert_eq!(check.status_code, Some(StatusCode::OK));
        assert_eq!(check.take_json::<Value>().await.unwrap()["isAdmin"], false);
        let mut listing = TestClient::get("http://example.test/hagency/api/browser/v1/agents")
            .add_header("host", "example.test", true)
            .add_header("cookie", &cookie, true)
            .send(&service)
            .await;
        let listing_body = listing.take_json::<Value>().await.unwrap();
        assert_eq!(listing.status_code, Some(StatusCode::OK), "{listing_body}");
        let csrf = TestClient::post("http://example.test/hagency/api/browser/v1/agents")
            .add_header("host", "example.test", true)
            .add_header("cookie", &cookie, true)
            .json(&json!({}))
            .send(&service)
            .await;
        assert_eq!(csrf.status_code, Some(StatusCode::FORBIDDEN));
        let hidden = TestClient::post("http://example.test/hagency/api/browser/v1/devices")
            .add_header("host", "example.test", true)
            .add_header("cookie", &cookie, true)
            .add_header("x-csrf-token", session["csrf"].as_str().unwrap(), true)
            .json(&json!({}))
            .send(&service)
            .await;
        assert_eq!(hidden.status_code, Some(StatusCode::NOT_FOUND));
        let cross = TestClient::post("http://example.test/hagency/api/login/token")
            .add_header("host", "example.test", true)
            .add_header("origin", "https://evil.test", true)
            .json(&json!({}))
            .send(&service)
            .await;
        assert_eq!(cross.status_code, Some(StatusCode::FORBIDDEN));
        let logout = TestClient::post("http://example.test/hagency/api/logout")
            .add_header("host", "example.test", true)
            .add_header("cookie", &cookie, true)
            .add_header("x-csrf-token", session["csrf"].as_str().unwrap(), true)
            .json(&json!({}))
            .send(&service)
            .await;
        assert_eq!(logout.status_code, Some(StatusCode::OK));
        let stale = TestClient::get("http://example.test/hagency/api/session")
            .add_header("host", "example.test", true)
            .add_header("cookie", &cookie, true)
            .send(&service)
            .await;
        assert_eq!(stale.status_code, Some(StatusCode::UNAUTHORIZED));
        worker.abort();
    }
}
