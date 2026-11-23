//! GitFab monitoring board (Annex 1 §1.2.1(7), Annex 3 §6.2): a read-only projection of the signed
//! records. Task progress, the signed event stream, token totals and deployments come only from the
//! event journals and the Runner's receipts, each line verified against the pinned keys; deployments are
//! also probed live. The board has no write path: it never changes a journal, a ledger or a run.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use hagency_gitfab_events::ledger::Ledger;
use hagency_gitfab_events::schema;
use hagency_gitfab_events::sign::Keyring;
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

/// A deployment to probe: project, port on 127.0.0.1, health and version paths.
#[derive(Clone)]
pub struct Probe {
    pub project: String,
    pub port: u16,
    pub health_path: String,
    pub version_path: String,
}

pub struct Sources {
    pub journals: Vec<PathBuf>,
    pub receipts: Vec<PathBuf>,
    pub keys: Keyring,
    pub probes: Vec<Probe>,
    /// The server's Agent status (`/hagency/gitfab/v1/agents`) and the board token file (mode 0600).
    pub server: Option<(String, PathBuf)>,
}

/// Agent status from the server. The token reaches curl on standard input, never on its command line.
pub fn server_agents(server: &str, token_file: &std::path::Path) -> Value {
    use std::io::Write as _;
    let token = match std::fs::read_to_string(token_file) {
        Ok(t) if !t.trim().is_empty() => t.trim().to_owned(),
        _ => return json!({"ok": false, "error": "board token file unreadable"}),
    };
    let url = format!("{}/hagency/gitfab/v1/agents", server.trim_end_matches('/'));
    let child = std::process::Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "8",
            "-K",
            "-",
            "-w",
            "\\n%{http_code}",
            &url,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return json!({"ok": false, "error": "curl unavailable"});
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "header = \"Authorization: Bearer {token}\"");
    }
    let Ok(out) = child.wait_with_output() else {
        return json!({"ok": false, "error": "server unreachable"});
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, status) = text.rsplit_once('\n').unwrap_or(("", "000"));
    match (status.trim(), serde_json::from_str::<Value>(body)) {
        ("200", Ok(v)) => json!({"ok": true, "status": v}),
        (code, _) => json!({"ok": false, "error": format!("server answered HTTP {code}")}),
    }
}

/// How a line checked out: verified signature, unsigned (allowed by its schema), or refused.
fn verdict(keys: &Keyring, event: &Value) -> &'static str {
    if schema::validate(event).is_err() {
        return "invalid";
    }
    // An identity event carries the signature of the event it attests; `gitfab verify` cross-checks it.
    if event["schema_version"] == schema::IDENTITY {
        return "identity";
    }
    if event.get("signature").is_none_or(Value::is_null) {
        return "unsigned";
    }
    if keys.verify(event).is_ok() {
        "verified"
    } else {
        "bad_signature"
    }
}

fn lines(files: &[PathBuf]) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for file in files {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for raw in text.lines().filter(|l| !l.trim().is_empty()) {
            let event = serde_json::from_str(raw).unwrap_or(Value::Null);
            out.push((name.clone(), event));
        }
    }
    out
}

fn short(digest: &Value) -> String {
    digest
        .as_str()
        .map(|d| d.trim_start_matches("sha256:").chars().take(12).collect())
        .unwrap_or_default()
}

/// One stream entry. Only public fields: identifiers, phase, status, key id and digest prefix.
fn entry(source: &str, event: &Value, check: &str) -> Value {
    let sig = &event["signature"];
    json!({
        "source": source,
        "schema": event["schema_version"],
        "id": event.get("event_id").or_else(|| event.get("release_id")).cloned().unwrap_or(Value::Null),
        "project": event["project_id"].as_str().or(event["project"].as_str()),
        "task": event["task_id"],
        "kind": event.get("phase").map(|p| format!("{} {}", p.as_str().unwrap_or(""), event["status"].as_str().unwrap_or("")))
            .or_else(|| event.get("event_type").and_then(Value::as_str).map(str::to_owned))
            .or_else(|| event.get("tag").and_then(Value::as_str).map(|t| format!("deploy {t}")))
            .or_else(|| event.get("action").and_then(Value::as_str).map(str::to_owned)),
        "time": event.get("timestamp").or_else(|| event.get("deployed_at")).cloned().unwrap_or(Value::Null),
        "actor": event.get("actor_id").or_else(|| event.get("runner_id")).or_else(|| event.get("contributor")).cloned().unwrap_or(Value::Null),
        "key_id": sig["key_id"],
        "digest": short(&sig["payload_digest"]),
        "check": check,
    })
}

