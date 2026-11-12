use std::io::{Read, Write};
use std::net::TcpListener;

use hagency_gitfab_events::journal::{Journal, Signer};
use hagency_gitfab_events::sign::Key;

use super::*;

fn task(phase: &str, status: &str, n: u32, refs: Value) -> Value {
    json!({
        "schema_version": schema::TASK, "event_id": format!("run-1.{n}.0"), "timestamp": format!("2026-11-13T01:0{n}:00Z"),
        "project_id": "DEMO-02", "task_id": "task-1", "run_id": "run-1", "matrix_room_id": "!r:example.org",
        "matrix_thread_event_id": "$root", "git_backend": "github", "repo_ref": "https://github.com/o/demo-02",
        "baseline_sha": "a".repeat(40), "spec_ref": "spec/DEMO-02.md@v1", "phase": phase, "status": status,
        "actor_id": "@agent:example.org", "input_digest": format!("sha256:{}", "1".repeat(64)),
        "output_digest": null, "artifact_refs": refs, "request_id": "req-1", "signature": null, "round": 1
    })
}

fn serve_once_per_request(body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut raw = Vec::new();
            let mut buf = [0u8; 256];
            while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => raw.extend_from_slice(&buf[..n]),
                }
            }
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        }
    });
    port
}

#[test]
fn the_board_shows_runs_tokens_deployments_and_flags_tampering() {
    let dir = tempfile::tempdir().unwrap();
    let key = Key::generate("gitfab-1").unwrap();
    let mut keys = Keyring::default();
    keys.insert("gitfab-1", &key.public_base64()).unwrap();
    let signer = Signer { key: &key, public_key_ref: "keys/gitfab-1.pub" };
    let journal = dir.path().join("events.jsonl");
    let mut j = Journal::open(&journal).unwrap();
    j.append(&task("intake", "succeeded", 1, json!([])), Some(&signer), "human").unwrap();
    j.append(&task("execute", "succeeded", 2, json!(["https://github.com/o/demo-02/pull/7"])), Some(&signer), "agent").unwrap();
    j.append(&json!({"schema_version": schema::MEMORY, "event_id": "m1", "timestamp": "2026-11-13T01:03:00Z",
        "project": "DEMO-02", "module": "api", "spec": "spec/DEMO-02.md@v1", "contributor": "@agent:example.org",
        "action": "execute", "input_digest": format!("sha256:{}", "1".repeat(64)), "output_digest": format!("sha256:{}", "2".repeat(64)),
        "token_delta": 7, "task_id": "task-1", "signature": null}), Some(&signer), "agent").unwrap();
    // A line changed after signing must show up as such, not disappear.
    let mut text = std::fs::read_to_string(&journal).unwrap();
    let changed = task("review", "succeeded", 4, json!([]));
    let mut forged = key.sign(&changed, "keys/gitfab-1.pub").unwrap();
    forged["status"] = json!("failed");
    text.push_str(&format!("{forged}\n"));
    std::fs::write(&journal, text).unwrap();

    let receipts = dir.path().join("receipts.jsonl");
    let mut r = Journal::open(&receipts).unwrap();
    r.append(&json!({"schema_version": schema::RELEASE, "release_id": "DEMO-02-v1", "project_id": "DEMO-02", "task_id": "task-1",
        "git_backend": "github", "repo_ref": "https://github.com/o/demo-02", "commit_sha": "c".repeat(40), "tag": "v1",
        "artifact_digest": format!("sha256:{}", "3".repeat(64)), "image_ref": "gitfab/demo-02:v1", "image_digest": format!("sha256:{}", "3".repeat(64)),
        "sbom_ref": "sbom/DEMO-02-v1.spdx.json", "sbom_digest": format!("sha256:{}", "4".repeat(64)), "deployed_at": "2026-11-13T01:05:00Z",
        "runner_id": "runner-1", "healthcheck": {"result": "passed"}, "rollback": {"result": "no_previous_release"}, "signature": null}),
        Some(&signer), "system").unwrap();

    let port = serve_once_per_request("{\"version\":\"v1\"}");
    let board = snapshot(&Sources {
        journals: vec![journal],
        receipts: vec![receipts],
        keys,
        probes: vec![Probe { project: "DEMO-02".into(), port, health_path: "/healthz".into(), version_path: "/version".into() }],
        server: None,
    });

    let run = &board["runs"][0];
    assert_eq!(run["run"], "run-1");
    assert_eq!(run["current"], json!({"phase": "review", "status": "failed"}));
    assert_eq!(run["steps"][1]["links"][0], "https://github.com/o/demo-02/pull/7");
    assert_eq!(run["steps"][1]["check"], "verified");
    assert_eq!(run["steps"][2]["check"], "bad_signature", "the forged review is flagged");
    assert_eq!(board["counts"]["bad_signature"], 1);
    // intake, execute and the receipt are key events with identity records; the memory event is not.
    assert_eq!(board["counts"]["identity"], 3);
    assert_eq!(board["tokens"]["ok"], true, "{}", board["tokens"]);
    assert!(board["tokens"]["summary"].to_string().contains('7'));
    let dep = &board["deployments"][0];
    assert_eq!(dep["receipts"][0]["health"], "passed");
    assert_eq!(dep["receipts"][0]["check"], "verified");
    assert_eq!(dep["live"]["health"], 200);
    assert!(dep["live"]["version"].as_str().unwrap().contains("v1"));
    // Only public material: no signature bytes, no key seed.
    let text = board.to_string();
    assert!(!text.contains("signature_base64") && !text.contains(&key.seed_base64()));
}
