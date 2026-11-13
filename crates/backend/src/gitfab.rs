//! GitFab acceptance signing: the server's event key, set at deployment, and the route that publishes its
//! public key. The private key stays in its file; nothing here returns or logs it.

use std::sync::Arc;

use hagency_gitfab_events::sign::Key;
use salvo::prelude::*;
use serde_json::{Value, json};

use crate::config::GitfabConfig;

pub const PUBLIC_KEYS_PATH: &str = "hagency/gitfab/v1/public-keys";

#[derive(Clone)]
pub struct Signer {
    key: Arc<Key>,
}

impl Signer {
    /// Fails when the file is missing, is not a key, or is readable by group or others.
    pub fn load(config: &GitfabConfig) -> anyhow::Result<Self> {
        let key = Key::load(&config.signing_key_file, &config.signing_key_id)
            .map_err(|e| anyhow::anyhow!("gitfab.signing_key_file: {e}"))?;
        Ok(Self { key: Arc::new(key) })
    }

    pub fn key(&self) -> &Key {
        &self.key
    }

    pub fn shared_key(&self) -> Arc<Key> {
        self.key.clone()
    }

    /// The reference recorded in each signature's `public_key_ref`.
    pub fn public_key_ref(&self) -> String {
        format!("/{PUBLIC_KEYS_PATH}#{}", self.key.id)
    }

    fn document(&self) -> Value {
        json!({"keys": [{
            "keyId": self.key.id,
            "algorithm": "ed25519",
            "publicKeyBase64": self.key.public_base64(),
            "publicKeyRef": self.public_key_ref(),
        }]})
    }

    pub fn router(&self) -> Router {
        Router::with_path(PUBLIC_KEYS_PATH).get(PublicKeys(self.document()))
    }
}

struct PublicKeys(Value);

#[handler]
impl PublicKeys {
    async fn handle(&self, res: &mut Response) {
        res.render(Json(self.0.clone()));
    }
}

#[cfg(test)]
mod tests {
    use salvo::test::{ResponseExt, TestClient};

    use super::*;

    fn write_key(dir: &std::path::Path, mode: u32) -> GitfabConfig {
        let key = Key::generate("gitfab-1").unwrap();
        let path = dir.join("gitfab-1.key");
        std::fs::write(&path, key.seed_base64()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        GitfabConfig {
            signing_key_file: path,
            signing_key_id: "gitfab-1".into(),
            journal_file: None,
            stage_timeout_minutes: None,
            github_token_file: None,
            gitcode_token_file: None,
            github_api: None,
            gitcode_api: None,
            board_token_file: None,
            team: None,
        }
    }

    #[tokio::test]
    async fn the_route_publishes_only_the_public_key() {
        let dir = tempfile::tempdir().unwrap();
        let config = write_key(dir.path(), 0o600);
        let signer = Signer::load(&config).unwrap();
        let seed = std::fs::read_to_string(&config.signing_key_file).unwrap();
        let service = Service::new(signer.router());
        let mut res = TestClient::get(format!("http://localhost/{PUBLIC_KEYS_PATH}"))
            .send(&service)
            .await;
        assert_eq!(res.status_code, Some(StatusCode::OK));
        let body = res.take_string().await.unwrap();
        assert!(
            !body.contains(seed.trim()),
            "the private seed must never be served"
        );
        let doc: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(doc["keys"][0]["keyId"], "gitfab-1");
        assert_eq!(
            doc["keys"][0]["publicKeyBase64"],
            signer.key().public_base64()
        );
        assert_eq!(
            doc["keys"][0]["publicKeyRef"],
            "/hagency/gitfab/v1/public-keys#gitfab-1"
        );
    }

    #[test]
    fn the_section_is_optional_and_strict() {
        let base = r#"
listen = "127.0.0.1:8088"
public_origin = "http://127.0.0.1:8088"
database_url = "postgres://example:example@localhost/hagency"
palpo_config = "palpo.toml"
"#;
        let host: crate::config::HostConfig = toml::from_str(base).unwrap();
        assert!(host.gitfab.is_none());
        let with = format!(
            "{base}[gitfab]\nsigning_key_file = \"keys/gitfab-1.key\"\nsigning_key_id = \"gitfab-1\"\n"
        );
        let host: crate::config::HostConfig = toml::from_str(&with).unwrap();
        let gitfab = host.gitfab.unwrap();
        assert_eq!(gitfab.signing_key_id, "gitfab-1");
        assert_eq!((gitfab.journal_file, gitfab.stage_timeout_minutes), (None, None));
        let orchestrated = format!("{with}journal_file = \"data/gitfab/events.jsonl\"\nstage_timeout_minutes = 30\n");
        let host: crate::config::HostConfig = toml::from_str(&orchestrated).unwrap();
        assert_eq!(host.gitfab.unwrap().stage_timeout_minutes, Some(30));
        assert!(
            toml::from_str::<crate::config::HostConfig>(&format!("{with}extra = 1\n")).is_err()
        );
        let teamed = format!(
            "{with}[gitfab.team.coordinator]\ncapabilities = [\"planning\"]\n[gitfab.team.implementer]\ncapabilities = [\"rust\", \"http\"]\n[gitfab.team.reviewer]\ncapabilities = [\"code-review\"]\n"
        );
        let team = toml::from_str::<crate::config::HostConfig>(&teamed)
            .unwrap()
            .gitfab
            .unwrap()
            .team
            .unwrap();
        assert_eq!(team["implementer"].capabilities, ["rust", "http"]);
        let declared = hagency_agent_service::orchestrator_team::Team::new(
            team.iter()
                .map(|(r, t)| (r.as_str(), t.capabilities.clone())),
        );
        assert!(declared.is_ok());
        let typo = format!("{with}[gitfab.team.implementer]\ncapabilites = [\"rust\"]\n");
        assert!(toml::from_str::<crate::config::HostConfig>(&typo).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_key_readable_by_others_or_a_missing_key_stops_startup() {
        let dir = tempfile::tempdir().unwrap();
        let err = Signer::load(&write_key(dir.path(), 0o644))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("0600"), "{err}");
        let missing = GitfabConfig {
            signing_key_file: dir.path().join("none.key"),
            signing_key_id: "gitfab-1".into(),
            journal_file: None,
            stage_timeout_minutes: None,
            github_token_file: None,
            gitcode_token_file: None,
            github_api: None,
            gitcode_api: None,
            board_token_file: None,
            team: None,
        };
        assert!(Signer::load(&missing).is_err());
    }
}
