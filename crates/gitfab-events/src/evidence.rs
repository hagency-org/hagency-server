//! The evidence bundle (Annex 3 §11): events split by kind for one project, and `manifest.json` with each
//! file's relative path, size and SHA-256. A bundle holding a credential, a private key or a local absolute
//! path gets no manifest.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Error, canonical, schema};

/// Bundle file for each event kind.
fn file_for(version: &str) -> &'static str {
    match version {
        schema::TASK => "task-events.jsonl",
        schema::IDENTITY => "identity-events.jsonl",
        schema::MEMORY => "memory-events.jsonl",
        schema::TOKEN => "token-events.jsonl",
        _ => "release-events.jsonl",
    }
}

fn project_of(event: &Value) -> Option<&str> {
    event
        .get("project_id")
        .or_else(|| event.get("project"))
        .and_then(Value::as_str)
}

/// Splits valid events of `project` into their bundle files under `out`. Every line must pass the schema;
/// the same event key twice must carry the same content. Returns the number of events per file.
pub fn export(
    project: &str,
    inputs: &[(String, String)],
    out: &Path,
) -> Result<BTreeMap<&'static str, usize>, Vec<Value>> {
    let mut failures = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut files: BTreeMap<&'static str, String> = BTreeMap::new();
    for name in [
        "task-events.jsonl",
        "identity-events.jsonl",
        "memory-events.jsonl",
        "token-events.jsonl",
    ] {
        files.insert(name, String::new());
    }
    let mut counts: BTreeMap<&'static str, usize> = files.keys().map(|k| (*k, 0)).collect();
    for (name, text) in inputs {
        for (n, raw) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let at = json!({"file": name, "line": n + 1});
            let checked = serde_json::from_str::<Value>(raw)
                .map_err(|e| Error::Json(e.to_string()))
                .and_then(|event| {
                    let version = schema::validate(&event)?;
                    let content = canonical::canonical(&event)?;
                    Ok((event, version, content))
                });
            let (event, version, content) = match checked {
                Ok(ok) => ok,
                Err(e) => {
                    failures.push(merge(
                        at,
                        json!({"code": e.code(), "message": e.to_string()}),
                    ));
                    continue;
                }
            };
            if project_of(&event) != Some(project) {
                continue;
            }
            let key = schema::event_key(&event).unwrap_or_default().to_owned();
            match seen.get(&key) {
                Some(prior) if *prior == content => continue,
                Some(_) => {
                    let e = Error::Conflict(key);
                    failures.push(merge(
                        at,
                        json!({"code": e.code(), "message": e.to_string()}),
                    ));
                    continue;
                }
                None => {
                    seen.insert(key, content.clone());
                }
            }
            let file = file_for(version);
            let lines = files.entry(file).or_default();
            lines.push_str(&content);
            lines.push('\n');
            *counts.entry(file).or_default() += 1;
        }
    }
    if !failures.is_empty() {
        return Err(failures);
    }
    std::fs::create_dir_all(out)
        .map_err(|e| vec![json!({"code": "io", "message": e.to_string()})])?;
    for (name, text) in &files {
        std::fs::write(out.join(name), text)
            .map_err(|e| vec![json!({"code": "io", "message": e.to_string()})])?;
    }
    Ok(counts)
}

/// What may not appear anywhere in a bundle (Annex 3 §11), as (code, needle).
const FORBIDDEN: &[(&str, &str)] = &[
    ("private_key", "PRIVATE KEY-----"),
    ("github_token", "ghp_"),
    ("github_token", "github_pat_"),
    ("github_token", "gho_"),
    ("github_token", "ghs_"),
    ("matrix_token", "syt_"),
    ("matrix_token", "mct_"),
    ("api_key", "sk-ant-"),
    ("api_key", "sk-proj-"),
    ("bearer_credential", "Bearer ey"),
    ("absolute_path", "/Users/"),
    ("absolute_path", "/home/"),
    ("absolute_path", "/root/"),
    ("absolute_path", "/private/var/"),
    ("absolute_path", "C:\\Users\\"),
];

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            // A link could point outside the bundle; it is refused rather than followed.
            out.push(path);
        } else if kind.is_dir() {
            walk(root, &path, out)?;
        } else if path.strip_prefix(root).ok() != Some(Path::new("manifest.json")) {
            out.push(path);
        }
    }
    Ok(())
}

