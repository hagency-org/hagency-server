//! Mandatory service-level registration for the integrated Agent product.
//! Uses Palpo's registration store; no per-user service installation.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use serde_json::json;

use crate::config::Config;

const ID: &str = "hagency_agents_v1";
mod device;

pub async fn prepare(conf: &Config) -> anyhow::Result<palpo::core::appservice::Registration> {
    let registration = load_or_create(
        &conf.data_dir.join("agent-appservice.json"),
        conf.matrix.server_name.as_str(),
        conf.internal_origin().as_str(),
    )?;
    device::prepare(&registration).await?;
    Ok(registration)
}
fn load_or_create(
    path: &Path,
    server: &str,
    callback: &str,
) -> anyhow::Result<palpo::core::appservice::Registration> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("missing Appservice directory"))?,
    )?;
    if !path.try_exists()? {
        // Never expose a partially written final credential file. A competing
        // initializer wins by atomic no-clobber persist, then both read it.
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        let body = json!({"id":ID,"url":callback.trim_end_matches('/'),
                "as_token":hex::encode(rand::random::<[u8;32]>()),
                "hs_token":hex::encode(rand::random::<[u8;32]>()),
                "sender_localpart":"_hagency_service",
                "namespaces":{"users":[{"exclusive":true,"regex":format!("^@_hagency_[a-z0-9_]+:{}$",regex::escape(server))}],"aliases":[],"rooms":[]},
                "rate_limited":true,"receive_ephemeral":false,"io.element.msc4190":true});
        file.write_all(&serde_json::to_vec(&body)?)?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(path) {
            Ok(_) => {
                std::fs::File::open(path.parent().unwrap())?.sync_all()?;
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error.into()),
        }
    }
    // Refuse symlinks and broadly readable secret files before reading credentials.
    let meta = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        meta.is_file() && !meta.file_type().is_symlink(),
        "Appservice credentials must be a regular private file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            meta.permissions().mode() & 0o077 == 0,
            "Appservice credential permissions must be owner-only"
        );
    }
    anyhow::ensure!(
        meta.len() <= 16384,
        "Appservice credentials exceed size limit"
    );
    let mut raw = Vec::new();
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let actual = file.metadata()?;
    anyhow::ensure!(
        actual.is_file() && actual.len() <= 16384,
        "Appservice credentials must be a bounded regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            actual.permissions().mode() & 0o077 == 0,
            "Appservice credentials must remain owner-only"
        );
    }
    file.take(16385).read_to_end(&mut raw)?;
    let mut registration: palpo::core::appservice::Registration = serde_json::from_slice(&raw)?;
    let expected = format!("^@_hagency_[a-z0-9_]+:{}$", regex::escape(server));
    let value = serde_json::to_value(&registration)?;
    anyhow::ensure!(
        registration.id == ID
            && registration.as_token.len() == 64
            && registration.hs_token.len() == 64
            && registration.url.as_deref() == Some(callback.trim_end_matches('/'))
            && registration.sender_localpart == "_hagency_service"
            && value["namespaces"]["users"] == json!([{"exclusive":true,"regex":expected}])
            && value["namespaces"]["aliases"]
                .as_array()
                .is_none_or(|v| v.is_empty())
            && value["namespaces"]["rooms"]
                .as_array()
                .is_none_or(|v| v.is_empty()),
        "Appservice credentials belong to another deployment or namespace"
    );
    // This built-in service already owns exactly the validated Agent namespace.
    // Old private files retain their original tokens and bytes. The database
    // capability upgrade compares the complete original identity under a lock.
    registration.device_management = true;
    Ok(registration)
}
/// Palpo's state reader skips PDUs it cannot load. Authorization requires every
/// event from the same immutable state frame, not a silently incomplete subset.
pub fn validate_state_completeness(
    expected: &[String],
    events: &[serde_json::Value],
) -> hagency_agent_service::Result<()> {
    let expected_count = expected.len();
    let expected = expected
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let actual = events
        .iter()
        .map(|event| event["event_id"].as_str())
        .collect::<Option<std::collections::BTreeSet<_>>>();
    if expected.len() != expected_count
        || actual
            .as_ref()
            .is_none_or(|actual| actual.len() != events.len() || actual != &expected)
    {
        return Err(hagency_agent_service::Error::Unavailable(
            "matrix_state_incomplete",
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_identity_survives_restart_and_rejects_moved_deployment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-appservice.json");
        let a = load_or_create(&path, "example.test", "http://127.0.0.1:8088/").unwrap();
        let b = load_or_create(&path, "example.test", "http://127.0.0.1:8088/").unwrap();
        assert_eq!(a.as_token, b.as_token);
        assert_eq!(a.hs_token, b.hs_token);
        assert_ne!(a.as_token, a.hs_token);
        assert!(
            a.device_management,
            "Agent SDK identities require fixed appservice devices"
        );
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("io.element.msc4190");
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let original = fs::read(&path).unwrap();
        let upgraded = load_or_create(&path, "example.test", "http://127.0.0.1:8088/").unwrap();
        assert!(upgraded.device_management);
        assert_eq!(upgraded.as_token, a.as_token);
        assert_eq!(upgraded.hs_token, a.hs_token);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(load_or_create(&path, "another.test", "http://127.0.0.1:8088/").is_err());
        assert!(load_or_create(&path, "example.test", "http://127.0.0.1:9090/").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load_or_create(&path, "example.test", "http://127.0.0.1:8088/").is_err());
        }
    }
    #[test]
    fn authorization_rejects_partial_duplicate_or_wrong_state_frame_events() {
        let expected = vec!["$member".into(), "$powers".into(), "$encryption".into()];
        let complete = vec![
            json!({"event_id":"$encryption"}),
            json!({"event_id":"$member"}),
            json!({"event_id":"$powers"}),
        ];
        assert!(validate_state_completeness(&expected, &complete).is_ok());
        assert!(validate_state_completeness(&expected, &complete[..2]).is_err());
        let duplicate = vec![
            complete[0].clone(),
            complete[0].clone(),
            complete[1].clone(),
        ];
        assert!(validate_state_completeness(&expected, &duplicate).is_err());
        let wrong = vec![
            complete[0].clone(),
            complete[1].clone(),
            json!({"event_id":"$other-frame"}),
        ];
        assert!(validate_state_completeness(&expected, &wrong).is_err());
        assert!(validate_state_completeness(&expected, &[json!({"content":{}})]).is_err());
    }
}
