use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;
use url::Url;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    pub listen: SocketAddr,
    pub public_origin: Url,
    /// Hagency administration and relay state; independent of Palpo migrations.
    pub database_url: String,
    /// Optional kill switch; no new configuration is required for existing hosts.
    #[serde(default = "board_enabled")]
    pub project_board_enabled: bool,
    /// Personal Agent encrypted-event routing; pooled Agents use executor devices.
    #[serde(default)]
    pub personal_encrypted_delivery: hagency_agent_service::transport::EncryptedDelivery,
    #[serde(default)]
    pub public_dir: Option<PathBuf>,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default)]
    pub session_ttl_ms: Option<u64>,
    #[serde(default)]
    pub read_timeout_ms: Option<u64>,
    #[serde(default)]
    pub queue: QueueConfig,
    #[serde(default)]
    pub pasion_config: Option<PathBuf>,
    pub palpo_config: PathBuf,
    #[serde(default)]
    pub management: ManagementConfig,
    /// GitFab acceptance event signing. Absent leaves it off; existing hosts need no new input.
    #[serde(default)]
    pub gitfab: Option<GitfabConfig>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitfabConfig {
    /// Base64 Ed25519 seed, mode 0600, outside the image and Git. Not the Palpo server key.
    pub signing_key_file: PathBuf,
    /// Published with the public key and recorded in every signature.
    pub signing_key_id: String,
    /// Append-only GitFab event journal. Setting it runs the workflow orchestrator.
    #[serde(default)]
    pub journal_file: Option<PathBuf>,
    /// A stage without a result after this long stops for a Human. Default 120.
    #[serde(default)]
    pub stage_timeout_minutes: Option<u32>,
    /// Token files (mode 0600) that let the orchestrator open Pull Requests and publish Releases.
    /// A backend without one is left alone.
    #[serde(default)]
    pub github_token_file: Option<PathBuf>,
    #[serde(default)]
    pub gitcode_token_file: Option<PathBuf>,
    /// API roots for self-hosted instances; the public ones by default.
    #[serde(default)]
    pub github_api: Option<String>,
    #[serde(default)]
    pub gitcode_api: Option<String>,
    /// Read-only token (mode 0600) the GitFab board presents to read Agent status.
    #[serde(default)]
    pub board_token_file: Option<PathBuf>,
    /// The fixed Agent team: `[gitfab.team.coordinator|implementer|reviewer]` each with
    /// `capabilities`. Every role a stage routes to must be declared; Plans may then give Modules
    /// only capabilities the implementer declares. Absent leaves Plans unchecked against a team.
    #[serde(default)]
    pub team: Option<std::collections::BTreeMap<String, GitfabTeamRole>>,
    /// The repository each Hagency project's runs use with the Git tokens:
    /// `[gitfab.repos."prj_…"]` with `backend = "github"|"gitcode"` and `repo = "owner/name"`.
    /// With a token configured, a run may only target its own project's repository.
    #[serde(default)]
    pub repos: std::collections::BTreeMap<String, GitfabRepo>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitfabRepo {
    pub backend: String,
    pub repo: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitfabTeamRole {
    pub capabilities: Vec<String>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ManagementConfig {
    /// Defaults to enabled when Pasion supplies the server's authentication.
    pub enabled: Option<bool>,
    /// Overrides the default Pasion administrator role with one local MXID.
    pub administrator: Option<String>,
    /// Off until the native shared ledger and reservation capability are ready.
    #[serde(default)]
    pub enable_project_reservations: Option<bool>,
    /// Host-private only until the native parent settlement adapter is proven.
    #[serde(default)]
    pub enable_parent_settlement: bool,
    /// Off until owner Matrix creation and recovery are integrated.
    #[serde(default)]
    pub enable_project_setup: bool,
    /// Admission-only, unadvertised until complete native managed execution.
    #[serde(default)]
    pub enable_agent_admission: Option<bool>,
    /// Host-private delivery only; does not enable shared model execution.
    #[serde(default)]
    pub enable_agent_executor: bool,
    /// Matrix membership only; never enables provider/runtime/work support.
    #[serde(default)]
    pub enable_agent_matrix_provisioning: bool,
    /// Finite no-prompts runtime qualification only; never shared work.
    #[serde(default)]
    pub enable_agent_runtime_provisioning: bool,
    /// Default off until paired shared execution is accepted.
    #[serde(default)]
    pub enable_shared_execution: bool,
    /// Append-only successor process issuance; separate from original v1.
    #[serde(default)]
    pub enable_shared_successors: bool,
    /// Retention-only until the paired demand consumer is qualified.
    #[serde(default)]
    pub enable_shared_durable_intake: bool,
    #[serde(default)]
    pub enable_shared_demands: bool,
    /// Immutable bounded same-thread context; paired input contract required.
    #[serde(default)]
    pub enable_shared_thread_context: bool,
    #[serde(default)]
    pub enable_agent_budget_deltas: bool,
    #[serde(default)]
    pub enable_shared_budget_execution: bool,
    /// Pool grant renewal/amendment and chain-long pool Agent lifetime. Off
    /// until the owners' desktops support succession.
    #[serde(default)]
    pub enable_pool_grant_succession: bool,
}
impl ManagementConfig {
    fn apply_pool_defaults(&mut self, enabled: bool) {
        self.enable_project_reservations.get_or_insert(enabled);
        self.enable_agent_admission.get_or_insert(enabled);
    }
}
/// Fully loaded runtime configuration; files stay owned by their components.
#[derive(Clone)]
pub struct Config {
    pub host: HostConfig,
    pub matrix: palpo::config::ServerConfig,
    pub pasion: Option<crate::pasion::PasionConfig>,
}
impl std::ops::Deref for Config {
    type Target = HostConfig;
    fn deref(&self) -> &HostConfig {
        &self.host
    }
}
impl std::ops::DerefMut for Config {
    fn deref_mut(&mut self) -> &mut HostConfig {
        &mut self.host
    }
}
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QueueConfig {
    pub max_pending: usize,
    pub max_records: usize,
    pub max_bytes: usize,
    pub lease_ms: i64,
    /// Maximum age of an unstarted Agent request, from trusted receipt time.
    pub event_ttl_ms: i64,
}
impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            max_pending: 1000,
            max_records: 10000,
            max_bytes: 16 * 1024 * 1024,
            lease_ms: 30000,
            event_ttl_ms: 86_400_000,
        }
    }
}
impl QueueConfig {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.max_pending > 0 && self.max_records > 0 && self.max_bytes > 0,
            "queue limits must be positive"
        );
        anyhow::ensure!(
            (1_000..=120_000).contains(&self.lease_ms),
            "queue lease_ms must be between one second and two minutes"
        );
        anyhow::ensure!(
            (1_000..=2_592_000_000).contains(&self.event_ttl_ms),
            "queue event_ttl_ms must be between one second and thirty days"
        );
        Ok(())
    }
}
impl HostConfig {
    pub(crate) fn browser_session_ttl_ms(&self) -> anyhow::Result<u64> {
        let ttl = self.session_ttl_ms.unwrap_or(1_800_000);
        anyhow::ensure!(
            (1_000..=86_400_000).contains(&ttl),
            "browser session TTL must be between one second and one day"
        );
        Ok(ttl)
    }
}
fn board_enabled() -> bool {
    true
}
fn default_data_dir() -> PathBuf {
    PathBuf::from("data")
}
impl Config {
    /// Paths are relative to the file that declares them, never the shell CWD.
    pub fn config_files(path: impl AsRef<Path>) -> anyhow::Result<Vec<PathBuf>> {
        let path = path.as_ref().canonicalize()?;
        let host = read_host(&path)?;
        let base = path.parent().unwrap();
        let mut paths = vec![path.clone(), resolve(base, &host.palpo_config)];
        if let Some(pasion) = host.pasion_config {
            paths.push(resolve(base, &pasion));
        }
        Ok(paths)
    }
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().canonicalize()?;
        let mut host = read_host(&path)?;
        const ENABLE_RESOURCE_POOLS: &str = "HAGENCY_ENABLE_RESOURCE_POOLS";
        match std::env::var(ENABLE_RESOURCE_POOLS) {
            Ok(value) => host.management.apply_pool_defaults(
                value
                    .parse()
                    .with_context(|| format!("{ENABLE_RESOURCE_POOLS} must be true or false"))?,
            ),
            Err(std::env::VarError::NotPresent) => {}
            Err(error) => return Err(error).context(ENABLE_RESOURCE_POOLS),
        }
        let base = path.parent().unwrap();
        for p in [&mut host.public_dir, &mut host.pasion_config]
            .into_iter()
            .flatten()
        {
            *p = resolve(base, p);
        }
        host.data_dir = resolve(base, &host.data_dir);
        if let Some(gitfab) = &mut host.gitfab {
            gitfab.signing_key_file = resolve(base, &gitfab.signing_key_file);
            gitfab.journal_file = gitfab.journal_file.as_ref().map(|p| resolve(base, p));
            gitfab.github_token_file = gitfab.github_token_file.as_ref().map(|p| resolve(base, p));
            gitfab.gitcode_token_file =
                gitfab.gitcode_token_file.as_ref().map(|p| resolve(base, p));
            gitfab.board_token_file = gitfab.board_token_file.as_ref().map(|p| resolve(base, p));
        }
        host.palpo_config = resolve(base, &host.palpo_config)
            .canonicalize()
            .context("cannot open palpo_config")?;
        let matrix_base = host.palpo_config.parent().unwrap();
        let raw = std::fs::read_to_string(&host.palpo_config)?;
        let mut settings: serde_json::Value =
            toml::from_str(&raw).context("invalid Palpo TOML configuration")?;
        resolve_file_references(&mut settings, matrix_base);
        let mut matrix: palpo::config::ServerConfig =
            serde_json::from_value(settings).context("invalid Palpo configuration")?;
        if let palpo::config::StorageConfig::Fs { root } = &mut matrix.storage {
            *root = resolve(matrix_base, Path::new(root))
                .to_string_lossy()
                .into_owned();
        }
        let pasion = host
            .pasion_config
            .as_ref()
            .map(crate::pasion::PasionConfig::load)
            .transpose()?;
        let conf = Self {
            host,
            matrix,
            pasion,
        };
        conf.validate()?;
        Ok(conf)
    }
    pub fn prepare_signing_key(&mut self) -> anyhow::Result<()> {
        use base64::Engine;
        if self.matrix.keypair.is_some() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.data_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.data_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = self.data_dir.join("matrix-signing-key.json");
        let pair = if path.exists() {
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path)?)?
        } else {
            let der = palpo::core::signatures::Ed25519KeyPair::generate()?;
            let pair = serde_json::json!({"document":base64::engine::general_purpose::STANDARD.encode(der),"version":uuid::Uuid::new_v4().simple().to_string()[..8].to_string()});
            let tmp = self
                .data_dir
                .join(format!(".signing-key-{}", uuid::Uuid::new_v4()));
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            use std::io::Write;
            let mut file = opts.open(&tmp)?;
            file.write_all(&serde_json::to_vec(&pair)?)?;
            file.sync_all()?;
            // An atomic hard link avoids replacing a key created by a concurrent startup.
            match std::fs::hard_link(&tmp, &path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
            std::fs::remove_file(tmp)?;
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path)?)?
        };
        let document = pair["document"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid signing key document"))?
            .to_string();
        let version = pair["version"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid signing key version"))?
            .to_string();
        let der = base64::engine::general_purpose::STANDARD.decode(&document)?;
        palpo::core::signatures::Ed25519KeyPair::from_der(&der, version.clone())?;
        self.matrix.keypair = Some(palpo::config::KeypairConfig { document, version });
        Ok(())
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(gitfab) = &self.gitfab {
            crate::gitfab::Signer::load(gitfab)?;
            let agent_service = self.pasion.as_ref().is_some_and(|p| p.delegate_matrix_auth);
            if gitfab.journal_file.is_some() && !agent_service {
                anyhow::bail!(
                    "gitfab.journal_file needs pasion.delegate_matrix_auth (the agent service)"
                );
            }
            if gitfab.stage_timeout_minutes == Some(0) {
                anyhow::bail!("gitfab.stage_timeout_minutes must be at least 1");
            }
        }
        if let Some(administrator) = &self.management.administrator {
            hagency_agent_service::management_types::ManagementPolicy::new(
                administrator.clone(),
                self.matrix.server_name.as_str(),
            )?;
        }

        origin(&self.public_origin, true)?;
        self.queue.validate()?;
        self.browser_session_ttl_ms()?;
        anyhow::ensure!(
            self.read_timeout_ms.unwrap_or(8000) > 0,
            "timeouts must be positive"
        );
        anyhow::ensure!(
            !self.listen.ip().is_unspecified()
                || !self.public_origin.host_str().unwrap_or("").is_empty(),
            "public origin is required"
        );
        anyhow::ensure!(
            !self.matrix.admin.console_automatic,
            "embedded Palpo does not support an automatic interactive console"
        );
        let admin_db = postgres_database(&self.database_url, "database_url")?;
        let matrix_db = postgres_database(&self.matrix.db.url, "matrix.db.url")?;
        anyhow::ensure!(
            admin_db.path() != matrix_db.path(),
            "Hagency and Palpo must use different database names"
        );
        if let Some(pasion) = &self.pasion {
            pasion.validate(self)?;
        }
        self.matrix.check()?;
        Ok(())
    }
    pub fn internal_origin(&self) -> Url {
        let ip = if self.listen.ip().is_unspecified() {
            if self.listen.is_ipv6() {
                "[::1]".into()
            } else {
                "127.0.0.1".into()
            }
        } else if self.listen.is_ipv6() {
            format!("[{}]", self.listen.ip())
        } else {
            self.listen.ip().to_string()
        };
        Url::parse(&format!("http://{ip}:{}/", self.listen.port())).unwrap()
    }
}
fn read_host(path: &Path) -> anyhow::Result<HostConfig> {
    toml::from_str(&std::fs::read_to_string(path)?)
        .context("invalid Hagency configuration; use palpo_config/pasion_config to reference component files")
}
pub(crate) fn resolve(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}
/// Native config secret-file references, including client_secret = { file = ... }.
pub(crate) fn resolve_file_references(value: &mut serde_json::Value, base: &Path) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if (key == "file" || key.ends_with("_file")) && value.is_string() {
                    *value = serde_json::json!(resolve(base, Path::new(value.as_str().unwrap())));
                } else {
                    resolve_file_references(value, base);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                resolve_file_references(item, base);
            }
        }
        _ => (),
    }
}
pub(crate) fn postgres_database(value: &str, field: &str) -> anyhow::Result<Url> {
    let url = Url::parse(value).map_err(|e| anyhow::anyhow!("invalid {field}: {e}"))?;
    anyhow::ensure!(
        ["postgres", "postgresql"].contains(&url.scheme())
            && !url.path().trim_start_matches('/').is_empty(),
        "{field} requires a PostgreSQL URL with an explicit database name"
    );
    Ok(url)
}
pub fn origin(url: &Url, public: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        ["http", "https"].contains(&url.scheme())
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "use a credential-free HTTP(S) origin"
    );
    anyhow::ensure!(
        !public
            || url.scheme() == "https"
            || matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        "public origins require HTTPS"
    );
    Ok(())
}

