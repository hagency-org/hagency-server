use crate::command;
use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
};

#[derive(Args)]
pub struct Options {
    #[arg(long)]
    pub source: Option<PathBuf>,
    #[arg(long)]
    pub output: Option<PathBuf>,
    #[arg(long)]
    pub skip_frontend: bool,
}

pub fn prepare(root: &Path, options: &Options, stopping: &AtomicBool) -> Result<()> {
    let source = match &options.source {
        Some(path) => path.canonicalize()?,
        None => pinned_source(root, stopping)?,
    };
    let output = std::path::absolute(
        options
            .output
            .clone()
            .unwrap_or_else(|| root.join("resources/pasion")),
    )?;
    fs::create_dir_all(&output)?;
    for (name, relative) in [
        ("templates", "templates"),
        ("translations", "translations"),
        ("cedar", "policies/cedar"),
    ] {
        copy_tree(&source.join(relative), &output.join(name))?;
    }
    if !options.skip_frontend {
        let dx = dioxus_cli(root, stopping)?;
        command::checked(
            Command::new(dx)
                .current_dir(&source)
                .env("CARGO_TARGET_DIR", source.join("target"))
                .args([
                    "build",
                    "--release",
                    "--package",
                    "pasion-frontend",
                    "--base-path",
                    "/auth/",
                    "--debug-symbols",
                    "false",
                    "--locked",
                ]),
            stopping,
        )?;
        copy_tree(
            &source.join("target/dx/pasion-frontend/release/web/public"),
            &output.join("public"),
        )?;
        retain_current_frontend_entrypoint(&output.join("public"))?;
    }
    fs::write(output.join("SOURCE"), format!("{}\n", source.display()))?;
    println!("Prepared Pasion resources: {}", output.display());
    Ok(())
}

/// Embedded Pasion discovers its entry point by scanning assets rather than
/// reading index.html. Dioxus retains old hashed bundles between builds.
fn retain_current_frontend_entrypoint(public: &Path) -> Result<()> {
    let index = fs::read_to_string(public.join("index.html"))?;
    let script = index
        .split("<script")
        .skip(1)
        .find_map(|tag| {
            let src = tag.split("src=\"").nth(1)?.split('"').next()?;
            let name = src.rsplit('/').next()?;
            (name.starts_with("pasion-frontend-") && name.ends_with(".js")).then_some(name)
        })
        .context("Pasion index.html has no hashed frontend entry point")?;
    ensure!(
        public.join("assets").join(script).is_file(),
        "Pasion entry point is missing"
    );
    for entry in fs::read_dir(public.join("assets"))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("pasion-frontend-") && name.ends_with(".js") && name != script {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn pinned_source(root: &Path, stopping: &AtomicBool) -> Result<PathBuf> {
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(root.join("Cargo.toml"))?)?;
    let dependency = &manifest["workspace"]["dependencies"]["pasion-backend"];
    if let Some(path) = dependency.get("path").and_then(toml::Value::as_str) {
        let path = root.join(path).canonicalize()?;
        return Ok(path
            .parent()
            .and_then(Path::parent)
            .context("Pasion backend path needs a workspace parent")?
            .into());
    }
    let repository = dependency
        .get("git")
        .and_then(toml::Value::as_str)
        .context("Pasion dependency needs a git repository")?;
    let revision = dependency
        .get("rev")
        .and_then(toml::Value::as_str)
        .context("Pasion dependency needs a pinned revision")?;
    let source = root.join(".run/pasion-source");
    fs::create_dir_all(source.parent().unwrap())?;
    if !source.exists() {
        command::checked(
            Command::new("git").args(["clone", repository]).arg(&source),
            stopping,
        )?;
    }
    command::checked(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["fetch", "origin", revision]),
        stopping,
    )?;
    command::checked(
        Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["checkout", "--detach", revision]),
        stopping,
    )?;
    Ok(source)
}

