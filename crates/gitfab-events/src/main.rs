//! `gitfab`: offline tools for GitFab acceptance events. Exit 0 when every check passes, 1 when a check
//! fails, 2 for usage or I/O errors. Reports go to stdout as JSON; nothing secret is printed.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use hagency_gitfab_events::ledger::Ledger;
use hagency_gitfab_events::sign::{Key, Keyring};
use hagency_gitfab_events::{Error, canonical, evidence, identity, schema};
use serde_json::{Value, json};
use sha2::Digest;

#[derive(Parser)]
#[command(
    name = "gitfab",
    about = "Canonicalize, sign, verify and total GitFab acceptance events"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create `<dir>/<key-id>.key` (mode 0600) and `<dir>/<key-id>.pub`.
    Keygen {
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        dir: PathBuf,
    },
    /// Print the canonical encoding of one event (or its payload digest with --digest).
    Canonicalize {
        #[arg(long)]
        digest: bool,
        file: Option<PathBuf>,
    },
    /// Sign one event and print it canonically, with its signature object.
    Sign {
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        key_id: String,
        /// Reference recorded in the event; defaults to `keys/<key-id>.pub`.
        #[arg(long)]
        public_key_ref: Option<String>,
        file: Option<PathBuf>,
    },
    /// Check every event in JSONL files: schema, signature where present or required, and that each signed
    /// key event has a matching identity event.
    Verify {
        /// Directory of `<key-id>.pub` files.
        #[arg(long)]
        keys: PathBuf,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Print the last signed, verified task event of a project in a given phase and status, with its
    /// artifact links flattened (`artifact_ref_0`, …). Exit 1 when there is none.
    Find {
        #[arg(long)]
        keys: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long)]
        phase: String,
        #[arg(long)]
        status: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Print the last signed, verified deployment receipt (`gitfab.release.v1`) of a project, for one tag or
    /// any, flattened (`healthcheck_result`, `rollback_result`, …). Exit 1 when there is none.
    Receipt {
        #[arg(long)]
        keys: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long)]
        tag: Option<String>,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Check an SBOM against its signed attestation (`<sbom>.sig`, from the Runner) and its `.sha256` file
    /// when present. Offline; exit 1 when anything does not match.
    VerifySbom {
        #[arg(long)]
        keys: PathBuf,
        sbom: PathBuf,
    },
    /// Recompute token totals from JSONL files; fails on any invalid line, conflict or bad correction.
    Ledger {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Write one project's events into an evidence bundle, split by kind (task, identity, memory, token).
    Export {
        #[arg(long)]
        project: String,
        #[arg(long)]
        out: PathBuf,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Write `<dir>/manifest.json` (path, size, SHA-256 of every file); refuses a bundle holding a
    /// credential, a private key or a local absolute path.
    Manifest { dir: PathBuf },
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("gitfab: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(command: Command) -> Result<bool, String> {
    match command {
        Command::Keygen { key_id, dir } => {
            let key = Key::generate(&key_id).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let private = dir.join(format!("{key_id}.key"));
            write_private(&private, &key.seed_base64())?;
            let public = dir.join(format!("{key_id}.pub"));
            std::fs::write(&public, key.public_base64() + "\n").map_err(|e| e.to_string())?;
            print(
                &json!({"key_id": key_id, "public_key": key.public_base64(), "public_key_file": public}),
            );
            Ok(true)
        }
        Command::Canonicalize { digest, file } => {
            let event = read_event(file.as_deref())?;
            let out = if digest {
                canonical::payload_digest(&event)
            } else {
                canonical::canonical(&event)
            };
            println!("{}", out.map_err(|e| e.to_string())?);
            Ok(true)
        }
        Command::Sign {
            key,
            key_id,
            public_key_ref,
            file,
        } => {
            let event = read_event(file.as_deref())?;
            let key = Key::load(&key, &key_id).map_err(|e| e.to_string())?;
            let reference = public_key_ref.unwrap_or_else(|| format!("keys/{key_id}.pub"));
            let signed = key.sign(&event, &reference).map_err(|e| e.to_string())?;
            schema::validate(&signed).map_err(|e| format!("signed event fails the schema: {e}"))?;
            println!(
                "{}",
                canonical::canonical(&signed).map_err(|e| e.to_string())?
            );
            Ok(true)
        }
        Command::Verify { keys, files } => {
            let ring = Keyring::load_dir(&keys).map_err(|e| e.to_string())?;
            let mut failures = Vec::new();
            let mut checked = 0;
            let mut signed = Vec::new();
            // Events that passed their own checks, for the identity cross-check.
            let (mut valid, mut places) = (Vec::new(), Vec::new());
            for file in &files {
                for (line, raw) in read(file)?
                    .lines()
                    .enumerate()
                    .filter(|(_, l)| !l.trim().is_empty())
                {
                    checked += 1;
                    let at = json!({"file": display(file), "line": line + 1});
                    match verify_line(&ring, raw) {
                        Ok((event, digest)) => {
                            signed.extend(digest);
                            valid.push(event);
                            places.push(at);
                        }
                        Err(e) => failures.push(merge(
                            at,
                            json!({"code": e.code(), "message": e.to_string()}),
                        )),
                    }
                }
            }
            for (i, e) in identity::check(&valid) {
                failures.push(merge(
                    places[i].clone(),
                    json!({"code": e.code(), "message": e.to_string()}),
                ));
            }
            print(
                &json!({"ok": failures.is_empty(), "events": checked, "signed": signed.len(), "failures": failures}),
            );
            Ok(failures.is_empty())
        }
        Command::Find {
            keys,
            project,
            phase,
            status,
            files,
        } => {
            let ring = Keyring::load_dir(&keys).map_err(|e| e.to_string())?;
            let mut found = None;
            for file in &files {
                for raw in read(file)?.lines().filter(|l| !l.trim().is_empty()) {
                    // Only a signed event that verifies can answer; anything else is skipped here and
                    // reported by `verify`.
                    if let Ok((event, Some(_))) = verify_line(&ring, raw)
                        && event["schema_version"] == schema::TASK
                        && event["project_id"] == project.as_str()
                        && event["phase"] == phase.as_str()
                        && event["status"] == status.as_str()
                    {
                        found = Some(event);
                    }
                }
            }
            let Some(event) = found else {
                print(&json!({"found": false, "project": project, "phase": phase, "status": status}));
                return Ok(false);
            };
            let mut flat = json!({"found": true});
            for key in ["event_id", "run_id", "task_id", "timestamp", "round", "git_backend", "repo_ref"] {
                flat[key] = event[key].clone();
            }
            for (i, link) in event["artifact_refs"].as_array().into_iter().flatten().enumerate() {
                flat[format!("artifact_ref_{i}")] = link.clone();
            }
            print(&flat);
            Ok(true)
        }
        Command::Receipt {
            keys,
            project,
            tag,
            files,
        } => {
            let ring = Keyring::load_dir(&keys).map_err(|e| e.to_string())?;
            let mut found = None;
            for file in &files {
                for raw in read(file)?.lines().filter(|l| !l.trim().is_empty()) {
                    if let Ok((event, Some(_))) = verify_line(&ring, raw)
                        && event["schema_version"] == schema::RELEASE
                        && event["project_id"] == project.as_str()
                        && tag.as_deref().is_none_or(|t| event["tag"] == t)
                    {
                        found = Some(event);
                    }
                }
            }
            let Some(event) = found else {
                print(&json!({"found": false, "project": project, "tag": tag}));
                return Ok(false);
            };
            let mut flat = json!({"found": true});
            for key in [
                "release_id", "tag", "commit_sha", "image_ref", "image_digest", "sbom_ref", "sbom_digest",
                "deployed_at", "runner_id",
            ] {
                flat[key] = event[key].clone();
            }
            flat["healthcheck_result"] = event["healthcheck"]["result"].clone();
            flat["rollback_result"] = event["rollback"]["result"].clone();
            flat["rollback_target"] = event["rollback"]["target_release_id"].clone();
            flat["key_id"] = event["signature"]["key_id"].clone();
            print(&flat);
            Ok(true)
        }
        Command::VerifySbom { keys, sbom } => {
            let ring = Keyring::load_dir(&keys).map_err(|e| e.to_string())?;
            let body = std::fs::read(&sbom).map_err(|e| format!("{}: {e}", sbom.display()))?;
            let digest = format!("sha256:{}", hex::encode(sha2::Sha256::digest(&body)));
            let with = |ext: &str| PathBuf::from(format!("{}.{ext}", sbom.display()));
            let mut failures = Vec::new();
            let attestation: Value = serde_json::from_str(&read(&with("sig"))?)
                .map_err(|e| format!("invalid attestation: {e}"))?;
            if attestation["schema_version"] != "gitfab.sbom-attestation.v1" {
                failures.push("not a gitfab.sbom-attestation.v1 document".to_owned());
            }
            if let Err(e) = ring.verify(&attestation) {
                failures.push(format!("attestation signature: {e}"));
            }
            if attestation["sbom_digest"] != digest.as_str() {
                failures.push(format!("SBOM is {digest}, attested {}", attestation["sbom_digest"]));
            }
            if let Ok(line) = std::fs::read_to_string(with("sha256"))
                && line.split_whitespace().next() != digest.strip_prefix("sha256:")
            {
                failures.push("the .sha256 file does not match the SBOM".to_owned());
            }
            print(&json!({"ok": failures.is_empty(), "sbom_digest": digest,
                "image_digest": attestation["image_digest"], "base_images": attestation["base_images"],
                "key_id": attestation["signature"]["key_id"], "failures": failures}));
            Ok(failures.is_empty())
        }
        Command::Ledger { files } => {
            let mut ledger = Ledger::default();
            let mut errors = Vec::new();
            for file in &files {
                for e in ledger.read(&read(file)?, 1) {
                    errors.push(merge(json!({"file": display(file)}), e.to_json()));
                }
            }
            if errors.is_empty() {
                match ledger.summary() {
                    Ok(summary) => {
                        print(&merge(json!({"ok": true}), summary));
                        return Ok(true);
                    }
                    Err(e) => errors.extend(e.iter().map(|e| e.to_json())),
                }
            }
            print(&json!({"ok": false, "errors": errors}));
            Ok(false)
        }
        Command::Export {
            project,
            out,
            files,
        } => {
            let mut inputs = Vec::new();
            for file in &files {
                inputs.push((display(file), read(file)?));
            }
            match evidence::export(&project, &inputs, &out) {
                Ok(counts) => {
                    print(&json!({"ok": true, "project": project, "files": counts}));
                    Ok(true)
                }
                Err(failures) => {
                    print(&json!({"ok": false, "project": project, "failures": failures}));
                    Ok(false)
                }
            }
        }
        Command::Manifest { dir } => {
            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            match evidence::manifest(&dir, &now) {
                Ok(manifest) => {
                    let files = manifest["files"].as_array().map_or(0, Vec::len);
                    print(&json!({"ok": true, "acceptance_run_id": manifest["acceptance_run_id"], "files": files}));
                    Ok(true)
                }
                Err(failures) => {
                    print(&json!({"ok": false, "failures": failures}));
                    Ok(false)
                }
            }
        }
    }
}

/// A file name for reports: never the local absolute path.
fn display(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Returns the event and, when it is signed and verified, its payload digest.
fn verify_line(ring: &Keyring, raw: &str) -> Result<(Value, Option<String>), Error> {
    let event: Value = serde_json::from_str(raw).map_err(|e| Error::Json(e.to_string()))?;
    schema::validate(&event)?;
    if event.get("signature").is_none_or(Value::is_null) {
        return Ok((event, None));
    }
    ring.verify(&event)?;
    let digest = event["signature"]["payload_digest"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    Ok((event, Some(digest)))
}

fn read_event(file: Option<&Path>) -> Result<Value, String> {
    let text = match file {
        Some(path) => read(path)?,
        None => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| e.to_string())?;
            s
        }
    };
    serde_json::from_str(&text).map_err(|e| format!("invalid JSON: {e}"))
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_private(path: &Path, contents: &str) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(format!("{contents}\n").as_bytes())
        .map_err(|e| e.to_string())
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

fn print(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).expect("JSON"));
}
