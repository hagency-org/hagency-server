//! The `gitfab` binary end to end: the exit codes and failure codes an acceptance script relies on.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};

fn gitfab(args: &[&str], dir: &Path) -> (i32, Value) {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(env!("CARGO_BIN_EXE_gitfab"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run gitfab");
    let text = String::from_utf8(stdout).unwrap();
    let parsed =
        serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.trim().to_owned()));
    assert!(
        status.code() != Some(2),
        "usage error: {}",
        String::from_utf8_lossy(&stderr)
    );
    (status.code().unwrap(), parsed)
}

fn memory(event_id: &str, delta: i64) -> Value {
    json!({
        "schema_version": "gitfab.memory.v1", "event_id": event_id, "timestamp": "2026-11-12T03:00:00Z",
        "project": "DEMO-05", "module": "ledger", "spec": "spec-v1", "contributor": "@impl:example.org",
        "action": "execute", "input_digest": format!("sha256:{}", "a".repeat(64)),
        "output_digest": format!("sha256:{}", "b".repeat(64)), "token_delta": delta, "task_id": "task-1",
        "signature": null
    })
}

fn sign(dir: &Path, event: &Value) -> Value {
    std::fs::write(dir.join("event.json"), event.to_string()).unwrap();
    let (code, out) = gitfab(
        &[
            "sign",
            "--key",
            "keys/k1.key",
            "--key-id",
            "k1",
            "event.json",
        ],
        dir,
    );
    assert_eq!(code, 0);
    assert!(out.is_object(), "{out}");
    out
}

