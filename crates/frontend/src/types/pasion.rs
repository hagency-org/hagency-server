use serde::{Deserialize, Serialize};

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
    pub id: String,
    pub owner_user_id: Option<String>,
    pub owner_client_id: Option<String>,
    pub actor_user_id: String,
    pub scope: String,
    pub human_name: String,
    pub created_at: String,
    pub last_active_at: Option<String>,
    pub last_active_ip: Option<String>,
    pub revoked_at: Option<String>,
    pub expires_at: Option<String>,
    pub access_token: Option<String>,
}
impl PasionPersonalSession {
    pub fn issued_token(&self) -> Result<String, String> {
        self.access_token.clone().filter(|t| !t.is_empty()).ok_or_else(|| "The server accepted the operation but did not return the new credential. The previous token may already be revoked; regenerate again to recover it.".into())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatePersonalSession {
    pub actor_user_id: String,
    pub human_name: String,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u32>,
}
impl CreatePersonalSession {
    pub fn from_input(
        actor: &str,
        name: &str,
        scope: &str,
        lifetime: &str,
    ) -> Result<Self, String> {
        let actor = actor.trim().to_ascii_uppercase();
        if actor.len() != 26
            || actor.as_bytes()[0] > b'7'
            || !actor
                .bytes()
                .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
        {
            return Err("Acting user must be a Pasion account ID (26-character ULID), not a Matrix ID or username.".into());
        }
        if name.trim().is_empty() || scope.trim().is_empty() {
            return Err("Name and scope are required.".into());
        }
        Ok(Self {
            actor_user_id: actor,
            human_name: name.trim().into(),
            scope: scope.trim().into(),
            expires_in: token_lifetime(lifetime)?,
        })
    }
}
pub fn token_lifetime(value: &str) -> Result<Option<u32>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|v| *v > 0)
        .map(Some)
        .ok_or_else(|| {
            "Lifetime must be positive seconds, or blank for a non-expiring token.".into()
        })
}

#[cfg(test)]
mod personal_session_contract_tests {
    use super::*;
    #[test]
    fn actor_owner_and_one_time_secret_remain_distinct() {
        let v = serde_json::json!({"id":"session","owner_user_id":"owner","owner_client_id":null,"actor_user_id":"actor","human_name":"tool","scope":"urn:pasion:admin","created_at":"2026-11-16T00:00:00Z","access_token":"new-secret","expires_at":"2026-11-16T01:00:00Z"});
        let session: PasionPersonalSession = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(session.owner_user_id.as_deref(), Some("owner"));
        assert_eq!(session.actor_user_id, "actor");
        assert_eq!(session.issued_token().unwrap(), "new-secret");
        let mut get = v;
        get.as_object_mut().unwrap().remove("access_token");
        assert!(
            serde_json::from_value::<PasionPersonalSession>(get)
                .unwrap()
                .issued_token()
                .is_err()
        );
        let input = CreatePersonalSession::from_input(
            "01J2KDPHTZYW3TAT1SKVAD63SQ",
            "tool",
            "urn:pasion:admin",
            "3600",
        )
        .unwrap();
        let body = serde_json::to_value(input).unwrap();
        assert_eq!(body["expires_in"], 3600);
        assert!(body.get("owner_user_id").is_none());
        assert!(body.get("user_id").is_none());
        assert!(CreatePersonalSession::from_input("@owner:server", "tool", "scope", "").is_err());
        assert!(token_lifetime("-1").is_err());
        assert_eq!(token_lifetime("").unwrap(), None);
    }
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
    pub channels: Vec<String>,
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
