//! `gitfab-runner`: deploy a GitFab Release as a container and record a signed receipt, or roll back to
//! the previous good Release. Exit 0 when the deployment passed its checks, 1 when it did not (the
//! receipt says so and whether the previous Release was restored), 2 for usage or setup errors.
//! Prints a JSON summary; nothing secret is printed.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use hagency_gitfab_events::sign::Key;
use hagency_gitfab_runner::{Docker, Probe, Receipts, Release, deploy, rollback};
use serde_json::json;

#[derive(Parser)]
#[command(
    name = "gitfab-runner",
    about = "Deploy GitFab Releases and record signed receipts"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct Common {
    /// Runner signing key (base64 seed, mode 0600).
    #[arg(long)]
    key: PathBuf,
    #[arg(long)]
    key_id: String,
    /// Recorded in each signature; defaults to `keys/<key-id>.pub`.
    #[arg(long)]
    public_key_ref: Option<String>,
    #[arg(long)]
    runner_id: String,
    /// Receipt journal (JSONL), appended to.
    #[arg(long)]
    receipts: PathBuf,
    /// Host port on 127.0.0.1 and the port the service listens on inside the container.
    #[arg(long)]
    port: u16,
    #[arg(long, default_value_t = 8080)]
    container_port: u16,
    #[arg(long, default_value = "/healthz")]
    health_path: String,
    #[arg(long, default_value = "/version")]
    version_path: String,
    /// Seconds to wait for the health check.
    #[arg(long, default_value_t = 60)]
    wait: u64,
}

#[derive(Subcommand)]
// Parsed once per run; boxing the larger variant would buy nothing.
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Build `--context` at the Release commit, start it and record the receipt.
    Deploy {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        project: String,
        #[arg(long)]
        task: String,
        #[arg(long)]
        backend: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        commit: String,
        #[arg(long)]
        tag: String,
        /// Checked-out source with its Dockerfile.
        #[arg(long)]
        context: PathBuf,
        /// Where `<project>-<tag>.spdx.json` is written.
        #[arg(long)]
        sbom_dir: PathBuf,
        /// Push the image to this registry (`host[:port]/namespace`) before starting it; the receipt
        /// then names it by its manifest digest.
        #[arg(long)]
        registry: Option<String>,
        /// Sign the published image with this cosign key (mode 0600; its password in COSIGN_PASSWORD).
        #[arg(long, requires = "registry")]
        cosign_key: Option<PathBuf>,
    },
    /// Return the project to its good Release before the current one.
    Rollback {
        #[command(flatten)]
        common: Common,
        #[arg(long)]
        project: String,
    },
}

fn setup(common: &Common) -> Result<(Probe, Receipts), String> {
    let key = Key::load(&common.key, &common.key_id).map_err(|e| format!("runner key: {e}"))?;
    let probe = Probe {
        port: common.port,
        container_port: common.container_port,
        health_path: common.health_path.clone(),
        version_path: common.version_path.clone(),
        wait: Duration::from_secs(common.wait),
    };
    let receipts = Receipts {
        journal: common.receipts.clone(),
        key,
        public_key_ref: common
            .public_key_ref
            .clone()
            .unwrap_or_else(|| format!("keys/{}.pub", common.key_id)),
        runner_id: common.runner_id.clone(),
    };
    Ok((probe, receipts))
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Deploy {
            common,
            project,
            task,
            backend,
            repo,
            commit,
            tag,
            context,
            sbom_dir,
            registry,
            cosign_key,
        } => setup(&common).and_then(|(probe, out)| {
            let release = Release {
                project,
                task,
                backend,
                repo,
                commit,
                tag,
                registry,
                cosign_key,
            };
            deploy(&Docker, &release, &context, &sbom_dir, &probe, &out)
        }),
        Command::Rollback { common, project } => {
            setup(&common).and_then(|(probe, out)| rollback(&Docker, &project, &probe, &out))
        }
    };
    match result {
        Ok(done) => {
            println!(
                "{}",
                json!({"releaseId": done.release_id, "image": done.image, "passed": done.passed, "restored": done.restored})
            );
            if done.passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(message) => {
            eprintln!("{}", json!({"error": message}));
            ExitCode::from(2)
        }
    }
}
