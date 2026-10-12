use hagency_server::config::Config;
use std::path::{Path, PathBuf};

const HOST: &str = include_str!("../../../config/examples/hagency.toml");
const PALPO: &str = include_str!("../../../config/examples/palpo.toml");
const PASION: &str = include_str!("../../../config/examples/pasion.toml");
fn fixture(root: &Path) -> PathBuf {
    let dir = root.join("config/dev");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hagency.toml"), HOST).unwrap();
    std::fs::write(dir.join("palpo.toml"), PALPO).unwrap();
    std::fs::write(dir.join("pasion.toml"), PASION).unwrap();
    dir.join("hagency.toml")
}
#[test]
fn component_config_and_signing_key_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    let mut conf = Config::load(&path).unwrap();
    conf.prepare_signing_key().unwrap();
    let key = conf.matrix.keypair.clone().unwrap();
    let mut reopened = Config::load(&path).unwrap();
    reopened.prepare_signing_key().unwrap();
    assert_eq!(
        reopened.matrix.keypair.as_ref().unwrap().document,
        key.document
    );
    assert_eq!(
        reopened.matrix.keypair.as_ref().unwrap().version,
        key.version
    );
    assert_eq!(conf.internal_origin().as_str(), "http://127.0.0.1:8088/");
}
#[test]
fn public_http_and_mistyped_host_configuration_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    for text in [
        HOST.replace(
            "public_origin = \"http://127.0.0.1:8088\"",
            "public_origin = \"http://public.invalid\"",
        ),
        format!("lissten = \"127.0.0.1:8088\"\n{HOST}"),
        format!("{HOST}\n[matrix]\nserver_name = \"old-inline.invalid\""),
    ] {
        std::fs::write(&path, text).unwrap();
        assert!(Config::load(&path).is_err());
    }
}
#[test]
fn databases_are_distinct_and_required_before_initialization() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    let conf = Config::load(&path).unwrap();
    assert!(conf.database_url.ends_with("/hagency"));
    assert!(conf.matrix.db.url.ends_with("/palpo"));
    assert!(
        conf.pasion
            .as_ref()
            .unwrap()
            .database_url
            .ends_with("/pasion")
    );
    for (component, original, from, to) in [
        ("palpo", PALPO, "palpo", "hagency"),
        ("pasion", PASION, "pasion", "palpo"),
        ("pasion", PASION, "pasion", "hagency"),
    ] {
        fixture(dir.path());
        let shared = original.replace(
            &format!("postgres://hagency:hagency_dev@127.0.0.1:55438/{from}"),
            &format!("postgresql://hagency:hagency_dev@localhost:55438/{to}"),
        );
        std::fs::write(
            path.parent().unwrap().join(format!("{component}.toml")),
            shared,
        )
        .unwrap();
        let error = Config::load(&path)
            .err()
            .expect("shared databases were accepted");
        assert!(error.to_string().contains("different database names"));
        assert!(!dir.path().join("data").exists());
    }
    fixture(dir.path());
    let missing = HOST.replace(
        "database_url = \"postgres://hagency:hagency_dev@127.0.0.1:55438/hagency\"",
        "",
    );
    std::fs::write(&path, missing).unwrap();
    assert!(Config::load(&path).is_err());
    for url in ["sqlite://admin.db", "postgres://localhost"] {
        std::fs::write(&path, HOST.replace(&conf.database_url, url)).unwrap();
        assert!(Config::load(&path).is_err());
    }
}
#[test]
fn component_paths_belong_to_each_file_and_are_listed_for_watchers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let host_dir = root.join("host");
    let palpo_dir = root.join("components/palpo");
    let pasion_dir = root.join("components/pasion");
    for d in [&host_dir, &palpo_dir, &pasion_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let path = host_dir.join("hagency.toml");
    std::fs::write(
        &path,
        HOST.replace("\"palpo.toml\"", "\"../components/palpo/palpo.toml\"")
            .replace("\"pasion.toml\"", "\"../components/pasion/pasion.toml\"")
            .replace("../../data", "data"),
    )
    .unwrap();
    std::fs::write(
        palpo_dir.join("palpo.toml"),
        PALPO.replace("../../data/media", "media").replacen(
            "allow_registration = false",
            "allow_registration = false\nregistration_token_file = \"registration-token\"",
            1,
        ),
    )
    .unwrap();
    std::fs::write(pasion_dir.join("pasion.toml"), PASION.replace("../../resources/pasion", "assets") + "\n[passwords]\nsecret_file = \"pepper\"\n[account]\npassword_registration_enabled = true\n").unwrap();
    let conf = Config::load(&path).unwrap();
    assert_eq!(conf.data_dir, host_dir.join("data"));
    assert_eq!(
        conf.matrix.registration_token_file.as_ref().unwrap(),
        &palpo_dir.join("registration-token")
    );
    match &conf.matrix.storage {
        palpo::config::StorageConfig::Fs { root } => {
            assert_eq!(Path::new(root), palpo_dir.join("media"))
        }
        _ => panic!("unexpected media backend"),
    }
    let pasion = conf.pasion.as_ref().unwrap();
    assert_eq!(pasion.resources_dir, pasion_dir.join("assets"));
    assert_eq!(
        pasion.settings["passwords"]["secret_file"],
        pasion_dir.join("pepper").to_string_lossy().as_ref()
    );
    assert_eq!(
        pasion.settings["account"]["password_registration_enabled"],
        true
    );
    let paths = Config::config_files(&path).unwrap();
    assert_eq!(paths.len(), 3);
    for file in paths {
        assert!(file.is_file());
    }
    std::fs::remove_file(pasion_dir.join("pasion.toml")).unwrap();
    assert!(Config::load(&path).is_err());
    assert!(!host_dir.join("data").exists());
}

