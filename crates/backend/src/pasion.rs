//! Pasion shares the process/listener, but never Palpo's database migrations.
use crate::config::Config;
use anyhow::Context;
use figment::{Figment, providers::Serialized};
use pasion_config::{AppConfig, ConfigurationSection, HttpResource};
use rand_chacha::rand_core::SeedableRng;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const MOUNT: &str = "/auth/";
pub const ADMIN_CLIENT_ID: &str = "01KMQPADM1N000000000000000";

#[derive(Clone)]
pub struct PasionConfig {
    pub database_url: String,
    pub resources_dir: PathBuf,
    pub delegate_matrix_auth: bool,
    pub settings: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmbeddingConfig {
    #[serde(default = "default_resources")]
    resources_dir: PathBuf,
    #[serde(default = "default_delegate")]
    delegate_matrix_auth: bool,
}
fn default_delegate() -> bool {
    true
}
fn default_resources() -> PathBuf {
    "../../resources/pasion".into()
}
impl PasionConfig {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path
            .as_ref()
            .canonicalize()
            .context("cannot open pasion_config")?;
        let base = path.parent().unwrap();
        let mut settings: Value = toml::from_str(&std::fs::read_to_string(&path)?)
            .context("invalid Pasion TOML configuration")?;
        let embedding = settings
            .as_object_mut()
            .context("Pasion configuration must be a table")?
            .remove("hagency")
            .unwrap_or_else(|| json!({}));
        let embedding: EmbeddingConfig = serde_json::from_value(embedding)
            .context("invalid Pasion [hagency] embedding options")?;
        let database_url = settings["database"]["uri"]
            .as_str()
            .context("Pasion configuration requires [database].uri")?
            .to_owned();
        crate::config::resolve_file_references(&mut settings, base);
        Ok(Self {
            database_url,
            resources_dir: crate::config::resolve(base, &embedding.resources_dir),
            delegate_matrix_auth: embedding.delegate_matrix_auth,
            settings,
        })
    }
    pub fn validate(&self, host: &Config) -> anyhow::Result<()> {
        let auth_db = crate::config::postgres_database(&self.database_url, "Pasion database.uri")?;
        let matrix_db = crate::config::postgres_database(&host.matrix.db.url, "matrix.db.url")?;
        let admin_db = crate::config::postgres_database(&host.database_url, "database_url")?;
        // Different hostname spellings may still refer to the same server.
        anyhow::ensure!(
            auth_db.path() != matrix_db.path() && auth_db.path() != admin_db.path(),
            "Hagency, Palpo and Pasion must use different database names"
        );
        let settings = self
            .settings
            .as_object()
            .context("Pasion configuration must be a table")?;
        for key in ["http", "matrix", "secrets", "templates", "storage"] {
            anyhow::ensure!(
                !settings.contains_key(key),
                "Pasion {key} is managed by hagency-server"
            );
        }
        anyhow::ensure!(
            !settings
                .get("policy")
                .is_some_and(|p| p.get("cedar_policy_file").is_some()),
            "Pasion policy files are managed through resources_dir"
        );
        Ok(())
    }
    pub fn resources(&self) -> anyhow::Result<Vec<HttpResource>> {
        let public = self.resources_dir.join("public");
        let public = camino::Utf8PathBuf::from_path_buf(public)
            .map_err(|_| anyhow::anyhow!("Pasion asset path must be UTF-8"))?;
        anyhow::ensure!(
            pasion_backend::server::discover_frontend_script(&public).is_some(),
            "Pasion frontend is missing; run just prepare-pasion"
        );
        Ok(vec![
            HttpResource::Discovery,
            HttpResource::Human,
            HttpResource::OAuth,
            HttpResource::RestApi {
                playground: false,
                undocumented_oauth2_access: false,
            },
            HttpResource::AdminApi,
            HttpResource::Health,
            HttpResource::Assets { path: public },
        ])
    }
}

