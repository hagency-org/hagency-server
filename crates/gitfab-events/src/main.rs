//! `gitfab`: offline tools for GitFab acceptance events. Exit 0 when every check passes, 1 when a check
//! fails, 2 for usage or I/O errors. Reports go to stdout as JSON; nothing secret is printed.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use hagency_gitfab_events::ledger::Ledger;
use hagency_gitfab_events::sign::{Key, Keyring};
use hagency_gitfab_events::{Error, canonical, schema};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "gitfab", about = "Canonicalize, sign, verify and total GitFab acceptance events")]
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
    /// Check every event in JSONL files: schema, then signature where present or required.
    Verify {
        /// Directory of `<key-id>.pub` files.
        #[arg(long)]
        keys: PathBuf,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Recompute token totals from JSONL files; fails on any invalid line, conflict or bad correction.
    Ledger {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
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
            print(&json!({"key_id": key_id, "public_key": key.public_base64(), "public_key_file": public}));
            Ok(true)
        }
        Command::Canonicalize { digest, file } => {
            let event = read_event(file.as_deref())?;
            let out = if digest { canonical::payload_digest(&event) } else { canonical::canonical(&event) };
            println!("{}", out.map_err(|e| e.to_string())?);
            Ok(true)
        }
        Command::Sign { key, key_id, public_key_ref, file } => {
            let event = read_event(file.as_deref())?;
            let key = Key::load(&key, &key_id).map_err(|e| e.to_string())?;
            let reference = public_key_ref.unwrap_or_else(|| format!("keys/{key_id}.pub"));
            let signed = key.sign(&event, &reference).map_err(|e| e.to_string())?;
            schema::validate(&signed).map_err(|e| format!("signed event fails the schema: {e}"))?;
            println!("{}", canonical::canonical(&signed).map_err(|e| e.to_string())?);
            Ok(true)
        }
        Command::Verify { keys, files } => {
            let ring = Keyring::load_dir(&keys).map_err(|e| e.to_string())?;
            let mut failures = Vec::new();
            let mut checked = 0;
            let mut signed = Vec::new();
            for file in &files {
                for (line, raw) in read(file)?.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
                    checked += 1;
                    let at = json!({"file": file, "line": line + 1});
                    match verify_line(&ring, raw) {
                        Ok(Some(digest)) => signed.push(digest),
                        Ok(None) => {}
                        Err(e) => failures.push(merge(at, json!({"code": e.code(), "message": e.to_string()}))),
                    }
                }
            }
            print(&json!({"ok": failures.is_empty(), "events": checked, "signed": signed.len(), "failures": failures}));
            Ok(failures.is_empty())
        }
        Command::Ledger { files } => {
            let mut ledger = Ledger::default();
            let mut errors = Vec::new();
            for file in &files {
                for e in ledger.read(&read(file)?, 1) {
                    errors.push(merge(json!({"file": file}), e.to_json()));
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
    }
}

/// Returns the payload digest of a verified signed event, `None` for a valid unsigned one.
fn verify_line(ring: &Keyring, raw: &str) -> Result<Option<String>, Error> {
    let event: Value = serde_json::from_str(raw).map_err(|e| Error::Json(e.to_string()))?;
    schema::validate(&event)?;
    if event.get("signature").is_none_or(Value::is_null) {
        return Ok(None);
    }
    ring.verify(&event)?;
    Ok(Some(event["signature"]["payload_digest"].as_str().unwrap_or_default().to_owned()))
}

fn read_event(file: Option<&Path>) -> Result<Value, String> {
    let text = match file {
        Some(path) => read(path)?,
        None => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).map_err(|e| e.to_string())?;
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
    let mut file = options.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(format!("{contents}\n").as_bytes()).map_err(|e| e.to_string())
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
