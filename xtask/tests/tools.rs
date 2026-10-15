use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("hagency tools ")
        .tempdir()
        .unwrap();
    let templates = dir.path().join("config/examples");
    fs::create_dir_all(&templates).unwrap();
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for name in ["hagency", "palpo", "pasion"] {
        fs::copy(
            project.join(format!("config/examples/{name}.toml")),
            templates.join(format!("{name}.toml")),
        )
        .unwrap();
    }
    dir
}

fn task(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hagency-xtask"));
    command
        .arg("--project-root")
        .arg(root)
        .env_remove("HAGENCY_DB_PASSWORD");
    command
}

fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn value(path: &Path) -> toml::Value {
    toml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn private_profiles_preserve_credentials_and_refuse_overwrites() {
    let dir = fixture();
    let canonical = dir.path().canonicalize().unwrap();
    let root = canonical.as_path();
    let password = "a:@ /+?#ü'\"";
    success(
        task(root)
            .args([
                "init-config",
                "--dev",
                "--origin",
                "https://localhost:8088",
                "--server-name",
                "example.test",
            ])
            .env("HAGENCY_DB_PASSWORD", password)
            .output()
            .unwrap(),
    );
    let profile = root.join("config/dev");
    let host = value(&profile.join("hagency.toml"));
    assert_eq!(
        host["public_origin"].as_str().unwrap(),
        "https://localhost:8088"
    );
    assert_eq!(
        host["data_dir"].as_str().unwrap(),
        root.join("data").to_str().unwrap()
    );
    assert_eq!(
        value(&profile.join("palpo.toml"))["server_name"]
            .as_str()
            .unwrap(),
        "example.test"
    );
    for (name, connection) in [
        ("hagency", host["database_url"].as_str().unwrap().to_owned()),
        (
            "palpo",
            value(&profile.join("palpo.toml"))["db"]["url"]
                .as_str()
                .unwrap()
                .to_owned(),
        ),
        (
            "pasion",
            value(&profile.join("pasion.toml"))["database"]["uri"]
                .as_str()
                .unwrap()
                .to_owned(),
        ),
    ] {
        let uri = url::Url::parse(&connection).unwrap();
        assert_eq!(uri.path(), format!("/{name}"));
        assert_eq!(
            percent_encoding::percent_decode_str(uri.password().unwrap())
                .decode_utf8()
                .unwrap(),
            password
        );
        assert_eq!(
            fs::metadata(profile.join(format!("{name}.toml")))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(!root.join(".env").exists());
    let original = fs::read(profile.join("hagency.toml")).unwrap();
    assert!(
        !task(root)
            .args(["init-config", "--dev"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(profile.join("hagency.toml")).unwrap(), original);

    success(task(root).arg("init-config").output().unwrap());
    let env = fs::read(root.join(".env")).unwrap();
    assert_eq!(
        fs::metadata(root.join(".env"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let host = value(&root.join("config/docker/hagency.toml"));
    assert_eq!(host["listen"].as_str().unwrap(), "0.0.0.0:8088");
    assert_eq!(host["data_dir"].as_str().unwrap(), "/app/data");
    let production = url::Url::parse(host["database_url"].as_str().unwrap()).unwrap();
    assert_eq!(production.host_str().unwrap(), "postgres");
    assert_eq!(production.password().unwrap().len(), 64);
    let alternative = root.join("other-profile");
    assert!(
        !task(root)
            .arg("init-config")
            .arg("--output-dir")
            .arg(&alternative)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!alternative.exists());
    assert_eq!(fs::read(root.join(".env")).unwrap(), env);
    success(
        task(root)
            .args(["init-config", "--dev", "--output-dir"])
            .arg(&alternative)
            .output()
            .unwrap(),
    );
    assert_eq!(
        url::Url::parse(
            value(&alternative.join("hagency.toml"))["database_url"]
                .as_str()
                .unwrap()
        )
        .unwrap()
        .password(),
        production.password()
    );
}

#[test]
fn copies_local_pasion_resources_without_building_or_starting_servers() {
    let dir = fixture();
    let source = dir.path().join("source with spaces");
    for relative in ["templates/nested", "translations", "policies/cedar"] {
        fs::create_dir_all(source.join(relative)).unwrap();
        fs::write(source.join(relative).join("example.txt"), relative).unwrap();
    }
    let shell = "<html><head><script>window.APP_CONFIG={}</script></head><body><div id=\"main\"></div><script src=\"{{ app_config.script_src }}\"></script></body></html>";
    fs::write(source.join("templates/app.html"), shell).unwrap();
    let output = dir.path().join("prepared assets");
    success(
        task(dir.path())
            .args(["prepare-pasion", "--skip-frontend", "--source"])
            .arg(&source)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap(),
    );
    for (relative, expected) in [
        ("templates/nested", "templates/nested"),
        ("translations", "translations"),
        ("cedar", "policies/cedar"),
    ] {
        assert_eq!(
            fs::read_to_string(output.join(relative).join("example.txt")).unwrap(),
            expected
        );
    }
    assert!(!output.join("public").exists());
    assert_eq!(
        fs::read_to_string(source.join("templates/app.html")).unwrap(),
        shell,
        "The upstream template must remain unchanged"
    );
    let branded = fs::read_to_string(output.join("templates/app.html")).unwrap();
    assert!(branded.contains("/assets/hagency-theme.css"));
    assert!(branded.contains("data-hagency-storage=\"pasion_theme\""));
    assert!(branded.contains("class=\"hagency-auth-brand\""));
    assert!(branded.contains("window.APP_CONFIG={}"));
    assert!(branded.contains("<div id=\"main\"></div>"));
    assert!(branded.contains("src=\"{{ app_config.script_src }}\""));
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Watcher(Child);
impl Drop for Watcher {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_some() {
            return;
        }
        // SAFETY: this child is owned by the test and the watcher handles SIGTERM.
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        let _ = self.0.wait();
    }
}

fn count(root: &Path, event: &str) -> usize {
    fs::read_to_string(root.join("events"))
        .unwrap_or_default()
        .lines()
        .filter(|line| *line == event)
        .count()
}

fn until(root: &Path, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "Timed out; events: {}",
            fs::read_to_string(root.join("events")).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn watcher_keeps_working_server_on_failure_and_watches_external_component_files() {
    let dir = fixture();
    let canonical = dir.path().canonicalize().unwrap();
    let root = canonical.as_path();
    let bin = root.join("bin");
    let target = root.join("custom target");
    let palpo = root.join("local palpo");
    for path in [
        bin.clone(),
        target.join("debug"),
        root.join("crates/backend/src"),
        root.join("external"),
        palpo.join("crates/server/src"),
    ] {
        fs::create_dir_all(path).unwrap();
    }
    fs::write(root.join("Cargo.toml"), "# fixture").unwrap();
    fs::write(root.join("crates/backend/src/main.rs"), "# fixture").unwrap();
    fs::write(palpo.join("crates/server/src/lib.rs"), "# fixture").unwrap();
    let config = root.join("config/host.toml");
    let text =
        "palpo_config = '../external/palpo.toml'\npasion_config = '../external/pasion.toml'\n";
    fs::write(&config, text).unwrap();
    for name in ["palpo", "pasion"] {
        fs::write(root.join(format!("external/{name}.toml")), "# fixture").unwrap();
    }
    executable(
        &bin.join("dx"),
        r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'dioxus 0.7.5 fixture'; exit 0; fi
echo frontend >> events
if [ -f crates/frontend/failure.rs ]; then echo frontend_failed >> events; exit 1; fi
mkdir -p "$CARGO_TARGET_DIR/dx/hagency-frontend/release/web/public/assets"
echo frontend > "$CARGO_TARGET_DIR/dx/hagency-frontend/release/web/public/index.html"
"#,
    );
    fs::create_dir_all(root.join("crates/frontend")).unwrap();
    let metadata = serde_json::json!({"target_directory": target}).to_string();
    executable(
        &bin.join("cargo"),
        &format!(
            r#"#!/bin/sh
if [ "$1" = --config ]; then shift 2; fi
case "$1" in
metadata) cat <<'METADATA'
{metadata}
METADATA
;;
build)
  echo build >> events
  if [ -f crates/backend/src/failure.rs ]; then echo failed >> events; exit 1; fi
  ;;
*) exit 2 ;;
esac
"#
        ),
    );
    executable(
        &target.join("debug/hagency-server"),
        r#"#!/bin/sh
if [ "$3" = --check-config ]; then
  case "$(cat "$2")" in *INVALID*) echo invalid >> events; exit 1 ;; esac
  exit 0
fi
echo start >> events
trap 'echo stop >> events; exit 0' TERM INT
while :; do sleep 1; done
"#,
    );
    let log = fs::File::create(root.join("watcher.log")).unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let mut watcher = Watcher(
        task(root)
            .arg("dev")
            .arg("--config")
            .arg(&config)
            .arg("--palpo-source")
            .arg(&palpo)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    );
    until(root, || count(root, "start") == 1);
    let patches = value(&root.join(".run/dev-cargo.toml"));
    assert_eq!(
        patches["patch"]["https://github.com/palpo-im/palpo.git"]["palpo"]["path"]
            .as_str()
            .unwrap(),
        palpo.join("crates/server").to_str().unwrap()
    );

    fs::write(
        root.join("crates/backend/src/failure.rs"),
        "# compile failure",
    )
    .unwrap();
    until(root, || count(root, "failed") == 1);
    assert_eq!(count(root, "start"), 1);
    assert_eq!(count(root, "stop"), 0);
    fs::remove_file(root.join("crates/backend/src/failure.rs")).unwrap();
    until(root, || count(root, "start") == 2);
    assert_eq!(count(root, "stop"), 1);

    fs::write(&config, format!("{text}# INVALID\n")).unwrap();
    until(root, || count(root, "invalid") == 1);
    assert_eq!(count(root, "start"), 2);
    assert_eq!(count(root, "stop"), 1);
    fs::write(&config, text).unwrap();
    until(root, || count(root, "start") == 3);
    for (index, name) in ["palpo", "pasion"].iter().enumerate() {
        fs::write(root.join(format!("external/{name}.toml")), "# changed\n").unwrap();
        until(root, || count(root, "start") == 4 + index);
    }
    fs::write(palpo.join("crates/server/src/lib.rs"), "# local change").unwrap();
    until(root, || count(root, "start") == 6);
    let builds = count(root, "build");
    fs::write(root.join("crates/frontend/failure.rs"), "# fail").unwrap();
    until(root, || count(root, "frontend_failed") == 1);
    assert_eq!(count(root, "start"), 6);
    assert_eq!(count(root, "stop"), 5);
    fs::remove_file(root.join("crates/frontend/failure.rs")).unwrap();
    until(root, || count(root, "frontend") == 3);
    assert_eq!(count(root, "build"), builds);
    assert_eq!(count(root, "start"), 6);
    // SAFETY: signal only this test's watcher, not the user's service.
    unsafe { libc::kill(watcher.0.id() as i32, libc::SIGTERM) };
    assert!(watcher.0.wait().unwrap().success());
    assert_eq!(count(root, "stop"), 6);
}

#[test]
fn concurrent_frontend_builds_do_not_overlap_dioxus_working_files() {
    let dir = fixture();
    let root = dir.path();
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    executable(
        &bin.join("dx"),
        r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'dioxus 0.7.5 fixture'; exit 0; fi
mkdir .run/dx-active || exit 3
sleep 0.3
mkdir -p "$CARGO_TARGET_DIR/dx/hagency-frontend/release/web/public/assets"
echo compiled > "$CARGO_TARGET_DIR/dx/hagency-frontend/release/web/public/index.html"
rmdir .run/dx-active
"#,
    );
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(paths).unwrap();
    let mut first = task(root)
        .arg("prepare-frontend")
        .env("PATH", &path)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut second = task(root)
        .arg("prepare-frontend")
        .env("PATH", &path)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    assert!(first.wait().unwrap().success());
    assert!(second.wait().unwrap().success());
    assert_eq!(
        fs::read_to_string(root.join("resources/frontend/public/index.html")).unwrap(),
        "compiled\n"
    );
}
