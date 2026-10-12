//! Same-origin Dioxus SPA hosting, separated from the Hagency API.
use crate::{
    config::Config,
    pasion::{ADMIN_CLIENT_ID, MOUNT},
};
use salvo::{http::header, prelude::*};
use serde_json::{Value, json};
use std::path::{Component, PathBuf};
use url::Url;

#[derive(Clone)]
pub struct Frontend {
    directory: PathBuf,
    origin: Url,
    runtime: Value,
}

impl Frontend {
    pub fn new(conf: &Config) -> Self {
        let pasion = conf.pasion.as_ref();
        Self {
            directory: conf
                .public_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("resources/frontend/public")),
            origin: conf.public_origin.clone(),
            runtime: json!({
                "server_name": conf.matrix.server_name,
                "pasion_enabled": pasion.is_some(),
                "oauth_enabled": pasion.is_some_and(|p| p.delegate_matrix_auth),
                "pasion_public_url": if pasion.is_some() {
                    conf.public_origin.join(MOUNT).unwrap().to_string()
                } else { String::new() },
                "oauth_client_id": ADMIN_CLIENT_ID,
                "palpo_admin_url": ""
            }),
        }
    }

    pub fn router(&self) -> Router {
        let mut router = Router::new();
        // API, Matrix and Pasion paths never use the SPA fallback.
        for path in [
            "config.json",
            "assets/{**path}",
            "favicon.ico",
            "login",
            "oauth/callback",
            "admin",
            "admin/{**path}",
            "hagency/{**path}",
        ] {
            router = router.push(Router::with_path(path).get(self.clone()));
        }
        router
    }
}

fn safe_asset(path: &str) -> Option<PathBuf> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    let path = PathBuf::from(decoded.trim_start_matches('/'));
    path.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then_some(path)
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "wasm" => "application/wasm",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "html" => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[async_trait]
impl Handler for Frontend {
    async fn handle(&self, req: &mut Request, _: &mut Depot, res: &mut Response, _: &mut FlowCtrl) {
        for (name, value) in [
            ("cache-control", "no-store"),
            (
                "content-security-policy",
                "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data: blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
            ),
            ("x-content-type-options", "nosniff"),
            ("referrer-policy", "no-referrer"),
        ] {
            let _ = res.add_header(name, value, true);
        }
        if req
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            != Some(&self.origin[url::Position::BeforeHost..url::Position::AfterPort])
        {
            res.status_code(StatusCode::FORBIDDEN);
            return;
        }
        if req.uri().path() == "/config.json" {
            res.render(Json(self.runtime.clone()));
            return;
        }
        if req.uri().path() == "/account-request" && self.runtime["oauth_enabled"] == true {
            res.status_code(StatusCode::FOUND);
            let _ = res.add_header(header::LOCATION, "/auth/register", true);
            return;
        }
        let asset = req.uri().path().starts_with("/assets/") || req.uri().path() == "/favicon.ico";
        let relative = if asset {
            safe_asset(req.uri().path())
        } else {
            Some("index.html".into())
        };
        let Some(relative) = relative else {
            res.status_code(StatusCode::NOT_FOUND);
            return;
        };
        let directory = tokio::fs::canonicalize(&self.directory).await;
        let target = tokio::fs::canonicalize(self.directory.join(&relative)).await;
        let Ok(directory) = directory else {
            res.status_code(StatusCode::SERVICE_UNAVAILABLE);
            res.render(Text::Plain(
                "Frontend assets are missing. Run just prepare-frontend.",
            ));
            return;
        };
        let Ok(target) = target else {
            res.status_code(StatusCode::NOT_FOUND);
            return;
        };
        if !target.starts_with(directory) {
            res.status_code(StatusCode::NOT_FOUND);
            return;
        }
        match tokio::fs::read(target).await {
            Ok(body) => {
                let _ = res.add_header(
                    header::CONTENT_TYPE,
                    content_type(relative.to_str().unwrap_or("")),
                    true,
                );
                res.status_code(StatusCode::OK);
                let _ = res.write_body(body);
            }
            Err(_) => {
                res.status_code(StatusCode::NOT_FOUND);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asset_paths_stay_below_the_public_directory() {
        assert_eq!(
            safe_asset("/assets/app.wasm"),
            Some("assets/app.wasm".into())
        );
        for path in [
            "/assets/%2e%2e/secret",
            "/assets/%2e%2e%2fsecret",
            "/assets/../../secret",
        ] {
            assert!(safe_asset(path).is_none());
        }
        assert_eq!(content_type("assets/app.wasm"), "application/wasm");
    }
}