/// The whole board as JSON, recomputed from the sources on every call.
pub fn snapshot(sources: &Sources) -> Value {
    let mut stream = Vec::new();
    let mut runs: BTreeMap<String, Value> = BTreeMap::new();
    let mut ledger = Ledger::default();
    let mut ledger_lines = String::new();
    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    for (source, event) in lines(&sources.journals) {
        let check = verdict(&sources.keys, &event);
        *counts.entry(check).or_default() += 1;
        stream.push(entry(&source, &event, check));
        match event["schema_version"].as_str() {
            Some(schema::TASK) => {
                let run_id = event["run_id"].as_str().unwrap_or("").to_owned();
                let run = runs.entry(run_id.clone()).or_insert_with(|| {
                    json!({"run": run_id, "project": event["project_id"], "task": event["task_id"],
                        "backend": event["git_backend"], "repo": event["repo_ref"], "steps": []})
                });
                run["steps"].as_array_mut().expect("array").push(json!({
                    "phase": event["phase"], "status": event["status"], "round": event["round"],
                    "time": event["timestamp"], "actor": event["actor_id"], "check": check,
                    "links": event["artifact_refs"],
                    "commit": event["commit_sha"], "platform_error": event["platform_error"],
                }));
                run["current"] = json!({"phase": event["phase"], "status": event["status"]});
            }
            Some(schema::MEMORY | schema::TOKEN) => {
                ledger_lines.push_str(&event.to_string());
                ledger_lines.push('\n');
            }
            _ => {}
        }
    }
    let tokens = {
        let errors = ledger.read(&ledger_lines, 1);
        if errors.is_empty() {
            match ledger.summary() {
                Ok(summary) => json!({"ok": true, "summary": summary}),
                Err(errors) => {
                    json!({"ok": false, "errors": errors.iter().map(|e| e.to_json()).collect::<Vec<_>>()})
                }
            }
        } else {
            json!({"ok": false, "errors": errors.iter().map(|e| e.to_json()).collect::<Vec<_>>()})
        }
    };
    let mut deployments: BTreeMap<String, Value> = BTreeMap::new();
    for (source, event) in lines(&sources.receipts) {
        let check = verdict(&sources.keys, &event);
        *counts.entry(check).or_default() += 1;
        stream.push(entry(&source, &event, check));
        if event["schema_version"] != schema::RELEASE {
            continue;
        }
        let project = event["project_id"].as_str().unwrap_or("").to_owned();
        let history = deployments
            .entry(project.clone())
            .or_insert_with(|| json!({"project": project, "receipts": []}));
        history["receipts"].as_array_mut().expect("array").push(json!({
            "release": event["release_id"], "tag": event["tag"], "commit": event["commit_sha"],
            "image": event["image_digest"], "sbom": event["sbom_ref"], "time": event["deployed_at"],
            "runner": event["runner_id"], "health": event["healthcheck"]["result"],
            "rollback": event["rollback"]["result"], "check": check,
        }));
    }
    for probe in &sources.probes {
        let live = probe_live(probe);
        deployments
            .entry(probe.project.clone())
            .or_insert_with(|| json!({"project": probe.project, "receipts": []}))["live"] = live;
    }
    stream.reverse();
    stream.truncate(200);
    let agents = match &sources.server {
        Some((server, token)) => server_agents(server, token),
        None => json!({"ok": false, "error": "not configured"}),
    };
    json!({
        "agents": agents,
        "generated_at": now(),
        "counts": counts,
        "runs": runs.into_values().collect::<Vec<_>>(),
        "events": stream,
        "tokens": tokens,
        "deployments": deployments.into_values().collect::<Vec<_>>(),
    })
}

fn probe_live(probe: &Probe) -> Value {
    let get =
        |path: &str| hagency_gitfab_runner::http_get(probe.port, path, Duration::from_secs(2));
    let health = get(&probe.health_path);
    let version = get(&probe.version_path);
    json!({
        "health": match &health { Ok((s, _)) => json!(s), Err(_) => json!("unreachable") },
        "version": match &version { Ok((200, body)) => json!(body.trim().chars().take(120).collect::<String>()), Ok((s, _)) => json!(format!("HTTP {s}")), Err(_) => json!("unreachable") },
    })
}

fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}