pub(crate) fn dioxus_cli(root: &Path, stopping: &AtomicBool) -> Result<PathBuf> {
    if let Ok(output) = Command::new("dx").arg("--version").output()
        && output.status.success()
        && String::from_utf8_lossy(&output.stdout).starts_with("dioxus 0.7.5 ")
    {
        return Ok("dx".into());
    }
    let cached = root.join(".run/tools/dioxus-0.7.5/dx");
    if cached.exists() {
        return Ok(cached);
    }
    let target = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        _ => bail!("Install dx 0.7.5 on this platform before building Pasion."),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        _ => bail!("Install dx 0.7.5 for this architecture before building Pasion."),
    };
    println!("Fetching matching Dioxus CLI 0.7.5…");
    fs::create_dir_all(cached.parent().unwrap())?;
    let download = tempfile::tempdir_in(cached.parent().unwrap())?;
    let archive = download.path().join("dx.tar.gz");
    let checksum = download.path().join("dx.sha256");
    let base =
        format!("https://github.com/DioxusLabs/dioxus/releases/download/v0.7.5/dx-{arch}-{target}");
    for (suffix, destination) in [(".tar.gz", &archive), (".sha256", &checksum)] {
        command::checked(
            Command::new("curl")
                .args([
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "120",
                    "--output",
                ])
                .arg(destination)
                .arg(format!("{base}{suffix}")),
            stopping,
        )?;
    }
    install_dx(&archive, &fs::read_to_string(checksum)?, &cached)?;
    Ok(cached)
}

fn install_dx(archive: &Path, checksum: &str, destination: &Path) -> Result<()> {
    let expected = checksum
        .split_whitespace()
        .next()
        .context("Dioxus checksum is empty")?;
    let mut source = fs::File::open(archive)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    ensure!(actual == expected, "Dioxus CLI checksum did not match");
    let decoder = flate2::read::GzDecoder::new(fs::File::open(archive)?);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_file()
            && entry.path()?.file_name().is_some_and(|name| name == "dx")
        {
            // Extract only the executable into a known path; no archive-supplied paths.
            let mut binary = tempfile::NamedTempFile::new_in(destination.parent().unwrap())?;
            std::io::copy(&mut entry, &mut binary)?;
            binary.flush()?;
            binary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o755))?;
            binary.persist(destination)?;
            return Ok(());
        }
    }
    bail!("Dioxus archive did not contain a dx executable")
}

pub(crate) fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("Read Pasion resources in {}", source.display()))?
    {
        let entry = entry?;
        let source = entry.path();
        let destination = destination.join(entry.file_name());
        if source.is_dir() {
            copy_tree(&source, &destination)?;
        } else {
            fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn current_pasion_bundle_replaces_stale_mount_entrypoints() {
        let root = tempfile::tempdir().unwrap();
        let assets = root.path().join("assets");
        std::fs::create_dir(&assets).unwrap();
        std::fs::write(
            root.path().join("index.html"),
            r#"<script type="module" src="/auth/assets/pasion-frontend-new.js"></script>"#,
        )
        .unwrap();
        std::fs::write(assets.join("pasion-frontend-old.js"), "old mount").unwrap();
        std::fs::write(assets.join("pasion-frontend-new.js"), "new mount").unwrap();
        std::fs::write(assets.join("pasion-frontend_bg-old.wasm"), "existing wasm").unwrap();
        super::retain_current_frontend_entrypoint(root.path()).unwrap();
        assert!(!assets.join("pasion-frontend-old.js").exists());
        assert!(assets.join("pasion-frontend-new.js").exists());
        assert!(assets.join("pasion-frontend_bg-old.wasm").exists());
        std::fs::remove_file(assets.join("pasion-frontend-new.js")).unwrap();
        assert!(super::retain_current_frontend_entrypoint(root.path()).is_err());
    }

    use super::*;

    #[test]
    fn checks_download_before_installing_only_the_dx_executable() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("download.tar.gz");
        let destination = dir.path().join("dx");
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, text) in [("ignored.txt", "ignore"), ("release/dx", "executable")] {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, name, text.as_bytes())
                .unwrap();
        }
        let payload = builder.into_inner().unwrap().finish().unwrap();
        let checksum: String = Sha256::digest(&payload)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        fs::write(&archive, payload).unwrap();
        assert!(install_dx(&archive, "bad-checksum", &destination).is_err());
        assert!(!destination.exists());
        install_dx(
            &archive,
            &format!("{checksum}  download.tar.gz\n"),
            &destination,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&destination).unwrap(), "executable");
        assert_eq!(
            fs::metadata(destination).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!dir.path().join("ignored.txt").exists());
        assert!(!dir.path().join("release").exists());
    }
}
