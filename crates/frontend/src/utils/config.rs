use crate::utils::storage;

pub fn clear_config() {
    let preferences = ["theme", "language"].map(|key| (key, storage::get_item(key)));
    storage::clear();
    for (key, value) in preferences {
        if let Some(value) = value {
            storage::set_item(key, &value);
        }
    }
}

pub fn get_home_server() -> Option<String> {
    storage::get_item("home_server")
}

/// Default OAuth client ID baked into the bundle. Used only as a last-resort
/// fallback when neither `config.json` nor `localStorage["oauth_client_id"]`
/// supplies one — this keeps the build self-contained for the canonical
/// pasion deployment shipped with `examples/pasion.yaml`.
pub const DEFAULT_OAUTH_CLIENT_ID: &str = "01KMQPADM1N000000000000000";

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub server_name: String,
    #[serde(default)]
    pub oauth_enabled: bool,
    #[serde(default)]
    pub pasion_enabled: bool,
    /// Pasion public URL for browser OAuth2 redirects (e.g. http://localhost:7080)
    #[serde(default)]
    pub pasion_public_url: String,
    /// OAuth2 client_id this padmin instance is registered under in pasion.
    /// Defaults to `DEFAULT_OAUTH_CLIENT_ID` when empty.
    #[serde(default)]
    pub oauth_client_id: String,
    /// Base URL of the palpo_admin sidecar (e.g. http://localhost:7090 or a
    /// same-origin proxy path like `/_palpo_admin`). When set, the Server Ops
    /// sidebar group and instance_config fetch become reachable. Leave empty to
    /// fall back to the same-origin default derived in `main.rs`.
    #[serde(default)]
    pub palpo_admin_url: String,
}

pub async fn load_runtime_config() -> RuntimeConfig {
    match gloo_net::http::Request::get("/config.json").send().await {
        Ok(resp) => resp.json::<RuntimeConfig>().await.unwrap_or_default(),
        Err(_) => RuntimeConfig::default(),
    }
}

/// Get the Pasion public URL. Falls back to same-origin /authorize if not configured.
pub fn get_pasion_public_url() -> Option<String> {
    storage::get_item("pasion_public_url")
}

/// Get the configured OAuth2 client_id, falling back to the bundle default.
pub fn get_oauth_client_id() -> String {
    storage::get_item("oauth_client_id")
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_OAUTH_CLIENT_ID.to_string())
}