/// Writes `<dir>/manifest.json` unless a file holds forbidden material. Returns the manifest, or what was
/// found where (never the matched text itself).
pub fn manifest(dir: &Path, generated_at: &str) -> Result<Value, Vec<Value>> {
    let io = |e: std::io::Error| vec![json!({"code": "io", "message": e.to_string()})];
    let mut paths = Vec::new();
    walk(dir, dir, &mut paths).map_err(io)?;
    paths.sort();
    let (mut entries, mut failures) = (Vec::new(), Vec::new());
    for path in paths {
        let relative = path
            .strip_prefix(dir)
            .expect("walked under dir")
            .to_string_lossy()
            .replace('\\', "/");
        if std::fs::symlink_metadata(&path)
            .map_err(io)?
            .file_type()
            .is_symlink()
        {
            failures.push(json!({"file": relative, "code": "symlink"}));
            continue;
        }
        let bytes = std::fs::read(&path).map_err(io)?;
        let text = String::from_utf8_lossy(&bytes);
        for (code, needle) in FORBIDDEN {
            if text.contains(needle) {
                failures.push(json!({"file": relative, "code": code}));
            }
        }
        entries.push(json!({
            "path": relative,
            "size": bytes.len(),
            "sha256": hex::encode(Sha256::digest(&bytes)),
        }));
    }
    if !failures.is_empty() {
        failures.dedup();
        return Err(failures);
    }
    let run = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let manifest = json!({
        "acceptance_run_id": run,
        "generated_at": generated_at,
        "algorithm": "sha256",
        "files": entries,
    });
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).expect("JSON") + "\n",
    )
    .map_err(io)?;
    Ok(manifest)
}

