//! Pasion shares the process/listener, but never Palpo's database migrations.
use crate::config::Config;
use anyhow::Context;
use figment::{Figment, providers::Serialized};
use pasion_config::{AppConfig, ConfigurationSection, HttpResource};
use rand_core::SeedableRng;
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
        let path = req.uri().path();
        if !["/_palpo/admin", "/_synapse/admin"]
            .iter()
            .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
        {
            return;
        }
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
                let user_write =
                    regex::Regex::new(r"^/(?:_palpo|_synapse)/admin/v2/users/([^/]+)$").unwrap();
                let path = req.uri().path().to_owned();
                let password_reset = req.method() == salvo::http::Method::POST
                    && path.contains("/admin/v1/reset_password/");
                let blocked = if let Some(captures) = user_write
                    .captures(&path)
                    .filter(|_| req.method() == salvo::http::Method::PUT)
                {
                    match req.parse_json::<Value>().await {
                        Ok(input) => {
                            let user = percent_encoding::percent_decode_str(&captures[1])
                                .decode_utf8()
                                .ok()
                                .and_then(|id| {
                                    palpo::core::OwnedUserId::try_from(id.as_ref()).ok()
                                });
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
