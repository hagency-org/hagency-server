use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use hagency_gitfab_events::sign::Keyring;

use super::*;

/// Images are the tag they serve; an image built from a context named `broken` never gets healthy.
#[derive(Clone, Default)]
struct Fake {
    images: Arc<Mutex<HashMap<String, (String, bool)>>>,
    running: Arc<Mutex<Option<String>>>,
}

impl Engine for Fake {
    fn build(&self, context: &Path, tag: &str, _labels: &[(&str, &str)]) -> Result<String, String> {
        let served = tag.rsplit(':').next().unwrap().to_owned();
        let id = sha256(tag.as_bytes());
        let healthy = !context.ends_with("broken");
        self.images.lock().unwrap().insert(id.clone(), (served, healthy));
        Ok(id)
    }
    fn sbom(&self, image: &str) -> Result<Vec<u8>, String> {
        Ok(format!("{{\"spdxVersion\":\"SPDX-2.3\",\"name\":\"{image}\"}}").into_bytes())
    }
    fn replace(&self, _name: &str, image: &str, _host: u16, _container: u16) -> Result<(), String> {
        *self.running.lock().unwrap() = Some(image.to_owned());
        Ok(())
    }
    fn running(&self, _name: &str) -> bool {
        self.running.lock().unwrap().is_some()
    }
    fn has_image(&self, image: &str) -> bool {
        self.images.lock().unwrap().contains_key(image)
    }
    fn base_images(&self, _context: &Path) -> Result<Vec<Value>, String> {
        Ok(vec![json!({"name": "busybox:1.36", "digest": format!("sha256:{}", "b".repeat(64))})])
    }
    fn push(&self, tag: &str, remote: &str) -> Result<String, String> {
        let repository = remote.rsplit_once(':').unwrap().0;
        Ok(format!("{repository}@{}", sha256(format!("manifest of {tag}").as_bytes())))
    }
}

/// Answers /healthz and /version as the image the fake engine is running.
fn serve(fake: &Fake) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let fake = fake.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut raw = Vec::new();
            let mut buf = [0u8; 256];
            while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => raw.extend_from_slice(&buf[..n]),
                }
            }
            let request = String::from_utf8_lossy(&raw);
            let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
            let current = fake.running.lock().unwrap().clone();
            let image = current.and_then(|i| fake.images.lock().unwrap().get(&i).cloned());
            let (status, body) = match (path.as_str(), image) {
                ("/healthz", Some((_, true))) => (200, "ok".to_owned()),
                ("/version", Some((tag, true))) => (200, format!("{{\"version\":\"{tag}\"}}")),
                _ => (503, String::new()),
            };
            let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        }
    });
    port
}

fn setup() -> (tempfile::TempDir, Fake, Probe, Receipts, Keyring) {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let port = serve(&fake);
    let probe = Probe {
        port,
        container_port: 8080,
        health_path: "/healthz".into(),
        version_path: "/version".into(),
        wait: Duration::from_millis(900),
    };
    let key = Key::generate("runner-1").unwrap();
    let mut ring = Keyring::default();
    ring.insert("runner-1", &key.public_base64()).unwrap();
    let out = Receipts {
        journal: dir.path().join("receipts.jsonl"),
        key,
        public_key_ref: "keys/runner-1.pub".into(),
        runner_id: "runner-test".into(),
    };
    (dir, fake, probe, out, ring)
}

fn release(tag: &str) -> Release {
    Release {
        project: "DEMO-01".into(),
        task: "task-1".into(),
        backend: "gitcode".into(),
        repo: "https://gitcode.com/example/demo-01".into(),
        commit: "c".repeat(40),
        tag: tag.into(),
        registry: None,
    }
}

fn signed_receipts(out: &Receipts, ring: &Keyring) -> Vec<Value> {
    let all = receipts(&out.journal, "DEMO-01").unwrap();
    for r in &all {
        schema::validate(r).unwrap();
        ring.verify(r).unwrap();
    }
    all
}

#[test]
fn a_good_release_is_recorded_and_a_broken_one_is_restored_from_the_previous() {
    let (dir, fake, probe, out, ring) = setup();
    let sbom = dir.path().join("sbom");
    let ok = deploy(&fake, &release("v1"), &dir.path().join("app"), &sbom, &probe, &out).unwrap();
    assert!(ok.passed && ok.restored.is_none());
    assert!(sbom.join("DEMO-01-v1.spdx.json").exists());

    let bad = deploy(&fake, &release("v2"), &dir.path().join("broken"), &sbom, &probe, &out).unwrap();
    assert!(!bad.passed);
    assert_eq!(bad.restored.as_deref(), Some("DEMO-01-v1"));
    // The previous Release serves again.
    assert!(http_get(probe.port, "/version", Duration::from_secs(1)).unwrap().1.contains("v1"));

    let all = signed_receipts(&out, &ring);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0]["healthcheck"]["result"], "passed");
    assert_eq!(all[0]["rollback"]["result"], "no_previous_release");
    assert_eq!(all[1]["healthcheck"]["result"], "failed");
    assert_eq!(all[1]["rollback"]["result"], "restored");
    assert_eq!(all[1]["rollback"]["target_release_id"], "DEMO-01-v1");
    assert_eq!(all[0]["sbom_digest"], sha256(&std::fs::read(sbom.join("DEMO-01-v1.spdx.json")).unwrap()));
    // Each receipt has its identity event in the journal.
    let lines = std::fs::read_to_string(&out.journal).unwrap();
    assert_eq!(lines.lines().filter(|l| l.contains("gitfab.identity.v1")).count(), 2);
}

