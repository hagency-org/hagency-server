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