#[cfg(test)]
mod encrypted_delivery_tests {
    use hagency_agent_service::transport::EncryptedDelivery;

    use super::HostConfig;

    #[test]
    fn management_defaults_to_pasion_and_explicit_pool_settings_override_the_environment() {
        let base = "listen='127.0.0.1:8088'\npublic_origin='http://127.0.0.1:8088'\ndatabase_url='postgres://localhost/hagency'\npalpo_config='palpo.toml'\n";
        let mut config: HostConfig = toml::from_str(base).unwrap();
        assert!(config.management.enabled.unwrap_or(true));
        assert!(config.management.administrator.is_none());
        assert!(!config.management.enable_agent_admission.unwrap_or(false));
        config.management.apply_pool_defaults(true);
        assert_eq!(config.management.enable_project_reservations, Some(true));
        assert_eq!(config.management.enable_agent_admission, Some(true));

        let mut config: HostConfig = toml::from_str(&format!(
            "{base}[management]\nenabled=false\nadministrator='@reviewer:example.test'\nenable_agent_admission=false\n"
        )).unwrap();
        config.management.apply_pool_defaults(true);
        assert_eq!(config.management.enabled, Some(false));
        assert_eq!(
            config.management.administrator.as_deref(),
            Some("@reviewer:example.test")
        );
        assert_eq!(config.management.enable_project_reservations, Some(true));
        assert_eq!(config.management.enable_agent_admission, Some(false));
    }

