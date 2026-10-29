use crate::{assets, command, frontend};
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

#[derive(Args)]
pub struct Options {
    #[arg(long, default_value = "config/dev/hagency.toml")]
    pub config: PathBuf,
    /// Local Palpo checkout containing the MatrixServer embedding API.
    #[arg(long)]
    pub palpo_source: Option<PathBuf>,
    /// Local Pasion checkout containing PasionServer.
    #[arg(long)]
    pub pasion_source: Option<PathBuf>,
}

pub fn watch(root: &Path, options: &Options, stopping: &AtomicBool) -> Result<()> {
    let config = root.join(&options.config);
    ensure!(
        config.exists(),
        "Create a development configuration with: just init-dev"
    );
    let config = config.canonicalize()?;
    let mut roots = vec![
        root.join("crates/backend"),
        root.join("crates/agent-service"),
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        config.clone(),
        config.parent().unwrap().into(),
    ];
    let mut patches = toml::Table::new();
    if let Some(source) = &options.palpo_source {
        let source = source.canonicalize()?;
        ensure!(
            source.join("crates/server/src/lib.rs").exists(),
            "Palpo checkout must expose MatrixServer"
        );
        patches.insert(
            "https://github.com/palpo-im/palpo.git".into(),
            patch(&[("palpo", source.join("crates/server"))])?,
        );
        roots.extend([source.join("crates"), source.join("Cargo.toml")]);
    }
    let pasion_source = options
        .pasion_source
        .as_ref()
        .map(|path| path.canonicalize())
        .transpose()?;
    if let Some(source) = &pasion_source {
        ensure!(
            source.join("crates/backend/src/embedded.rs").exists(),
            "Pasion checkout must expose PasionServer"
        );
        patches.insert(
            "https://github.com/meldry-com/pasion.git".into(),
            patch(&[
                ("pasion-backend", source.join("crates/backend")),
                ("pasion-config", source.join("crates/config")),
            ])?,
        );
        roots.extend([
            source.join("crates"),
            source.join("Cargo.toml"),
            source.join("templates"),
            source.join("translations"),
            source.join("policies"),
        ]);
    }
    let mut cargo_args = Vec::new();
    if !patches.is_empty() {
        let file = root.join(".run/dev-cargo.toml");
        fs::create_dir_all(file.parent().unwrap())?;
        let value = toml::Table::from_iter([("patch".into(), toml::Value::Table(patches))]);
        fs::write(&file, toml::to_string(&value)?)?;
        cargo_args.extend(["--config".into(), file.into_os_string()]);
    }
    let cargo = || {
        let mut cmd = Command::new("cargo");
        cmd.current_dir(root).args(&cargo_args);
        cmd
    };
    let metadata: Value = serde_json::from_slice(
        &command::output(cargo().args(["metadata", "--format-version", "1", "--no-deps"]))?.stdout,
    )?;
    let binary = Path::new(
        metadata["target_directory"]
            .as_str()
            .context("Cargo target directory missing")?,
    )
    .join("debug/hagency-server");
    let mut server = Server(None);
    let frontend_roots = [
        root.join("crates/frontend"),
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
    ];
    let mut frontend_before = None;
    let mut frontend_ready = false;
    let mut before = None;
    let mut component_roots = Vec::new();
    while !stopping.load(Ordering::Relaxed) {
        // Read references even when the server cannot compile or the config is invalid.
        // Retain previous paths during an incomplete editor write.
        if let Ok(paths) = component_files(&config) {
            component_roots = paths;
        }
        let frontend_current = snapshot(frontend_roots.iter())?;
        if frontend_before.as_ref() != Some(&frontend_current) {
            frontend_before = Some(frontend_current);
            frontend_ready = false;
            println!("Building hagency-frontend…");
            if let Err(error) = frontend::prepare(root, &frontend::Options::default(), stopping) {
                eprintln!("Frontend build failed; keeping the previous assets/server: {error:#}");
                thread::sleep(Duration::from_millis(500));
                continue;
            }
            frontend_ready = true;
        }
        if !frontend_ready {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        let current = snapshot(roots.iter().chain(&component_roots))?;
        if before.as_ref() != Some(&current) {
            before = Some(current);
            println!("Building hagency-server…");
            if command::run(
                cargo().args([
                    "build",
                    "--package",
                    "hagency-server",
                    "--bin",
                    "hagency-server",
                ]),
                stopping,
            )? && !stopping.load(Ordering::Relaxed)
            {
                if let Some(source) = &pasion_source {
                    let prepare = assets::Options {
                        source: Some(source.clone()),
                        output: None,
                        skip_frontend: false,
                    };
                    if let Err(error) = assets::prepare(root, &prepare, stopping) {
                        eprintln!(
                            "Pasion resources failed; keeping the previous server: {error:#}"
                        );
                        continue;
                    }
                }
                if command::run(
                    Command::new(&binary)
                        .current_dir(root)
                        .arg("--config")
                        .arg(&config)
                        .arg("--check-config"),
                    stopping,
                )? && !stopping.load(Ordering::Relaxed)
                {
                    server.stop()?;
                    server.0 = Some(command::spawn(
                        Command::new(&binary)
                            .current_dir(root)
                            .arg("--config")
                            .arg(&config),
                    )?);
                }
            }
        }
        if let Some(child) = &mut server.0
            && child.try_wait()?.is_some()
        {
            eprintln!("Server stopped; edit source/configuration to retry.");
            server.0 = None;
        }
        thread::sleep(Duration::from_millis(500));
    }
    server.stop()
}

struct Server(Option<Child>);

impl Server {
    fn stop(&mut self) -> Result<()> {
        if let Some(child) = &mut self.0 {
            // Match the packaged server's bounded worker/HTTP/Pasion shutdown.
            command::stop_with_grace(child, Duration::from_secs(75))?;
        }
        self.0 = None;
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn patch(packages: &[(&str, PathBuf)]) -> Result<toml::Value> {
    let mut table = toml::Table::new();
    for (name, path) in packages {
        let fields = toml::Table::from_iter([(
            "path".into(),
            toml::Value::String(path.to_str().context("source path must be UTF-8")?.into()),
        )]);
        table.insert((*name).into(), toml::Value::Table(fields));
    }
    Ok(toml::Value::Table(table))
}

fn component_files(config: &Path) -> Result<Vec<PathBuf>> {
    let value: toml::Value = toml::from_str(&fs::read_to_string(config)?)?;
    Ok(["palpo_config", "pasion_config"]
        .into_iter()
        .filter_map(|key| value.get(key).and_then(toml::Value::as_str))
        .map(|path| config.parent().unwrap().join(path))
        .collect())
}

fn snapshot<'a>(roots: impl Iterator<Item = &'a PathBuf>) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for root in roots {
        visit(root, &mut files, true)?;
    }
    Ok(files)
}

fn visit(path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>, explicit: bool) -> Result<()> {
    if path.is_file() {
        if explicit
            || path
                .extension()
                .and_then(|v| v.to_str())
                .is_some_and(|ext| {
                    matches!(
                        ext,
                        "rs" | "toml" | "js" | "css" | "html" | "json" | "cedar" | "ftl"
                    )
                })
        {
            files.insert(path.into(), Sha256::digest(fs::read(path)?).to_vec());
        }
    } else if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_name() != "target" && entry.file_name() != ".git" {
                visit(&entry.path(), files, false)?;
            }
        }
    }
    Ok(())
}
