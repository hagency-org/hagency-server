use serde::{Deserialize, Serialize};

// Generic list response wrapper
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionListResponse<T> {
    #[serde(default)]
    pub data: Vec<T>,
    #[serde(default)]
    pub meta: PasionPaginationMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionPaginationMeta {
    #[serde(default)]
    pub count: u64,
}

// User
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PasionUser {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub locked_at: Option<String>,
    #[serde(default)]
    pub deactivated_at: Option<String>,
    /// Pasion's `can_request_admin` flag: lets the user obtain the
    /// `urn:pasion:admin` scope (and thus sign in to padmin).
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub legacy_guest: bool,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub preferred_locale: Option<String>,
}

/// One page of a cursor-paginated Pasion list.
#[derive(Debug, Clone, Default)]
pub struct PasionPage<T> {
    pub data: Vec<T>,
    pub count: Option<u64>,
    /// Cursor to pass as `page[after]` for the next page, if there is one.
    pub next_cursor: Option<String>,
}

// User Email
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionUserEmail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub confirmed_at: Option<String>,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default)]
    pub created_at: String,
}

// Browser Session
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionBrowserSession {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub last_active_at: Option<String>,
    #[serde(default)]
    pub last_active_ip: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
}

// OAuth2 Session
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionOAuth2Session {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub user_session_id: Option<String>,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub last_active_at: Option<String>,
    #[serde(default)]
    pub last_active_ip: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
}

// Personal Session (API Token)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionPersonalSession {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub last_active_at: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<String>,
    #[serde(default)]
    pub token: Option<String>, // only returned on create/regenerate
}

// Upstream OAuth Provider
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionUpstreamProvider {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub human_name: Option<String>,
    #[serde(default)]
    pub brand_name: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub disabled_at: Option<String>,
    /// "config" or "manual". Config-sourced rows reject edits via the admin API.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    /// Whether a client secret is stored. The secret itself is write-only.
    #[serde(default)]
    pub has_client_secret: bool,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    pub token_endpoint_signing_alg: Option<String>,
    #[serde(default)]
    pub id_token_signed_response_alg: Option<String>,
    #[serde(default)]
    pub userinfo_signed_response_alg: Option<String>,
    #[serde(default)]
    pub discovery_mode: Option<String>,
    #[serde(default)]
    pub pkce_mode: Option<String>,
    #[serde(default)]
    pub response_mode: Option<String>,
    #[serde(default)]
    pub authorization_endpoint_override: Option<String>,
    #[serde(default)]
    pub token_endpoint_override: Option<String>,
    #[serde(default)]
    pub userinfo_endpoint_override: Option<String>,
    #[serde(default)]
    pub jwks_uri_override: Option<String>,
    #[serde(default)]
    pub fetch_userinfo: bool,
    #[serde(default)]
    pub forward_login_hint: bool,
    #[serde(default)]
    pub additional_authorization_parameters: Vec<(String, String)>,
    #[serde(default)]
    pub ui_order: i32,
    #[serde(default)]
    pub on_backchannel_logout: Option<String>,
    #[serde(default)]
    pub claims_imports: Option<serde_json::Value>,
}

// Upstream OAuth Link
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionUpstreamLink {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub human_account_name: Option<String>,
}

// Audit Log
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PasionAuditEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub admin_user_id: Option<String>,
    #[serde(default)]
    pub resource_type: Option<String>,
    #[serde(default)]
    pub resource_id: Option<String>,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionAuditFeedResponse {
    #[serde(default)]
    pub data: Vec<PasionAuditEntry>,
}

// Notification Channel
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionNotificationChannel {
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub configured: bool,
}

// Notification Template
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionNotificationTemplate {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub description: String,
}

// Connector Health
//
// Pasion returns: {provider, homeserver, status: "healthy"|"unhealthy", error?}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PasionConnectorHealth {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub homeserver: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}