/// Persist random OAuth signing/encryption keys before initializing either
/// component. URLs and the Matrix shared secret are wired from one host config.
pub async fn prepare(host: &mut Config) -> anyhow::Result<Option<Figment>> {
    let Some(conf) = host.pasion.clone() else {
        return Ok(None);
    };
    conf.resources()?;
    std::fs::create_dir_all(&host.data_dir)?;
    let path = host.data_dir.join("pasion-secrets.json");
    if !path.exists() {
        let mut rng = rand_chacha::ChaChaRng::from_entropy();
        let generated = pasion_config::RootConfig::generate(&mut rng).await?;
        let generated = serde_json::to_value(generated)?;
        write_private_once(
            &path,
            &json!({ "secrets": generated["secrets"], "matrix_secret": generated["matrix"]["secret"] }),
        )?;
    }
    let saved: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let secret = host.matrix.admin.mas_secret.clone().unwrap_or(
        saved["matrix_secret"]
            .as_str()
            .context("invalid persisted Pasion shared secret")?
            .to_owned(),
    );
    anyhow::ensure!(
        !secret.trim().is_empty(),
        "Matrix/Pasion shared secret must not be empty"
    );
    host.matrix.admin.mas_secret = Some(secret.clone());
    let public = host.public_origin.join(MOUNT)?;
    let internal = host.internal_origin().join(MOUNT)?;
    if conf.delegate_matrix_auth {
        anyhow::ensure!(
            host.matrix.delegated_auth.is_none(),
            "remove matrix.delegated_auth; pasion.delegate_matrix_auth manages it"
        );
        host.matrix.delegated_auth = Some(palpo::config::DelegatedAuthConfig {
            enable: true,
            issuer: Some(public.to_string()),
            introspection_endpoint: Some(internal.join("oauth2/introspect")?.to_string()),
            password_login_endpoint: Some(
                internal
                    .join("api/internal/matrix/password-login")?
                    .to_string(),
            ),
            account_management_url: Some(public.join("settings")?.to_string()),
            // Pasion is the role authority: revocation must affect the next request.
            introspection_cache_ttl: 0,
            ..Default::default()
        });
    }
    let root = conf.resources_dir;
    let mut settings = conf.settings;
    if settings.get("account").is_none() {
        settings["account"] = json!({});
    }
    settings["account"]["admin_portal_url"] = json!(host.public_origin.join("admin")?);
    if settings.get("branding").is_none() {
        settings["branding"] = json!({});
    }
    let logo_url = host.public_origin.join("assets/hagency-mark.svg")?;
    settings["branding"]["logo_uri"] = json!(logo_url);
    // The bundled SPA is a public PKCE client. Keep its callback aligned with
    // the host origin, without requiring a second deployment/configuration.
    let client = json!({
        "client_id": ADMIN_CLIENT_ID,
        "client_auth_method": "none",
        "client_name": "Hagency Server",
        "logo_uri": logo_url,
        "redirect_uris": [host.public_origin.join("oauth/callback")?]
    });
    let clients = settings
        .as_object_mut()
        .unwrap()
        .entry("clients")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("Pasion clients must be an array")?;
    if let Some(existing) = clients
        .iter_mut()
        .find(|c| c["client_id"] == ADMIN_CLIENT_ID)
    {
        *existing = client;
    } else {
        clients.push(client);
    }
    // The host owns the listener, issuer, homeserver and key lifecycle.
    // Native database pool/TLS options stay in Pasion's own file.
    for (k,v) in json!({
        "http": {"public_base":public,"issuer":public,"listeners":[{"binds":[{"address":host.listen.to_string()}],"resources":[{"name":"assets","path":root.join("public")}]}],"trusted_proxies":[]},
        "matrix":{"kind":"palpo","homeserver":host.matrix.server_name,"endpoint":host.internal_origin(),"secret":secret},
        "secrets":saved["secrets"],
        "templates":{"path":root.join("templates"),"translations_path":root.join("translations")},
        "storage":{"backend":"fs","root":host.data_dir.join("pasion-media")}
    }).as_object().unwrap() { settings[k]=v.clone(); }
    if settings.get("policy").is_none() {
        settings["policy"] = json!({});
    }
    settings["policy"]["cedar_policy_file"] = json!(root.join("cedar/default.cedar"));
    let figment = Figment::new().merge(Serialized::defaults(settings));
    AppConfig::extract(&figment).map_err(anyhow::Error::from_boxed)?;
    Ok(Some(figment))
}

