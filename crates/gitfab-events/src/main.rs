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
    /// Write a Markdown "what I did" report of one project from the journals and receipts: each run's
    /// stages in time order with actors and links, deployments, token totals and how many lines
    /// verified. Only signed lines that verify are reported; the rest are counted and named.
    Report {
        #[arg(long)]
        keys: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Every verified signed record that names an identifier: a project, task, run, commit SHA, Release
    /// id or tag, image digest or a link. Exit 1 when nothing matches.
    ///
    /// With `--text FILE`, the identifier is the file's SHA-256: it finds the signed stage whose input or
    /// output was exactly that text, so an edited Spec or Reviewer reply matches nothing.
    Trace {
        #[arg(long)]
        keys: PathBuf,
        /// The identifier to follow.
        #[arg(long, required_unless_present = "text", conflicts_with = "text")]
        id: Option<String>,
        /// A text (a Spec, a Reviewer reply) to find by its digest.
        #[arg(long)]
        text: Option<PathBuf>,
        #[arg(required = true)]
        files: Vec<PathBuf>,
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
    /// Check a bundle against its `manifest.json`: listed files present and unchanged, nothing
    /// unlisted. Exit 1 when anything differs.
    CheckManifest { dir: PathBuf },
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
            for key in ["event_id", "run_id", "task_id", "timestamp", "round", "git_backend", "repo_ref", "commit_sha", "platform_error"] {
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
        Command::Report {
            keys,
            project,
            files,
        } => {
            let ring = Keyring::load_dir(&keys).map_err(|e| e.to_string())?;
            println!("{}", report(&ring, &project, &files)?);
            Ok(true)
        }
        Command::Trace {
            keys,
            id,
            text,
            files,
        } => {
            let ring = Keyring::load_dir(&keys).map_err(|e| e.to_string())?;
            let id = match (id, text) {
                (Some(id), _) => id,
                (None, Some(file)) => {
                    let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", display(&file)))?;
                    format!("sha256:{}", hex::encode(sha2::Sha256::digest(&bytes)))
                }
                (None, None) => unreachable!("clap requires --id or --text"),
            };
            let found = trace(&ring, &id, &files)?;
            let any = !found.is_empty();
            print(&json!({"id": id, "matches": found}));
            Ok(any)
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
        Command::CheckManifest { dir } => {
            let failures = evidence::check_manifest(&dir);
            print(&json!({"ok": failures.is_empty(), "failures": failures}));
            Ok(failures.is_empty())
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

/// Fields an identifier is looked up in, and the fields shown for each match.
const TRACE_FIELDS: &[&str] = &[
    "project_id", "project", "task_id", "run_id", "event_id", "release_id", "tag", "commit_sha",
    "baseline_sha", "image_digest", "artifact_digest", "input_digest", "output_digest",
];

/// Records naming `id` (Annex 2 §5: project, task, commit and Release ids lead to one another).
fn trace(ring: &Keyring, id: &str, files: &[PathBuf]) -> Result<Vec<Value>, String> {
    let mut found = Vec::new();
    for file in files {
        for (n, raw) in read(file)?.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let Ok((event, Some(_))) = verify_line(ring, raw) else { continue };
            let refs: Vec<&str> = event["artifact_refs"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let named = TRACE_FIELDS.iter().any(|f| event[*f] == id) || refs.iter().any(|r| *r == id || r.ends_with(&format!("/{id}")));
            if !named {
                continue;
            }
            let mut hit = json!({"file": display(file), "line": n + 1, "schema": event["schema_version"]});
            for f in TRACE_FIELDS.iter().chain(&["phase", "status", "timestamp", "deployed_at"]) {
                if !event[*f].is_null() {
                    hit[*f] = event[*f].clone();
                }
            }
            if !refs.is_empty() {
                hit["artifact_refs"] = json!(refs);
            }
            found.push(hit);
        }
    }
    Ok(found)
}

/// The "what I did" report (Annex 1 §1.2.1(6)): built only from verified signed records.
fn report(ring: &Keyring, project: &str, files: &[PathBuf]) -> Result<String, String> {
    use std::collections::BTreeMap;
    let (mut verified, mut refused) = (0usize, Vec::new());
    let mut runs: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut receipts = Vec::new();
    let mut ledger = Ledger::default();
    let mut tokens = String::new();
    for file in files {
        for (n, raw) in read(file)?.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let Ok((event, digest)) = verify_line(ring, raw) else {
                refused.push(format!("{}:{}", display(file), n + 1));
                continue;
            };
            let mine = event["project_id"] == project || event["project"] == project;
            if !mine || digest.is_none() {
                continue;
            }
            verified += 1;
            match event["schema_version"].as_str() {
                Some(schema::TASK) => runs.entry(event["run_id"].as_str().unwrap_or("").to_owned()).or_default().push(event),
                Some(schema::RELEASE) => receipts.push(event),
                Some(schema::MEMORY | schema::TOKEN) => {
                    tokens.push_str(&event.to_string());
                    tokens.push('\n');
                }
                _ => {}
            }
        }
    }
    let mut out = format!("# {project} 工作报告\n\n只采用验签通过的签名记录：{verified} 条");
    if !refused.is_empty() {
        out += &format!("；另有 {} 行未通过校验，未采用（{}）", refused.len(), refused.join("、"));
    }
    out += "。\n";
    for (run, mut events) in runs {
        events.sort_by(|a, b| a["timestamp"].as_str().cmp(&b["timestamp"].as_str()));
        let first = &events[0];
        let last = &events[events.len() - 1];
        out += &format!(
            "\n## 运行 {run}\n\n任务 {}，仓库 {}（{}，基线 {}），Spec {}。当前：{} {}。\n\n| 时间 | 阶段 | 状态 | 轮次 | 执行者 | 链接 |\n|---|---|---|---|---|---|\n",
            first["task_id"].as_str().unwrap_or(""), first["repo_ref"].as_str().unwrap_or(""),
            first["git_backend"].as_str().unwrap_or(""), first["baseline_sha"].as_str().unwrap_or(""),
            first["spec_ref"].as_str().unwrap_or(""), last["phase"].as_str().unwrap_or(""), last["status"].as_str().unwrap_or(""),
        );
        for e in &events {
            let mut links: Vec<String> = e["artifact_refs"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
            if let Some(sha) = e["commit_sha"].as_str() {
                links.push(format!("提交 {}", &sha[..sha.len().min(12)]));
            }
            // Each Spec version and Reviewer reply is named by its digest; `trace --text` finds it again.
            if matches!(e["phase"].as_str(), Some("spec" | "review"))
                && let Some(digest) = e["output_digest"].as_str()
            {
                links.push(format!("输出 {}", &digest[..digest.len().min(19)]));
            }
            out += &format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                e["timestamp"].as_str().unwrap_or(""), e["phase"].as_str().unwrap_or(""), e["status"].as_str().unwrap_or(""),
                e["round"].as_i64().map_or(String::new(), |r| r.to_string()), e["actor_id"].as_str().unwrap_or(""),
                links.join(" "),
            );
            if let Some(error) = e["platform_error"].as_str() {
                out += &format!("|  | 平台未完成 | {} |  |  |  |\n", error.replace('|', "/").replace('\n', " "));
            }
            // A long Reviewer reply keeps its verdict line as a second entry; the last one counts.
            let findings = e["findings"].as_array().into_iter().flatten().filter_map(Value::as_str);
            if let Some(line) = findings.flat_map(str::lines).map(str::trim).rfind(|l| l.starts_with("VERDICT:")) {
                out += &format!("|  | 评审结论 | {line} |  |  |  |\n");
            }
        }
    }
    if !receipts.is_empty() {
        out += "\n## 部署\n\n| 时间 | 回执 | 提交 | 镜像 | 健康检查 | 回退 | Runner |\n|---|---|---|---|---|---|---|\n";
        for r in &receipts {
            out += &format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                r["deployed_at"].as_str().unwrap_or(""), r["release_id"].as_str().unwrap_or(""),
                r["commit_sha"].as_str().unwrap_or("").chars().take(12).collect::<String>(),
                r["image_digest"].as_str().unwrap_or("").chars().take(19).collect::<String>(),
                r["healthcheck"]["result"].as_str().unwrap_or(""), r["rollback"]["result"].as_str().unwrap_or(""),
                r["runner_id"].as_str().unwrap_or(""),
            );
        }
    }
    if !tokens.is_empty() {
        let errors = ledger.read(&tokens, 1);
        out += "\n## Token\n\n";
        out += &match (errors.is_empty(), ledger.summary()) {
            (true, Ok(summary)) => format!("合计 {}，去重后 {} 条，忽略重复 {} 条，更正 {} 条（账本摘要 {}）。\n",
                summary["token_total"], summary["unique_events"], summary["duplicates_ignored"], summary["corrections"],
                summary["ledger_digest"].as_str().unwrap_or("")),
            _ => "账本复算未通过，请运行 gitfab ledger 查看原因。\n".to_owned(),
        };
    }
    Ok(out)
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
