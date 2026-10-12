use anyhow::Context;
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    pub listen: SocketAddr,
    pub public_origin: Url,
    /// Hagency administration and relay state; independent of Palpo migrations.
    pub database_url: String,
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
        let base = path.parent().unwrap();
        for p in [&mut host.public_dir, &mut host.pasion_config]
            .into_iter()
            .flatten()
        {
            *p = resolve(base, p);
        }
        host.data_dir = resolve(base, &host.data_dir);
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
        origin(&self.public_origin, true)?;
        anyhow::ensure!(
            self.queue.max_pending > 0
                && self.queue.max_records > 0
                && self.queue.max_bytes > 0
                && self.queue.lease_ms > 0,
            "queue limits must be positive"
        );
        anyhow::ensure!(
            (1_000..=2_592_000_000).contains(&self.queue.event_ttl_ms),
            "queue event_ttl_ms must be between one second and thirty days"
        );
        anyhow::ensure!(
            self.session_ttl_ms.unwrap_or(1800000) > 0 && self.read_timeout_ms.unwrap_or(8000) > 0,
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