fn verify(dir: &Path, events: &[Value]) -> (i32, Value) {
    std::fs::write(
        dir.join("events.jsonl"),
        events
            .iter()
            .map(|e| e.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    gitfab(&["verify", "--keys", "keys", "events.jsonl"], dir)
}

#[test]
fn keygen_sign_verify_and_the_three_tamper_cases() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert_eq!(
        gitfab(&["keygen", "--key-id", "k1", "--dir", "keys"], dir).0,
        0
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.join("keys/k1.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let signed = sign(dir, &memory("m1", 5));
    let (code, report) = verify(dir, std::slice::from_ref(&signed));
    assert_eq!((code, report["signed"].as_u64()), (0, Some(1)), "{report}");

    let mut changed = signed.clone();
    changed["token_delta"] = json!(50);
    let (code, report) = verify(dir, &[changed]);
    assert_eq!(
        (code, report["failures"][0]["code"].as_str()),
        (1, Some("payload_changed"))
    );

    let original = std::fs::read_to_string(dir.join("keys/k1.pub")).unwrap();
    assert_eq!(
        gitfab(&["keygen", "--key-id", "k2", "--dir", "other"], dir).0,
        0
    );
    std::fs::copy(dir.join("other/k2.pub"), dir.join("keys/k1.pub")).unwrap();
    let (code, report) = verify(dir, std::slice::from_ref(&signed));
    assert_eq!(
        (code, report["failures"][0]["code"].as_str()),
        (1, Some("bad_signature"))
    );
    std::fs::write(dir.join("keys/k1.pub"), original).unwrap();

    let mut unknown = signed.clone();
    unknown["signature"]["key_id"] = json!("nobody");
    let (code, report) = verify(dir, &[unknown]);
    // The key id is part of the signature object, not the payload, so the digest still matches.
    assert_eq!(
        (code, report["failures"][0]["code"].as_str()),
        (1, Some("unknown_key"))
    );
}

#[test]
fn a_readable_private_key_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert_eq!(
        gitfab(&["keygen", "--key-id", "k1", "--dir", "keys"], dir).0,
        0
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.join("keys/k1.key"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::write(dir.join("event.json"), memory("m1", 1).to_string()).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_gitfab"))
            .args([
                "sign",
                "--key",
                "keys/k1.key",
                "--key-id",
                "k1",
                "event.json",
            ])
            .current_dir(dir)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("0600"));
    }
}

#[test]
fn ledger_totals_duplicates_and_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert_eq!(
        gitfab(&["keygen", "--key-id", "k1", "--dir", "keys"], dir).0,
        0
    );
    let (a, b) = (sign(dir, &memory("m1", 5)), sign(dir, &memory("m2", -2)));
    let lines = |events: &[&Value]| {
        events
            .iter()
            .map(|e| e.to_string() + "\n")
            .collect::<String>()
    };

    std::fs::write(dir.join("ok.jsonl"), lines(&[&a, &a, &b])).unwrap();
    let (code, s) = gitfab(&["ledger", "ok.jsonl"], dir);
    assert_eq!(code, 0, "{s}");
    assert_eq!(
        (s["token_total"].as_i64(), s["duplicates_ignored"].as_u64()),
        (Some(3), Some(1))
    );

    let conflicting = sign(dir, &memory("m1", 6));
    std::fs::write(dir.join("bad.jsonl"), lines(&[&a, &conflicting])).unwrap();
    let (code, s) = gitfab(&["ledger", "bad.jsonl"], dir);
    assert_eq!(
        (
            code,
            s["errors"][0]["code"].as_str(),
            s["errors"][0]["line"].as_u64()
        ),
        (1, Some("conflict"), Some(2))
    );
}

#[test]
fn a_signed_key_event_needs_its_identity_event() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert_eq!(
        gitfab(&["keygen", "--key-id", "k1", "--dir", "keys"], dir).0,
        0
    );
    let created = json!({
        "schema_version": "gitfab.task.v1", "event_id": "evt-1", "timestamp": "2026-11-12T03:00:00Z",
        "project_id": "DEMO-01", "task_id": "task-1", "run_id": "run-1", "matrix_room_id": "!room:example.org",
        "matrix_thread_event_id": "$root", "git_backend": "github", "repo_ref": "https://github.com/org/demo-01",
        "baseline_sha": "a".repeat(40), "spec_ref": "spec/DEMO-01.md@v1", "phase": "intake", "status": "succeeded",
        "actor_id": "@coordinator:example.org", "input_digest": format!("sha256:{}", "a".repeat(64)),
        "output_digest": null, "artifact_refs": [], "request_id": "req-1", "signature": null
    });
    let signed = sign(dir, &created);
    let (code, report) = verify(dir, std::slice::from_ref(&signed));
    assert_eq!(
        (code, report["failures"][0]["code"].as_str()),
        (1, Some("missing_identity"))
    );

    let sig = &signed["signature"];
    let identity = json!({
        "schema_version": "gitfab.identity.v1", "event_id": "id-1", "timestamp": "2026-11-12T03:00:01Z",
        "event_type": "task_created", "project_id": "DEMO-01", "task_id": "task-1",
        "actor_id": "@coordinator:example.org", "actor_type": "agent", "key_id": sig["key_id"],
        "algorithm": "ed25519", "payload_digest": sig["payload_digest"], "signature_base64": sig["signature_base64"]
    });
    let (code, report) = verify(dir, &[signed, identity]);
    assert_eq!(code, 0, "{report}");
}

