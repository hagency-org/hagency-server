//! Same-origin Dioxus SPA hosting, separated from the Hagency API.
use crate::{
    config::Config,
    pasion::{ADMIN_CLIENT_ID, MOUNT},
};
use salvo::{
    http::{Method, header},
    prelude::*,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Component, PathBuf};
use url::Url;

#[derive(Clone)]
pub struct Frontend {
    directory: PathBuf,
    archive: PathBuf,
    origin: Url,
    runtime: Value,
}

impl Frontend {
    pub fn new(conf: &Config) -> Self {
        let pasion = conf.pasion.as_ref();
        Self {
            archive: conf.data_dir.join("frontend-assets"),
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
        let mut router = Router::new().push(Router::new().get(self.clone()).head(self.clone()));
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
            router = router.push(Router::with_path(path).get(self.clone()).head(self.clone()));
        }
        router
    }

    /// Keep immutable public bundles across container replacements. Only generated
    /// hashed assets are archived; config, HTML and private data never enter here.
    pub async fn preserve_assets(&self) -> anyhow::Result<()> {
        let source = self.directory.join("assets");
        if !source.exists() {
            return Ok(());
        }
        let destination = self.archive.join("assets");
        tokio::fs::create_dir_all(&destination).await?;
        let mut files = tokio::fs::read_dir(source).await?;
        while let Some(entry) = files.next_entry().await? {
            let name = entry.file_name();
            if !entry.file_type().await?.is_file() || !hashed_asset(&name.to_string_lossy()) {
                continue;
            }
            let body = tokio::fs::read(entry.path()).await?;
            let target = destination.join(name);
            match tokio::fs::read(&target).await {
                Ok(existing) => {
                    anyhow::ensure!(existing == body, "Immutable frontend asset collision")
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // Rename prevents partial files becoming visible during startup.
                    let temp = tempfile::NamedTempFile::new_in(&destination)?;
                    tokio::fs::write(temp.path(), body).await?;
                    temp.persist(target)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

fn hashed_asset(name: &str) -> bool {
    name.rsplit_once("-dxh").is_some_and(|(_, suffix)| {
        let hash = suffix.split('.').next().unwrap_or("");
        hash.len() == 16
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            && suffix.contains('.')
            && !suffix.ends_with(".html")
    })
}

fn encoding_quality(header: &str, encoding: &str) -> f32 {
    let mut wildcard = None;
    for item in header.split(',') {
        let mut parts = item.trim().split(';');
        let name = parts.next().unwrap_or("").trim();
        let quality = parts
            .find_map(|p| {
                p.trim()
                    .split_once('=')
                    .filter(|(key, _)| key.eq_ignore_ascii_case("q"))
            })
            .map_or(1.0, |(_, q)| {
                q.trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|q| (0.0..=1.0).contains(q))
                    .unwrap_or(0.0)
            });
        if name.eq_ignore_ascii_case(encoding) {
            return quality;
        }
        if name == "*" {
            wildcard = Some(quality);
        }
    }
    if encoding == "identity" {
        if wildcard == Some(0.0) { 0.0 } else { 1.0 }
    } else {
        wildcard.unwrap_or(0.0)
    }
}

/// Runs outside embedded Matrix CORS, which can replace inner Vary headers.
#[handler]
pub async fn frontend_vary(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    ctrl.call_next(req, depot, res).await;
    if depot
        .get::<bool>("hagency.frontend.file")
        .copied()
        .unwrap_or(false)
    {
        salvo::http::append_vary_header(res.headers_mut(), "Accept-Encoding");
    }
}

async fn confined_file(directory: &std::path::Path, relative: &std::path::Path) -> Option<PathBuf> {
    let directory = tokio::fs::canonicalize(directory).await.ok()?;
    let target = tokio::fs::canonicalize(directory.join(relative))
        .await
        .ok()?;
    (target.starts_with(&directory) && target.is_file()).then_some(target)
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
    async fn handle(
        &self,
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        _: &mut FlowCtrl,
    ) {
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
        let Ok(directory) = directory else {
            res.status_code(StatusCode::SERVICE_UNAVAILABLE);
            res.render(Text::Plain(
                "Frontend assets are missing. Run just prepare-frontend.",
            ));
            return;
        };
        let mut selected_directory = self.directory.clone();
        let mut target = confined_file(&selected_directory, &relative).await;
        if target.is_none()
            && asset
            && relative
                .file_name()
                .is_some_and(|n| hashed_asset(&n.to_string_lossy()))
        {
            selected_directory = self.archive.clone();
            target = confined_file(&selected_directory, &relative).await;
        }
        let Some(target) = target else {
            res.status_code(StatusCode::NOT_FOUND);
            return;
        };
        if !target.starts_with(directory) && selected_directory == self.directory {
            res.status_code(StatusCode::NOT_FOUND);
            return;
        }
        let accept = req
            .headers()
            .get(header::ACCEPT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let mut selected = (encoding_quality(accept, "identity"), None, target);
        // Ties prefer Brotli, then gzip, then identity. Respect q=0 exclusions.
        for (encoding, extension) in [("gzip", ".gz"), ("br", ".br")] {
            let quality = encoding_quality(accept, encoding);
            if quality > 0.0 && quality >= selected.0 {
                let mut variant = relative.as_os_str().to_os_string();
                variant.push(extension);
                if let Some(path) =
                    confined_file(&selected_directory, std::path::Path::new(&variant)).await
                {
                    selected = (quality, Some(encoding), path);
                }
            }
        }
        if selected.0 <= 0.0 {
            res.status_code(StatusCode::NOT_ACCEPTABLE);
            return;
        }
        match tokio::fs::read(selected.2).await {
            Ok(body) => {
                depot.insert("hagency.frontend.file", true);
                salvo::http::append_vary_header(res.headers_mut(), "Accept-Encoding");
                let policy = if asset
                    && relative
                        .file_name()
                        .is_some_and(|n| hashed_asset(&n.to_string_lossy()))
                {
                    "public, max-age=31536000, immutable"
                } else {
                    "no-cache"
                };
                let _ = res.add_header(header::CACHE_CONTROL, policy, true);
                let etag = format!("\"{}\"", hex::encode(Sha256::digest(&body)));
                let _ = res.add_header(header::ETAG, &etag, true);
                if let Some(encoding) = selected.1 {
                    let _ = res.add_header(header::CONTENT_ENCODING, encoding, true);
                }
                let _ = res.add_header(
                    header::CONTENT_TYPE,
                    content_type(relative.to_str().unwrap_or("")),
                    true,
                );
                let not_modified = req
                    .headers()
                    .get_all(header::IF_NONE_MATCH)
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .flat_map(|v| v.split(','))
                    .any(|tag| tag.trim() == "*" || tag.trim().trim_start_matches("W/") == etag);
                if not_modified {
                    res.status_code(StatusCode::NOT_MODIFIED);
                } else {
                    res.status_code(StatusCode::OK);
                    let _ = res.add_header(header::CONTENT_LENGTH, body.len().to_string(), true);
                    if req.method() != Method::HEAD {
                        let _ = res.write_body(body);
                    }
                }
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
    use salvo::test::{ResponseExt, TestClient};

    #[test]
    fn encoding_exclusions_and_immutable_names_are_strict() {
        assert_eq!(encoding_quality("br;q=0, gzip", "br"), 0.0);
        assert_eq!(encoding_quality("*;q=0", "identity"), 0.0);
        assert_eq!(encoding_quality("br;q=NaN", "br"), 0.0);
        assert_eq!(encoding_quality("*;q=0.5, identity;q=0", "gzip"), 0.5);
        assert!(hashed_asset("app-dxh0123456789abcdef.wasm.br"));
        assert!(!hashed_asset("app-dxhlatest.wasm"));
        assert!(!hashed_asset("index.html"));
    }

    #[tokio::test]
    async fn public_files_negotiate_revalidate_and_survive_releases_without_exposing_private_data()
    {
        let dir = tempfile::tempdir().unwrap();
        let public = dir.path().join("public");
        let archive = dir.path().join("data/frontend-assets");
        tokio::fs::create_dir_all(public.join("assets"))
            .await
            .unwrap();
        let file = "app-dxh0123456789abcdef.wasm";
        for (suffix, bytes) in [
            ("", b"original".as_slice()),
            (".br", b"brotli".as_slice()),
            (".gz", b"gzip".as_slice()),
        ] {
            tokio::fs::write(public.join("assets").join(format!("{file}{suffix}")), bytes)
                .await
                .unwrap();
        }
        tokio::fs::write(public.join("index.html"), "old index")
            .await
            .unwrap();
        tokio::fs::write(public.join("assets/private.json"), "not archived")
            .await
            .unwrap();
        let frontend = Frontend {
            directory: public.clone(),
            archive: archive.clone(),
            origin: Url::parse("http://example.test").unwrap(),
            runtime: json!({"server_name":"example.test"}),
        };
        frontend.preserve_assets().await.unwrap();
        assert!(!archive.join("index.html").exists());
        assert!(!archive.join("assets/private.json").exists());
        let service = Service::new(frontend.clone().router()).hoop(frontend_vary);
        let url = format!("http://example.test/assets/{file}");
        let mut etags = Vec::new();
        for (accept, encoding, body) in [
            ("identity", None, "original"),
            ("br, gzip", Some("br"), "brotli"),
            ("br;q=0, gzip", Some("gzip"), "gzip"),
        ] {
            let mut response = TestClient::get(&url)
                .add_header("host", "example.test", true)
                .add_header("accept-encoding", accept, true)
                .send(&service)
                .await;
            assert_eq!(response.status_code, Some(StatusCode::OK));
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_ENCODING)
                    .and_then(|v| v.to_str().ok()),
                encoding
            );
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "public, max-age=31536000, immutable"
            );
            assert_eq!(response.headers()[header::CONTENT_TYPE], "application/wasm");
            let etag = response.headers()[header::ETAG]
                .to_str()
                .unwrap()
                .to_owned();
            etags.push(etag.clone());
            assert_eq!(
                response.take_bytes(None).await.unwrap().as_ref(),
                body.as_bytes()
            );
            let mut head = TestClient::head(&url)
                .add_header("host", "example.test", true)
                .add_header("accept-encoding", accept, true)
                .send(&service)
                .await;
            assert_eq!(head.headers()[header::ETAG], etag);
            assert!(head.take_bytes(None).await.unwrap().is_empty());
            let mut cached = TestClient::get(&url)
                .add_header("host", "example.test", true)
                .add_header("accept-encoding", accept, true)
                .add_header("if-none-match", format!("W/{etag}"), true)
                .send(&service)
                .await;
            assert_eq!(cached.status_code, Some(StatusCode::NOT_MODIFIED));
            assert!(
                cached
                    .headers()
                    .get_all(header::VARY)
                    .iter()
                    .any(|v| v.to_str().unwrap().contains("Accept-Encoding"))
            );
            assert!(cached.take_bytes(None).await.unwrap().is_empty());
        }
        assert_ne!(etags[0], etags[1]);
        let rejected = TestClient::get(&url)
            .add_header("host", "example.test", true)
            .add_header("accept-encoding", "*;q=0", true)
            .send(&service)
            .await;
        assert_eq!(rejected.status_code, Some(StatusCode::NOT_ACCEPTABLE));
        assert_eq!(rejected.headers()[header::CACHE_CONTROL], "no-store");
        let mut old = TestClient::get("http://example.test/")
            .add_header("host", "example.test", true)
            .send(&service)
            .await;
        let old_etag = old.headers()[header::ETAG].to_str().unwrap().to_owned();
        assert_eq!(old.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(old.take_string().await.unwrap(), "old index");
        tokio::fs::write(public.join("index.html"), "new index")
            .await
            .unwrap();
        let mut new = TestClient::get("http://example.test/")
            .add_header("host", "example.test", true)
            .add_header("if-none-match", old_etag, true)
            .send(&service)
            .await;
        assert_eq!(new.status_code, Some(StatusCode::OK));
        assert_eq!(new.take_string().await.unwrap(), "new index");
        tokio::fs::remove_file(public.join("assets").join(file))
            .await
            .unwrap();
        let mut retained = TestClient::get(&url)
            .add_header("host", "example.test", true)
            .send(&service)
            .await;
        assert_eq!(retained.status_code, Some(StatusCode::OK));
        assert_eq!(retained.take_string().await.unwrap(), "original");
        for path in [
            "/config.json",
            "/assets/missing.js",
            "/assets/%2e%2e%2fsecret",
            "/assets/private-not-present.json",
        ] {
            let response = TestClient::get(format!("http://example.test{path}"))
                .add_header("host", "example.test", true)
                .send(&service)
                .await;
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        tokio::fs::write(public.join("assets").join(file), "different bytes")
            .await
            .unwrap();
        assert!(frontend.preserve_assets().await.is_err());
    }
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