    #[test]
    fn startup_and_check_config_share_browser_ttl_boundaries() {
        let base = "listen='127.0.0.1:8088'\npublic_origin='http://127.0.0.1:8088'\ndatabase_url='postgres://localhost/hagency'\npalpo_config='palpo.toml'\n";
        for (ttl, valid) in [
            (0, false),
            (999, false),
            (1_000, true),
            (86_400_000, true),
            (86_400_001, false),
        ] {
            let config: HostConfig =
                toml::from_str(&format!("{base}session_ttl_ms={ttl}\n")).unwrap();
            assert_eq!(config.browser_session_ttl_ms().is_ok(), valid, "{ttl}");
        }
        for (lease_ms, valid) in [
            (999, false),
            (1_000, true),
            (120_000, true),
            (120_001, false),
        ] {
            assert_eq!(
                super::QueueConfig {
                    lease_ms,
                    ..Default::default()
                }
                .validate()
                .is_ok(),
                valid,
                "{lease_ms}"
            );
        }
    }

    #[test]
    fn personal_encrypted_delivery_defaults_to_executor_and_accepts_brokered_mode() {
        let config = r#"
listen = "127.0.0.1:8088"
public_origin = "http://127.0.0.1:8088"
database_url = "postgres://example:example@localhost/hagency"
palpo_config = "palpo.toml"
"#;
        let default: HostConfig = toml::from_str(config).unwrap();
        assert_eq!(
            default.personal_encrypted_delivery,
            EncryptedDelivery::ExecutorDevice
        );
        let brokered: HostConfig = toml::from_str(&format!(
            "{config}personal_encrypted_delivery = \"ciphertext_inbox\"\n"
        ))
        .unwrap();
        assert_eq!(
            brokered.personal_encrypted_delivery,
            EncryptedDelivery::CiphertextInbox
        );
        assert!(
            toml::from_str::<HostConfig>(&format!(
                "{config}personal_encrypted_delivery = \"invalid\"\n"
            ))
            .is_err()
        );
    }
}