/// Operator-only first administrator setup. Linking a pre-existing Matrix
/// identity is explicit and restricted to an active human administrator.
pub async fn bootstrap_admin(
    figment: &Figment,
    server_name: &str,
    username: &str,
    password_path: &Path,
    link_existing: bool,
) -> anyhow::Result<()> {
    use pasion_data::{
        PgRepositoryFactory, RepositoryAccess, RepositoryFactory, SystemClock,
        queue::{ProvisionUserJob, QueueJobRepositoryExt},
        user::UserFilter,
    };
    use zeroize::Zeroizing;

    anyhow::ensure!(
        pasion_backend::util::username_valid(username),
        "invalid bootstrap username"
    );
    let user_id = palpo::core::OwnedUserId::try_from(format!("@{username}:{server_name}"))?;
    user_id.validate_strict()?;
    let existing = palpo::data::user::user_exists(&user_id).await?;
    if existing {
        let user = palpo::data::user::get_user(&user_id).await?;
        anyhow::ensure!(
            link_existing
                && user.is_admin
                && !user.is_guest
                && user.appservice_id.is_none()
                && user.deactivated_at.is_none()
                && user.suspended_at.is_none(),
            "existing Matrix identity must be an active human administrator; explicitly use --link-existing-matrix-admin"
        );
    } else {
        anyhow::ensure!(!link_existing, "no existing Matrix administrator to link");
    }
    let config = AppConfig::extract(figment).map_err(anyhow::Error::from_boxed)?;
    let manager = pasion_backend::util::password_manager_from_config(&config.passwords).await?;
    let password = Zeroizing::new(
        std::fs::read_to_string(password_path)?
            .trim_end_matches(['\r', '\n'])
            .to_owned(),
    );
    anyhow::ensure!(
        password.len() >= 12 && manager.is_password_complex_enough(&password)?,
        "bootstrap password does not meet Pasion password policy"
    );
    let mut rng = rand_chacha::ChaChaRng::from_entropy();
    let (version, hash) = manager.hash(&mut rng, password).await?;
    let pool = pasion_backend::util::diesel_pool_from_config(&config.database).await?;
    let mut repo = PgRepositoryFactory::new(pool).create().await?;
    repo.user().acquire_bootstrap_admin_lock().await?;
    anyhow::ensure!(
        repo.user()
            .count(UserFilter::new().can_request_admin_only())
            .await?
            == 0,
        "Pasion already has an administrator; use its account management to grant roles"
    );
    anyhow::ensure!(
        !repo.user().exists(username).await?,
        "Pasion account already exists; bootstrap refuses to overwrite it"
    );
    let clock = SystemClock::default();
    let user = repo
        .user()
        .add(&mut rng, &clock, username.to_owned())
        .await?;
    let user = repo.user().set_can_request_admin(user, true).await?;
    repo.user_password()
        .add(&mut rng, &clock, &user, version, hash, None)
        .await?;
    repo.queue_job()
        .schedule_job(&mut rng, &clock, ProvisionUserJob::new(&user))
        .await?;
    repo.save().await?;
    // The durable Pasion provisioning job mirrors this role after the listener
    // starts, and retries on failure. Never change Matrix before Pasion commits.
    tracing::info!(user=%user_id, "Pasion administrator created; Matrix role synchronization queued");
    Ok(())
}

