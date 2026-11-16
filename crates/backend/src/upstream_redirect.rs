//! Pasion's `/auth/api/v1/auth/providers` lists each upstream provider's
//! `authorize_url` as `/upstream/authorize/<id>`, without the `/auth/` mount
//! Pasion is served under here, and its login page links there. Send those
//! browsers on to the mounted path so "Continue with GitHub" works.
// TODO(upstream): drop once Pasion prefixes authorize_url with its URL builder
// (crates/backend/src/handlers/account/auth.rs, `providers`).
use salvo::http::StatusCode;
use salvo::prelude::*;

/// Path under the site root that Pasion's unprefixed links point at.
const UPSTREAM_PATH: &str = "upstream/{**rest}";

/// Where an unprefixed `/upstream/...` request belongs, query included.
#[must_use]
pub fn mounted_location(path: &str, query: Option<&str>) -> String {
    let mount = crate::pasion::MOUNT.trim_end_matches('/');
    match query.filter(|q| !q.is_empty()) {
        Some(query) => format!("{mount}{path}?{query}"),
        None => format!("{mount}{path}"),
    }
}

#[handler]
async fn redirect(req: &mut Request, res: &mut Response) {
    let location = mounted_location(req.uri().path(), req.uri().query());
    res.status_code(StatusCode::TEMPORARY_REDIRECT);
    if res.add_header(salvo::http::header::LOCATION, location, true).is_err() {
        // An unencodable location cannot be redirected to; say so instead.
        res.status_code(StatusCode::BAD_REQUEST);
    }
}

/// Pasion's registration steps (e.g. after "Create account" in Element or a new
/// GitHub sign-up) link to `/auth/auth/register/steps/<id>/finish`: its mount
/// added twice. Strip the duplicate so the step page loads (Nov 15 demo).
// TODO(upstream): drop once Pasion stops double-prefixing step URLs.
const DOUBLED_PATH: &str = "auth/auth/{**rest}";

/// `/auth/auth/x?q` → `/auth/x?q`.
#[must_use]
pub fn undoubled_location(path: &str, query: Option<&str>) -> String {
    let mount = crate::pasion::MOUNT.trim_end_matches('/');
    let single = path.strip_prefix(mount).unwrap_or(path);
    match query.filter(|q| !q.is_empty()) {
        Some(query) => format!("{single}?{query}"),
        None => single.to_owned(),
    }
}

#[handler]
async fn undouble(req: &mut Request, res: &mut Response) {
    let location = undoubled_location(req.uri().path(), req.uri().query());
    res.status_code(StatusCode::TEMPORARY_REDIRECT);
    if res.add_header(salvo::http::header::LOCATION, location, true).is_err() {
        res.status_code(StatusCode::BAD_REQUEST);
    }
}

/// Routes `/auth/auth/**` back to a single mount; push before Pasion's router.
#[must_use]
pub fn doubled_router() -> Router {
    Router::with_path(DOUBLED_PATH).get(undouble).head(undouble)
}

/// Routes `/upstream/**` to the Pasion mount.
#[must_use]
pub fn router() -> Router {
    Router::with_path(UPSTREAM_PATH).get(redirect).head(redirect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use salvo::test::{ResponseExt, TestClient};

    #[test]
    fn keeps_the_continue_grant_query() {
        assert_eq!(
            mounted_location("/upstream/authorize/01ABC", Some("kind=continue_authorization_grant&id=01G")),
            "/auth/upstream/authorize/01ABC?kind=continue_authorization_grant&id=01G"
        );
        assert_eq!(mounted_location("/upstream/callback/01ABC", None), "/auth/upstream/callback/01ABC");
    }

    #[test]
    fn a_doubled_mount_is_undone_once() {
        assert_eq!(
            undoubled_location("/auth/auth/register/steps/01M7/finish", None),
            "/auth/register/steps/01M7/finish"
        );
        assert_eq!(undoubled_location("/auth/auth/x", Some("a=1")), "/auth/x?a=1");
    }

    #[tokio::test]
    async fn the_doubled_step_link_reaches_pasion() {
        let service = Service::new(Router::new().push(doubled_router()));
        let mut res = TestClient::get("http://127.0.0.1:8090/auth/auth/register/steps/01M7/finish").send(&service).await;
        assert_eq!(res.status_code, Some(StatusCode::TEMPORARY_REDIRECT));
        assert_eq!(res.headers().get("location").and_then(|v| v.to_str().ok()), Some("/auth/register/steps/01M7/finish"));
        let _ = res.take_string().await;
    }

    #[tokio::test]
    async fn the_login_page_link_reaches_pasion() {
        let service = Service::new(Router::new().push(router()));
        let mut res = TestClient::get("http://127.0.0.1:8090/upstream/authorize/01ABC?id=01G")
            .send(&service)
            .await;
        assert_eq!(res.status_code, Some(StatusCode::TEMPORARY_REDIRECT));
        assert_eq!(
            res.headers().get("location").and_then(|v| v.to_str().ok()),
            Some("/auth/upstream/authorize/01ABC?id=01G")
        );
        let _ = res.take_string().await;
    }
}
