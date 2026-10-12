//! Public site homepage, independent of browser sessions and the console SPA.
use crate::config::{Config, remote_home_page};
use salvo::{http::header, prelude::*};
use std::time::Duration;

const DEFAULT_PAGE: &str = include_str!("home_page.html");
const HTML: &str = "text/html; charset=utf-8";

#[derive(Clone)]
pub struct HomePage {
    source: Option<String>,
    host: String,
    client: reqwest::Client,
}

impl HomePage {
    pub fn new(conf: &Config) -> Self {
        Self {
            source: conf.home_page.clone(),
            host: conf.public_origin[url::Position::BeforeHost..url::Position::AfterPort]
                .to_owned(),
            client: reqwest::Client::new(),
        }
    }

    async fn remote(&self, url: &str) -> anyhow::Result<(Vec<u8>, String)> {
        // Bound both headers and body, including redirects, to one request deadline.
        tokio::time::timeout(Duration::from_secs(10), async {
            let response = self.client.get(url).send().await?.error_for_status()?;
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or(HTML)
                .to_owned();
            Ok((response.bytes().await?.to_vec(), content_type))
        })
        .await?
    }
}

#[async_trait]
impl Handler for HomePage {
    async fn handle(&self, req: &mut Request, _: &mut Depot, res: &mut Response, _: &mut FlowCtrl) {
        for (name, value) in [
            ("cache-control", "no-store"),
            ("x-content-type-options", "nosniff"),
            ("referrer-policy", "no-referrer"),
        ] {
            let _ = res.add_header(name, value, true);
        }
        if req
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            != Some(self.host.as_str())
        {
            res.status_code(StatusCode::FORBIDDEN);
            return;
        }
        let (body, content_type) = match &self.source {
            Some(source) if remote_home_page(source) => match self.remote(source).await {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(%error, "failed to fetch home_page; serving built-in welcome page");
                    (DEFAULT_PAGE.as_bytes().to_vec(), HTML.to_owned())
                }
            },
            Some(path) => match tokio::fs::read(path).await {
                Ok(body) => (body, HTML.to_owned()),
                Err(error) => {
                    tracing::warn!(%error, "failed to read local home_page");
                    res.status_code(StatusCode::SERVICE_UNAVAILABLE);
                    res.render(Text::Plain("Configured homepage is unavailable."));
                    return;
                }
            },
            None => (DEFAULT_PAGE.as_bytes().to_vec(), HTML.to_owned()),
        };
        // Custom pages own their scripts/styles; the console's SPA CSP is not applied here.
        // Only content is copied from remote sources, never their cookies or other headers.
        let _ = res.add_header(header::CONTENT_TYPE, content_type, true);
        res.status_code(StatusCode::OK);
        let _ = res.write_body(body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use salvo::test::{ResponseExt, TestClient};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn page(source: Option<String>) -> HomePage {
        HomePage {
            source,
            host: "example.test".into(),
            client: reqwest::Client::new(),
        }
    }

    async fn get(page: HomePage) -> Response {
        TestClient::get("http://example.test/")
            .add_header("host", "example.test", true)
            .send(&Service::new(Router::new().get(page)))
            .await
    }

    #[tokio::test]
    async fn default_home_is_public_and_console_paths_are_separate() {
        let service = Service::new(
            Router::new()
                .push(Router::new().get(page(None)))
                .push(Router::with_path("hagency/home").get(console)),
        );
        let mut response = TestClient::get("http://example.test/")
            .add_header("host", "example.test", true)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        let body = response.take_string().await.unwrap();
        assert!(body.contains("href=\"/hagency/home\""));
        assert!(!body.contains(".wasm"));
        let mut response = TestClient::get("http://example.test/hagency/home")
            .add_header("host", "example.test", true)
            .send(&service)
            .await;
        assert_eq!(response.take_string().await.unwrap(), "console");
        for path in ["api/session", "_matrix/client/versions", "auth/", "unknown"] {
            let response = TestClient::get(format!("http://example.test/{path}"))
                .add_header("host", "example.test", true)
                .send(&service)
                .await;
            assert_eq!(response.status_code, Some(StatusCode::NOT_FOUND));
        }
        let response = TestClient::get("http://other.test/")
            .add_header("host", "other.test", true)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::FORBIDDEN));
    }

    #[handler]
    async fn console() -> &'static str {
        "console"
    }

    #[tokio::test]
    async fn local_home_is_read_per_request_and_missing_files_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("home.html");
        let page = page(Some(path.to_str().unwrap().to_owned()));
        for body in ["<h1>First homepage</h1>", "<h1>Updated homepage</h1>"] {
            tokio::fs::write(&path, body).await.unwrap();
            let mut response = get(page.clone()).await;
            assert_eq!(response.status_code, Some(StatusCode::OK));
            assert_eq!(response.headers()[header::CONTENT_TYPE], HTML);
            assert_eq!(response.take_string().await.unwrap(), body);
        }
        tokio::fs::remove_file(path).await.unwrap();
        assert_eq!(
            get(page).await.status_code,
            Some(StatusCode::SERVICE_UNAVAILABLE)
        );
    }

    async fn remote_fixture(response: &'static str) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/home", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let size = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..size]).to_ascii_lowercase();
            assert!(!request.contains("cookie:"));
            assert!(!request.contains("authorization:"));
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        (url, task)
    }

    #[tokio::test]
    async fn remote_home_preserves_content_type_without_forwarding_browser_credentials() {
        let (url, task) = remote_fixture(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=iso-8859-1\r\nSet-Cookie: upstream=secret\r\nConnection: close\r\n\r\n<h1>Remote home</h1>",
        ).await;
        let service = Service::new(Router::new().get(page(Some(url))));
        let mut response = TestClient::get("http://example.test/")
            .add_header("host", "example.test", true)
            .add_header("cookie", "host_session=private", true)
            .add_header("authorization", "Bearer private", true)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=iso-8859-1"
        );
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert_eq!(
            response.take_string().await.unwrap(),
            "<h1>Remote home</h1>"
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn remote_http_and_connection_failures_use_the_public_default() {
        let (url, task) =
            remote_fixture("HTTP/1.1 503 Unavailable\r\nConnection: close\r\n\r\n").await;
        let mut response = get(page(Some(url.clone()))).await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert_eq!(response.take_string().await.unwrap(), DEFAULT_PAGE);
        task.await.unwrap();
        // The fixture listener is closed, so this request covers a connection failure too.
        let mut response = get(page(Some(url))).await;
        assert_eq!(response.take_string().await.unwrap(), DEFAULT_PAGE);
    }
}