#[test]
fn the_report_uses_verified_records_and_names_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert_eq!(gitfab(&["keygen", "--key-id", "k1", "--dir", "keys"], dir).0, 0);
    let task = |phase: &str, status: &str, n: u32, refs: Value| json!({
        "schema_version": "gitfab.task.v1", "event_id": format!("run-1.{n}.0"), "timestamp": format!("2026-11-12T03:0{n}:00Z"),
        "project_id": "DEMO-01", "task_id": "task-1", "run_id": "run-1", "matrix_room_id": "!room:example.org",
        "matrix_thread_event_id": "$root", "git_backend": "gitcode", "repo_ref": "https://gitcode.com/org/demo-01",
        "baseline_sha": "a".repeat(40), "spec_ref": "spec/DEMO-01.md@v1", "phase": phase, "status": status,
        "actor_id": "@agent:example.org", "input_digest": format!("sha256:{}", "a".repeat(64)),
        "output_digest": null, "artifact_refs": refs, "request_id": "req-1", "signature": null, "round": 1
    });
    let intake = sign(dir, &task("intake", "succeeded", 1, json!([])));
    let mut released = task("release", "succeeded", 2, json!(["https://gitcode.com/org/demo-01/releases/v1"]));
    released["commit_sha"] = json!("b".repeat(40));
    let release = sign(dir, &released);
    let mut refused = task("execute", "succeeded", 4, json!([]));
    refused["platform_error"] = json!("GitCode answered 403 to POST /repos/org/demo-01/pulls");
    let refused = sign(dir, &refused);
    let mut forged = sign(dir, &task("review", "succeeded", 3, json!([])));
    forged["status"] = json!("failed");
    let lines = [intake, release, forged, refused].iter().map(|e| e.to_string() + "\n").collect::<String>();
    std::fs::write(dir.join("events.jsonl"), lines).unwrap();
    let (code, out) = gitfab(&["report", "--keys", "keys", "--project", "DEMO-01", "events.jsonl"], dir);
    let text = out.as_str().unwrap();
    assert_eq!(code, 0);
    assert!(text.contains("验签通过的签名记录：3 条"), "{text}");
    assert!(text.contains("1 行未通过校验") && text.contains("events.jsonl:3"), "{text}");
    assert!(text.contains("https://gitcode.com/org/demo-01/releases/v1"));
    assert!(text.contains("提交 bbbbbbbbbbbb"), "{text}");
    assert!(text.contains("| 平台未完成 | GitCode answered 403 to POST"), "{text}");
    let (code, out) = gitfab(&["trace", "--keys", "keys", "--id", &"b".repeat(40), "events.jsonl"], dir);
    assert_eq!((code, out["matches"][0]["phase"].as_str()), (0, Some("release")), "{out}");
    assert!(!text.contains("review"), "the forged review is not reported");
}

#[test]
fn trace_follows_a_release_through_task_events_and_ignores_forged_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert_eq!(gitfab(&["keygen", "--key-id", "k1", "--dir", "keys"], dir).0, 0);
    let event = |phase: &str, n: u32, refs: Value| json!({
        "schema_version": "gitfab.task.v1", "event_id": format!("run-9.{n}.0"), "timestamp": "2026-11-12T03:00:00Z",
        "project_id": "DEMO-04", "task_id": "task-9", "run_id": "run-9", "matrix_room_id": "!room:example.org",
        "matrix_thread_event_id": "$root", "git_backend": "github", "repo_ref": "https://github.com/org/demo-04",
        "baseline_sha": "a".repeat(40), "spec_ref": "spec/DEMO-04.md@v1", "phase": phase, "status": "succeeded",
        "actor_id": "@agent:example.org", "input_digest": format!("sha256:{}", "a".repeat(64)),
        "output_digest": null, "artifact_refs": refs, "request_id": "req-9", "signature": null, "round": 1
    });
    let release = sign(dir, &event("release", 2, json!(["https://github.com/org/demo-04/releases/tag/v4"])));
    let mut forged = sign(dir, &event("release", 3, json!([])));
    forged["artifact_refs"] = json!(["https://github.com/org/demo-04/releases/tag/v4"]);
    let lines = [sign(dir, &event("intake", 1, json!([]))), release, forged].iter().map(|e| e.to_string() + "\n").collect::<String>();
    std::fs::write(dir.join("events.jsonl"), lines).unwrap();
    let (code, out) = gitfab(&["trace", "--keys", "keys", "--id", "v4", "events.jsonl"], dir);
    assert_eq!(code, 0);
    assert_eq!(out["matches"].as_array().unwrap().len(), 1, "only the signed Release, not the forged copy: {out}");
    assert_eq!(out["matches"][0]["line"], 2);
    let (code, out) = gitfab(&["trace", "--keys", "keys", "--id", "task-9", "events.jsonl"], dir);
    assert_eq!((code, out["matches"].as_array().unwrap().len()), (0, 2));
    assert_eq!(gitfab(&["trace", "--keys", "keys", "--id", "nothing", "events.jsonl"], dir).0, 1);
}

