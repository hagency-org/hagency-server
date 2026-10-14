use crate::{assets, command};
use anyhow::Result;
use clap::Args;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Args, Default)]
pub struct Options {
    #[arg(long)]
    pub output: Option<PathBuf>,
}

pub fn prepare(root: &Path, options: &Options, stopping: &AtomicBool) -> Result<()> {
    fs::create_dir_all(root.join(".run"))?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".run/frontend-build.lock"))?;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::ensure!(
                    !stopping.load(Ordering::Relaxed),
                    "Frontend build interrupted"
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
    let dx = assets::dioxus_cli(root, stopping)?;
    let target = root.join(".run/frontend-target");
    command::checked(
        Command::new(dx)
            .current_dir(root)
            .env("CARGO_TARGET_DIR", &target)
            .args([
                "build",
                "--release",
                "--package",
                "hagency-frontend",
                "--base-path",
                "/",
                "--debug-symbols",
                "false",
                "--locked",
            ]),
        stopping,
    )?;
    let source = target.join("dx/hagency-frontend/release/web/public");
    let output = options
        .output
        .clone()
        .unwrap_or_else(|| root.join("resources/frontend/public"));
    // Publish hashed assets before the index so an active server never refers to missing files.
    fs::create_dir_all(&output)?;
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        if entry.file_name() == "index.html" {
            continue;
        }
        if entry.path().is_dir() {
            assets::copy_tree(&entry.path(), &output.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), output.join(entry.file_name()))?;
        }
    }
    let index = tempfile::NamedTempFile::new_in(&output)?;
    fs::copy(source.join("index.html"), index.path())?;
    index.persist(output.join("index.html"))?;
    gzip_public_files(&output)?;
    println!("Prepared Hagency frontend: {}", output.display());
    Ok(())
}

// Generate deterministic gzip siblings in CI, never compress multi-MB WASM per request.
fn gzip_public_files(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            gzip_public_files(&path)?;
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("js" | "css" | "wasm" | "html" | "svg" | "json")
        ) {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            encoder.write_all(&fs::read(&path)?)?;
            let mut destination = path.as_os_str().to_os_string();
            destination.push(".gz");
            fs::write(PathBuf::from(destination), encoder.finish()?)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn precompressed_files_round_trip_and_are_deterministic() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.wasm"), b"\0asm frontend fixture").unwrap();
        super::gzip_public_files(dir.path()).unwrap();
        let first = std::fs::read(dir.path().join("app.wasm.gz")).unwrap();
        super::gzip_public_files(dir.path()).unwrap();
        assert_eq!(
            first,
            std::fs::read(dir.path().join("app.wasm.gz")).unwrap()
        );
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(first.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, b"\0asm frontend fixture");
        assert!(!dir.path().join("app.wasm.gz.gz").exists());
    }
}