/// The embedded Palpo version checks its Matrix role but does not enforce
/// OAuth admin scopes. This boundary requires Pasion's current authorization
/// on every Palpo admin call, including internal Hagency adapter calls.
#[derive(Clone)]
pub struct DelegatedAdminGuard {
    endpoint: url::Url,
    secret: String,
    client: reqwest::Client,
}
impl DelegatedAdminGuard {
    pub fn new(host: &Config) -> anyhow::Result<Self> {
        Ok(Self {
            endpoint: host.internal_origin().join("auth/oauth2/introspect")?,
            secret: host
                .matrix
                .admin
                .mas_secret
                .clone()
                .context("missing Pasion shared secret")?,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
        })
    }
}
pub fn allows_palpo_admin(value: &Value) -> bool {
    value["active"] == true
        && value["scope"]
            .as_str()
            .is_some_and(|scope| scope.split_whitespace().any(|s| s == "urn:palpo:admin:*"))
}
#[salvo::async_trait]
impl salvo::Handler for DelegatedAdminGuard {
    async fn handle(
        &self,
        req: &mut salvo::Request,
        _: &mut salvo::Depot,
        res: &mut salvo::Response,
        ctrl: &mut salvo::FlowCtrl,
    ) {
        // Use the router's parser itself: it decodes each segment once and
        // ignores repeated/trailing separators. Raw URI prefix checks disagree.
        let mut path = salvo::routing::PathState::from_borrowed_path(req.uri().path());
        let namespace = match path.pick() {
            Some("_palpo") => "_palpo",
            Some("_synapse") => "_synapse",
            _ => return,
        };
        path.forward(namespace.len());
        if path.pick() != Some("admin") {
            return;
        }
        path.forward("admin".len());
        let mut decoded = vec![namespace.to_owned(), "admin".into()];
        while let Some(part) = path.pick() {
            let width = part.len();
            decoded.push(part.to_owned());
            path.forward(width);
        }
        let parts = decoded.iter().map(String::as_str).collect::<Vec<_>>();
        let token = match req.headers().get("authorization") {
            Some(header) => header
                .to_str()
                .ok()
                .and_then(|s| s.split_once(' '))
                .filter(|(scheme, token)| {
                    scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()
                })
                .map(|(_, token)| token.to_owned()),
            None => req.query::<String>("access_token"),
        };
        let result = if let Some(token) = token {
            match self
                .client
                .post(self.endpoint.clone())
                .bearer_auth(&self.secret)
                .form(&[("token", token)])
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    response.json::<Value>().await.ok()
                }
                _ => None,
            }
        } else {
            Some(json!({"active":false}))
        };
        let (status, code, message) = match result {
            Some(value) if allows_palpo_admin(&value) => {
                // Human credentials and account creation must not bypass the
                // identity provider through the copied Matrix admin screens.
                let user_write = match parts.as_slice() {
                    [_, "admin", "v2", "users", user]
                        if req.method() == salvo::http::Method::PUT =>
                    {
                        Some(*user)
                    }
                    _ => None,
                };
                let password_reset = req.method() == salvo::http::Method::POST
                    && matches!(parts.as_slice(), [_, "admin", "v1", "reset_password", _]);
                let blocked = if let Some(user) = user_write {
                    match req.parse_json::<Value>().await {
                        Ok(input) => {
                            // PathState already decoded this exact routing segment.
                            let user = palpo::core::OwnedUserId::try_from(user).ok();
                            match user {
                                Some(user) => {
                                    !input["password"].is_null()
                                        || !palpo::data::user::user_exists(&user)
                                            .await
                                            .unwrap_or(false)
                                }
                                None => true,
                            }
                        }
                        Err(_) => true,
                    }
                } else {
                    password_reset
                };
                if blocked {
                    (
                        salvo::http::StatusCode::FORBIDDEN,
                        "M_FORBIDDEN",
                        "Manage accounts and passwords through Pasion Account management.",
                    )
                } else {
                    return;
                }
            }
            Some(value) if value["active"] == true => (
                salvo::http::StatusCode::FORBIDDEN,
                "M_FORBIDDEN",
                "Pasion administrator authorization is required.",
            ),
            Some(_) => (
                salvo::http::StatusCode::UNAUTHORIZED,
                "M_UNKNOWN_TOKEN",
                "A valid Pasion access token is required.",
            ),
            None => (
                salvo::http::StatusCode::BAD_GATEWAY,
                "M_UNKNOWN",
                "Authentication service is unavailable.",
            ),
        };
        res.status_code(status);
        res.render(salvo::writing::Json(
            json!({"errcode":code,"error":message}),
        ));
        ctrl.skip_rest();
    }
}