/// Checks a bundle against its `manifest.json`: every listed file present with its size and SHA-256,
/// and no file the manifest does not list. Returns what differs (empty when the bundle is intact).
pub fn check_manifest(dir: &Path) -> Vec<Value> {
    let manifest: Value = match std::fs::read(dir.join("manifest.json")) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(m) => m,
            Err(_) => return vec![json!({"file": "manifest.json", "code": "invalid_manifest"})],
        },
        Err(_) => return vec![json!({"file": "manifest.json", "code": "missing_manifest"})],
    };
    let mut failures = Vec::new();
    let mut listed = std::collections::BTreeSet::new();
    for entry in manifest["files"].as_array().into_iter().flatten() {
        let Some(relative) = entry["path"].as_str() else {
            failures.push(json!({"file": "manifest.json", "code": "invalid_manifest"}));
            continue;
        };
        listed.insert(relative.to_owned());
        if relative
            .split('/')
            .any(|part| part == ".." || part.is_empty())
        {
            failures.push(json!({"file": relative, "code": "invalid_path"}));
            continue;
        }
        match std::fs::read(dir.join(relative)) {
            Ok(bytes) => {
                let digest = hex::encode(Sha256::digest(&bytes));
                if Some(bytes.len() as u64) != entry["size"].as_u64()
                    || Some(digest.as_str()) != entry["sha256"].as_str()
                {
                    failures.push(json!({"file": relative, "code": "changed"}));
                }
            }
            Err(_) => failures.push(json!({"file": relative, "code": "missing"})),
        }
    }
    let mut paths = Vec::new();
    if walk(dir, dir, &mut paths).is_ok() {
        for path in paths {
            let relative = path
                .strip_prefix(dir)
                .expect("walked under dir")
                .to_string_lossy()
                .replace('\\', "/");
            if !listed.contains(&relative) {
                failures.push(json!({"file": relative, "code": "unlisted"}));
            }
        }
    }
    failures
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{memory, task};
    use crate::sign::Key;

    #[test]
    fn a_bundle_is_checked_against_its_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("ar-1");
        std::fs::create_dir_all(bundle.join("sub")).unwrap();
        std::fs::write(bundle.join("a-report.json"), "{}").unwrap();
        std::fs::write(bundle.join("sub/b.jsonl"), "x\n").unwrap();
        assert_eq!(
            check_manifest(&bundle),
            vec![json!({"file": "manifest.json", "code": "missing_manifest"})]
        );
        manifest(&bundle, "2026-11-13T00:00:00Z").unwrap();
        assert!(check_manifest(&bundle).is_empty());

        std::fs::write(bundle.join("sub/b.jsonl"), "y\n").unwrap();
        std::fs::write(bundle.join("late.json"), "{}").unwrap();
        std::fs::remove_file(bundle.join("a-report.json")).unwrap();
        let mut found = check_manifest(&bundle);
        found.sort_by_key(|f| f["file"].as_str().unwrap().to_owned());
        assert_eq!(
            found,
            vec![
                json!({"file": "a-report.json", "code": "missing"}),
                json!({"file": "late.json", "code": "unlisted"}),
                json!({"file": "sub/b.jsonl", "code": "changed"}),
            ]
        );
    }

    fn signed_memory(key: &Key, id: &str, project: &str) -> String {
        let mut e = memory(id, 5);
        e["project"] = json!(project);
        e["signature"] = Value::Null;
        canonical::canonical(&key.sign(&e, "keys/gitfab-1.pub").unwrap()).unwrap()
    }

    #[test]
    fn export_keeps_one_project_and_splits_by_kind() {
        let key = Key::generate("gitfab-1").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let running = canonical::canonical(&task("plan", "running")).unwrap();
        let mine = signed_memory(&key, "m1", "DEMO-05");
        let other = signed_memory(&key, "m2", "DEMO-01");
        let text = format!("{running}\n{mine}\n{other}\n{mine}\n");
        let counts = export("DEMO-05", &[("events.jsonl".into(), text)], dir.path()).unwrap();
        assert_eq!(
            counts["memory-events.jsonl"], 1,
            "same content twice is one event"
        );
        assert_eq!(
            counts["task-events.jsonl"], 0,
            "the task fixture is DEMO-01"
        );
        let written = std::fs::read_to_string(dir.path().join("memory-events.jsonl")).unwrap();
        assert_eq!(written, format!("{mine}\n"));
        assert!(dir.path().join("token-events.jsonl").exists());
    }

    #[test]
    fn export_refuses_invalid_lines_and_conflicts_and_writes_nothing() {
        let key = Key::generate("gitfab-1").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("bundle");
        let mine = signed_memory(&key, "m1", "DEMO-05");
        let mut changed: Value = serde_json::from_str(&mine).unwrap();
        changed["token_delta"] = json!(9);
        let text = format!("{mine}\n{changed}\nnot json\n");
        let failures = export("DEMO-05", &[("e.jsonl".into(), text)], &out).unwrap_err();
        let codes: Vec<&str> = failures
            .iter()
            .map(|f| f["code"].as_str().unwrap())
            .collect();
        assert_eq!(codes, ["conflict", "invalid_json"]);
        assert!(!out.exists());
    }

    #[test]
    fn manifest_lists_every_file_with_its_digest() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("ar-1");
        std::fs::create_dir_all(bundle.join("ui")).unwrap();
        std::fs::write(bundle.join("summary.md"), "ok\n").unwrap();
        std::fs::write(bundle.join("ui/step-1.json"), "{}").unwrap();
        let m = manifest(&bundle, "2026-11-12T00:00:00Z").unwrap();
        assert_eq!(m["acceptance_run_id"], "ar-1");
        let files = m["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["path"], "summary.md");
        assert_eq!(files[0]["size"], 3);
        assert_eq!(
            files[0]["sha256"],
            "dc51b8c96c2d745df3bd5590d990230a482fd247123599548e0632fdbf97fc22"
        );
        assert_eq!(files[1]["path"], "ui/step-1.json");
        // Writing again does not list the manifest itself.
        let again = manifest(&bundle, "2026-11-12T00:00:00Z").unwrap();
        assert_eq!(again, m);
    }

    #[test]
    fn a_bundle_with_a_secret_or_local_path_gets_no_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("ar-2");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("deployment.log"), "pushed with ghp_abcdef\n").unwrap();
        std::fs::write(bundle.join("summary.md"), "built in /home/runner/work\n").unwrap();
        let failures = manifest(&bundle, "2026-11-12T00:00:00Z").unwrap_err();
        assert_eq!(
            failures,
            vec![
                json!({"file": "deployment.log", "code": "github_token"}),
                json!({"file": "summary.md", "code": "absolute_path"}),
            ]
        );
        assert!(!bundle.join("manifest.json").exists());
        assert!(
            !serde_json::to_string(&failures).unwrap().contains("abcdef"),
            "the secret is not repeated"
        );
    }
}