#[test]
fn rollback_returns_to_the_good_release_before_the_current_one() {
    let (dir, fake, probe, out, ring) = setup();
    let sbom = dir.path().join("sbom");
    assert!(rollback(&fake, "DEMO-01", &probe, &out).is_err(), "nothing to roll back to yet");
    deploy(&fake, &release("v1"), &dir.path().join("app"), &sbom, &probe, &out).unwrap();
    deploy(&fake, &release("v2"), &dir.path().join("app"), &sbom, &probe, &out).unwrap();
    assert!(http_get(probe.port, "/version", Duration::from_secs(1)).unwrap().1.contains("v2"));

    let back = rollback(&fake, "DEMO-01", &probe, &out).unwrap();
    assert!(back.passed);
    assert!(http_get(probe.port, "/version", Duration::from_secs(1)).unwrap().1.contains("v1"));
    let all = signed_receipts(&out, &ring);
    let last = all.last().unwrap();
    assert_eq!(last["tag"], "v1");
    assert_eq!(last["rollback"]["from_release_id"], "DEMO-01-v2");
    assert_eq!(last["rollback"]["target_release_id"], "DEMO-01-v1");
    assert_eq!(last["rollback"]["result"], "passed");
}

#[test]
fn a_version_endpoint_naming_another_release_fails_the_check() {
    let (_dir, fake, probe, _out, _ring) = setup();
    let image = fake.build(Path::new("app"), "gitfab/demo-01:v1", &[]).unwrap();
    fake.replace("c", &image, probe.port, 8080).unwrap();
    let r = check(&probe, "v1"); assert!(r.is_ok(), "{r:?}");
    let failed = check(&probe, "v9").unwrap_err();
    assert_eq!(failed["result"], "failed");
}

#[test]
fn from_lines_name_the_base_images_but_not_earlier_stages() {
    let file = "FROM --platform=$BUILDPLATFORM rust:1.90 AS build\nRUN cargo build\nFROM build AS test\nFROM scratch\nfrom gcr.io/distroless/cc:nonroot\nCOPY --from=build /a /a\n";
    assert_eq!(from_lines(file), vec!["rust:1.90", "gcr.io/distroless/cc:nonroot"]);
}

#[test]
fn the_sbom_comes_with_its_digest_and_a_signed_attestation() {
    let (dir, fake, probe, out, ring) = setup();
    let sbom = dir.path().join("sbom");
    deploy(&fake, &release("v1"), &dir.path().join("app"), &sbom, &probe, &out).unwrap();
    let body = std::fs::read(sbom.join("DEMO-01-v1.spdx.json")).unwrap();
    let line = std::fs::read_to_string(sbom.join("DEMO-01-v1.spdx.json.sha256")).unwrap();
    assert_eq!(line, format!("{}  DEMO-01-v1.spdx.json\n", sha256(&body).trim_start_matches("sha256:")));
    let sig: Value = serde_json::from_str(&std::fs::read_to_string(sbom.join("DEMO-01-v1.spdx.json.sig")).unwrap()).unwrap();
    ring.verify(&sig).unwrap();
    assert_eq!(sig["sbom_digest"], sha256(&body));
    assert_eq!(sig["base_images"][0]["name"], "busybox:1.36");
}

#[test]
fn a_release_deployed_again_gets_its_own_receipt_and_sbom() {
    let (dir, fake, probe, out, ring) = setup();
    let sbom = dir.path().join("sbom");
    deploy(&fake, &release("v1"), &dir.path().join("app"), &sbom, &probe, &out).unwrap();
    let first = std::fs::read(sbom.join("DEMO-01-v1.spdx.json")).unwrap();
    let again = deploy(&fake, &release("v1"), &dir.path().join("app"), &sbom, &probe, &out).unwrap();
    assert!(again.passed);
    assert_eq!(again.release_id, "DEMO-01-v1-d2");
    assert_eq!(std::fs::read(sbom.join("DEMO-01-v1.spdx.json")).unwrap(), first, "the first SBOM is kept");
    assert!(sbom.join("DEMO-01-v1-d2.spdx.json.sig").exists());
    let all = signed_receipts(&out, &ring);
    assert_eq!(all.len(), 2);
    assert_eq!(all[1]["sbom_ref"], "sbom/DEMO-01-v1-d2.spdx.json");
}

#[test]
fn a_release_pushed_to_a_registry_is_named_by_its_manifest_digest() {
    let (dir, fake, probe, out, ring) = setup();
    let sbom = dir.path().join("sbom");
    let mut published = release("v1");
    published.registry = Some("127.0.0.1:5000/gitfab/".into());
    let done = deploy(&fake, &published, &dir.path().join("app"), &sbom, &probe, &out).unwrap();
    assert!(done.passed);
    let all = signed_receipts(&out, &ring);
    let image_ref = all[0]["image_ref"].as_str().unwrap();
    assert!(image_ref.starts_with("127.0.0.1:5000/gitfab/demo-01@sha256:"), "{image_ref}");
    // The local image ID stays the digest that rollback and the container check use.
    assert_eq!(all[0]["image_digest"], json!(done.image));

    // Without a registry the local tag is pinned to the image ID, as before.
    deploy(&fake, &release("v2"), &dir.path().join("app"), &sbom, &probe, &out).unwrap();
    let all = signed_receipts(&out, &ring);
    assert_eq!(all[1]["image_ref"], json!(format!("gitfab/demo-01:v2@{}", all[1]["image_digest"].as_str().unwrap())));
}