#[test]
fn obsolete_hagency_domain_configuration_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    for obsolete in [
        "account_config = \"approval.json\"",
        "retirement_admin_token_file = \"token\"",
        "callback_origins = [\"http://127.0.0.1:3010\"]",
        "[fleet_access]\nallow_self_service = true",
        "[hafleet_access]\nallow_self_service = true",
        "[action_notifications]\nbot_mxid = \"@bot:localhost\"\ntoken_file = \"token\"",
    ] {
        std::fs::write(&path, format!("{HOST}\n{obsolete}\n")).unwrap();
        assert!(
            Config::load(&path).is_err(),
            "accepted obsolete configuration"
        );
    }
}

#[test]
fn offline_request_ttl_is_bounded_and_configurable() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    assert_eq!(Config::load(&path).unwrap().queue.event_ttl_ms, 86_400_000);
    for value in [0_i64, 999, 2_592_000_001] {
        std::fs::write(
            &path,
            HOST.replace(
                "event_ttl_ms = 86400000",
                &format!("event_ttl_ms = {value}"),
            ),
        )
        .unwrap();
        assert!(Config::load(&path).is_err());
    }
    std::fs::write(
        &path,
        HOST.replace("event_ttl_ms = 86400000", "event_ttl_ms = 60000"),
    )
    .unwrap();
    assert_eq!(Config::load(&path).unwrap().queue.event_ttl_ms, 60_000);
}

#[test]
fn removed_home_page_setting_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    std::fs::write(&path, format!("home_page = \"home.html\"\n{HOST}")).unwrap();
    assert!(
        Config::load(&path)
            .err()
            .unwrap()
            .to_string()
            .contains("invalid Hagency configuration")
    );
}

#[tokio::test]
async fn public_home_and_console_use_the_same_frontend_and_content_policy() {
    use hagency_server::frontend::Frontend;
    use salvo::{
        prelude::*,
        test::{ResponseExt, TestClient},
    };
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    let conf = Config::load(&path).unwrap();
    let public_dir = conf.public_dir.as_ref().unwrap();
    std::fs::create_dir_all(public_dir).unwrap();
    std::fs::write(public_dir.join("index.html"), "<h1>Hagency frontend</h1>").unwrap();
    let service = Service::new(Frontend::new(&conf).router());
    for route in [
        "/",
        "/login",
        "/hagency/home",
        "/hagency/projects",
        "/admin/users",
    ] {
        let mut response = TestClient::get(format!("http://127.0.0.1:8088{route}"))
            .add_header("host", "127.0.0.1:8088", true)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        assert!(response.headers().contains_key("content-security-policy"));
        assert_eq!(
            response.take_string().await.unwrap(),
            "<h1>Hagency frontend</h1>"
        );
    }
    for route in ["/unknown", "/_matrix/client/versions", "/auth/"] {
        let response = TestClient::get(format!("http://127.0.0.1:8088{route}"))
            .add_header("host", "127.0.0.1:8088", true)
            .send(&service)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::NOT_FOUND));
    }
    let response = TestClient::get("http://other.test/")
        .add_header("host", "other.test", true)
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::FORBIDDEN));
}
