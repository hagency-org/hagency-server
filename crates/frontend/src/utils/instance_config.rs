use std::sync::Mutex;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::utils::storage;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DisableFeatures {
    #[serde(default)]
    pub support: bool,
    #[serde(default)]
    pub actions: bool,
    #[serde(default)]
    pub attributions: bool,
    #[serde(default)]
    pub federation: bool,
    #[serde(default)]
    pub monitoring: bool,
    #[serde(default)]
    pub notifications: bool,
    #[serde(default)]
    pub registration_tokens: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstanceConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub logo_url: Option<String>,
    #[serde(default)]
    pub favicon_url: Option<String>,
    #[serde(default)]
    pub background_url: Option<String>,
    #[serde(default)]
    pub disabled: DisableFeatures,
}

static INSTANCE_CONFIG: OnceLock<Mutex<InstanceConfig>> = OnceLock::new();

fn config_mutex() -> &'static Mutex<InstanceConfig> {
    INSTANCE_CONFIG.get_or_init(|| Mutex::new(InstanceConfig::default()))
}

pub fn get_instance_config() -> InstanceConfig {
    config_mutex().lock().unwrap().clone()
}

pub fn is_palpo_admin_enabled() -> bool {
    storage::get_item("palpo_admin_url").is_some()
}

pub fn get_palpo_admin_url() -> Option<String> {
    storage::get_item("palpo_admin_url")
}