fn write_private_once(path: &Path, value: &Value) -> anyhow::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&tmp)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        match std::fs::hard_link(&tmp, path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[salvo::handler]
    async fn guarded_route_reached(res: &mut salvo::Response) {
        res.status_code(salvo::http::StatusCode::NO_CONTENT);
    }

    async fn admin_guard_fixture() -> (salvo::Service, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint =
            url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let worker = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    loop {
                        let mut chunk = [0; 4096];
                        let n = stream.read(&mut chunk).await.unwrap();
                        assert!(n > 0);
                        raw.extend_from_slice(&chunk[..n]);
                        assert!(raw.len() < 16384);
                        if let Some(end) = raw.windows(4).position(|v| v == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&raw[..end]);
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap())
                                })
                                .unwrap();
                            if raw.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    let high_scope = String::from_utf8_lossy(&raw).contains("token=admin-token");
                    let body = json!({"active":true,"scope":if high_scope {
                        "urn:matrix:client:api:* urn:palpo:admin:*"
                    } else { "urn:matrix:client:api:*" }})
                    .to_string();
                    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
                });
            }
        });
        let guard = DelegatedAdminGuard {
            endpoint,
            secret: "fixture-secret".into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .unwrap(),
        };
        let mut router = salvo::Router::new().hoop(guard);
        for namespace in ["_palpo", "_synapse"] {
            router = router.push(
                salvo::Router::with_path(format!("{namespace}/admin"))
                    .push(salvo::Router::with_path("v1/appservices").get(guarded_route_reached))
                    .push(
                        salvo::Router::with_path("v1/reset_password/{user}")
                            .post(guarded_route_reached),
                    )
                    .push(salvo::Router::with_path("v2/users/{user}").put(guarded_route_reached)),
            );
        }
        (salvo::Service::new(router), worker)
    }

    #[tokio::test]
    async fn admin_guard_uses_the_same_path_segments_as_salvo() {
        use salvo::{http::StatusCode, test::TestClient};
        let (service, worker) = admin_guard_fixture().await;
        for path in [
            "/_palpo/admin/v1/appservices",
            "/_%70alpo/admin/v1/appservices",
            "/_palpo/%61dmin/v1/appservices",
            "/_palpo//admin/v1/appservices",
            "//_palpo/admin/v1/appservices",
            "/_palpo/admin/v1/appservices/",
            "/_synapse/admin/v1/appservices",
            "/_%73ynapse/admin/v1/appservices",
            "/_synapse//admin/v1/appservices",
        ] {
            for (token, expected) in [
                (None, StatusCode::UNAUTHORIZED),
                (Some("low-scope"), StatusCode::FORBIDDEN),
                (Some("admin-token"), StatusCode::NO_CONTENT),
            ] {
                let mut request = TestClient::get(format!("http://example.test{path}"));
                if let Some(token) = token {
                    request = request.bearer_auth(token);
                }
                let reply = request.send(&service).await;
                assert_eq!(reply.status_code, Some(expected), "{path} {token:?}");
            }
        }
        worker.abort();
    }

    #[tokio::test]
    async fn account_password_writes_cannot_bypass_pasion_with_path_variants() {
        use salvo::{http::StatusCode, test::TestClient};
        let (service, worker) = admin_guard_fixture().await;
        for namespace in ["_palpo", "_synapse"] {
            for path in [
                format!("/{namespace}/admin/v2/users/%40alice%3Aexample.test"),
                format!("/{namespace}/admin/v%32/u%73ers/%40alice%3Aexample.test/"),
                format!("/{namespace}//admin/v2/users/%40alice%3Aexample.test"),
            ] {
                let reply = TestClient::put(format!("http://example.test{path}"))
                    .bearer_auth("admin-token")
                    .json(&json!({"password":"fixture-only"}))
                    .send(&service)
                    .await;
                assert_eq!(reply.status_code, Some(StatusCode::FORBIDDEN), "{path}");
            }
            for path in [
                format!("/{namespace}/admin/v1/reset_password/%40alice%3Aexample.test"),
                format!("/{namespace}/%61dmin/v1/reset%5fpassword/%40alice%3Aexample.test/"),
                format!("/{namespace}//admin/v1/reset_password/%40alice%3Aexample.test"),
            ] {
                let reply = TestClient::post(format!("http://example.test{path}"))
                    .bearer_auth("admin-token")
                    .json(&json!({"new_password":"fixture-only"}))
                    .send(&service)
                    .await;
                assert_eq!(reply.status_code, Some(StatusCode::FORBIDDEN), "{path}");
            }
        }
        worker.abort();
    }

    #[test]
    fn delegated_administration_requires_live_exact_scope() {
        assert!(allows_palpo_admin(
            &json!({"active":true,"scope":"urn:matrix:client:api:* urn:palpo:admin:* urn:pasion:admin"})
        ));
        for scope in [
            "urn:matrix:client:api:*",
            "urn:pasion:admin",
            "urn:palpo:admin:users",
            "urn:palpo:admin:*suffix",
        ] {
            assert!(!allows_palpo_admin(&json!({"active":true,"scope":scope})));
        }
        assert!(!allows_palpo_admin(
            &json!({"active":false,"scope":"urn:palpo:admin:*"})
        ));
    }
}