#[test]
fn trace_by_text_finds_the_signed_spec_and_not_an_edited_copy() {
    use sha2::Digest;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert_eq!(gitfab(&["keygen", "--key-id", "k1", "--dir", "keys"], dir).0, 0);
    let spec = "# Spec v2\n\n验收标准：标题 1 到 120 个字符。\n";
    let digest = format!("sha256:{}", hex::encode(sha2::Sha256::digest(spec.as_bytes())));
    let event = json!({
        "schema_version": "gitfab.task.v1", "event_id": "run-3.2.0", "timestamp": "2026-11-12T03:00:00Z",
        "project_id": "DEMO-02", "task_id": "task-3", "run_id": "run-3", "matrix_room_id": "!room:example.org",
        "matrix_thread_event_id": "$root", "git_backend": "github", "repo_ref": "https://github.com/org/demo-02",
        "baseline_sha": "a".repeat(40), "spec_ref": "spec/DEMO-02.md@v1", "phase": "spec", "status": "succeeded",
        "actor_id": "@agent:example.org", "input_digest": format!("sha256:{}", "a".repeat(64)),
        "output_digest": digest, "artifact_refs": [], "request_id": "req-3", "signature": null, "round": 0
    });
    std::fs::write(dir.join("events.jsonl"), sign(dir, &event).to_string() + "\n").unwrap();
    std::fs::write(dir.join("spec.md"), spec).unwrap();
    std::fs::write(dir.join("edited.md"), spec.replace("120", "200")).unwrap();

    let (code, out) = gitfab(&["trace", "--keys", "keys", "--text", "spec.md", "events.jsonl"], dir);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["id"], digest);
    assert_eq!(out["matches"][0]["phase"], "spec");
    let (code, out) = gitfab(&["trace", "--keys", "keys", "--text", "edited.md", "events.jsonl"], dir);
    assert_eq!((code, out["matches"].as_array().map(Vec::len)), (1, Some(0)), "{out}");

    let (_, report) = gitfab(&["report", "--keys", "keys", "--project", "DEMO-02", "events.jsonl"], dir);
    assert!(report.as_str().unwrap().contains(&format!("输出 {}", &digest[..19])), "{report}");
}

#[test]
fn find_narrows_to_a_run_and_can_take_its_first_event() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert_eq!(
        gitfab(&["keygen", "--key-id", "k1", "--dir", "keys"], dir).0,
        0
    );
    let event = |run: &str, n: u32, blob: char| {
        json!({
            "schema_version": "gitfab.task.v1", "event_id": format!("{run}.{n}.0"), "timestamp": "2026-11-12T03:00:00Z",
            "project_id": "DEMO-02", "task_id": "task-1", "run_id": run, "matrix_room_id": "!room:example.org",
            "matrix_thread_event_id": "$root", "git_backend": "github", "repo_ref": "https://github.com/org/demo-02",
            "baseline_sha": "a".repeat(40), "spec_ref": "spec/DEMO-02.md@v1", "phase": "execute", "status": "succeeded",
            "actor_id": "@agent:example.org", "input_digest": format!("sha256:{}", "a".repeat(64)),
            "output_digest": null, "artifact_refs": [], "request_id": "req", "signature": null, "round": n,
            "spec_revision": 1, "spec_blob": blob.to_string().repeat(40)
        })
    };
    let lines = [
        event("run-1", 1, 'b'),
        event("run-1", 2, 'c'),
        event("run-2", 1, 'd'),
    ]
    .iter()
    .map(|e| sign(dir, e).to_string() + "\n")
    .collect::<String>();
    std::fs::write(dir.join("events.jsonl"), lines).unwrap();
    let find = |extra: &[&str]| {
        let mut args = vec![
            "find",
            "--keys",
            "keys",
            "--project",
            "DEMO-02",
            "--phase",
            "execute",
            "--status",
            "succeeded",
        ];
        args.extend_from_slice(extra);
        args.push("events.jsonl");
        gitfab(&args, dir)
    };
    let (code, out) = find(&[]);
    assert_eq!(
        (code, out["run_id"].as_str()),
        (0, Some("run-2")),
        "the project's last by default"
    );
    let (_, out) = find(&["--run", "run-1"]);
    assert_eq!(out["spec_blob"], "c".repeat(40));
    let (_, out) = find(&["--run", "run-1", "--first"]);
    assert_eq!(
        (out["spec_blob"].as_str(), out["spec_revision"].as_i64()),
        (Some("b".repeat(40).as_str()), Some(1))
    );
    assert_eq!(find(&["--run", "run-3"]).0, 1);
}
