//! GitFab Runner (Annex 3 §9): deploys a Release as a container, checks it and records a signed
//! deployment receipt (`gitfab.release.v1`); restores the previous Release when the new one does not
//! come up, and rolls back on request.
//!
//! Receipts are signed with the Runner's own key, separate from the Agent event key. The container
//! engine is behind [`Engine`] so the flow is tested without Docker; health and version probes are real
//! HTTP requests to the published port.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hagency_gitfab_events::journal::{Journal, Signer};
use hagency_gitfab_events::schema;
use hagency_gitfab_events::sign::Key;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

/// What the Runner needs from a container engine.
pub trait Engine {
    /// Builds `context` as `tag`, passing `args` as build arguments; returns the immutable image ID
    /// (`sha256:…`).
    fn build(
        &self,
        context: &Path,
        tag: &str,
        labels: &[(&str, &str)],
        args: &[(&str, &str)],
    ) -> Result<String, String>;
    /// SPDX JSON of an image.
    fn sbom(&self, image: &str) -> Result<Vec<u8>, String>;
    /// Replaces the container `name` with one running `image`, publishing `host_port` to
    /// `container_port` on 127.0.0.1.
    fn replace(
        &self,
        name: &str,
        image: &str,
        host_port: u16,
        container_port: u16,
    ) -> Result<(), String>;
    fn running(&self, name: &str) -> bool;
    /// Whether the image is still present locally.
    fn has_image(&self, image: &str) -> bool;
    /// Base images of the build, `{"name", "digest"}` each (Annex 3 §9: recorded, never drifting).
    fn base_images(&self, context: &Path) -> Result<Vec<Value>, String>;
    /// Pushes local image `tag` as `remote` (`registry/repository:tag`); returns the pushed
    /// reference pinned to its manifest digest, `registry/repository@sha256:…`.
    fn push(&self, tag: &str, remote: &str) -> Result<String, String>;
    /// Signs a pushed image (`registry/repository@sha256:…`) with a cosign key; the signature is
    /// stored in the registry next to the image.
    fn cosign_sign(&self, image_ref: &str, key: &Path) -> Result<(), String>;
}

/// The cosign image the Runner and verify-release use (`GITFAB_COSIGN_IMAGE` overrides it).
pub fn cosign_image() -> String {
    std::env::var("GITFAB_COSIGN_IMAGE")
        .unwrap_or_else(|_| "gcr.io/projectsigstore/cosign:v2.4.1".into())
}

/// A registry on this machine is reached over plain HTTP; any other must use HTTPS.
fn local_registry(image_ref: &str) -> bool {
    let host = image_ref.split('/').next().unwrap_or_default();
    let name = host.rsplit_once(':').map_or(host, |(h, _)| h);
    matches!(name, "127.0.0.1" | "localhost")
}

/// Image references in the `FROM` lines of a Dockerfile, skipping earlier stages and `scratch`.
pub fn from_lines(dockerfile: &str) -> Vec<String> {
    let mut stages = Vec::new();
    let mut bases = Vec::new();
    for line in dockerfile.lines().map(str::trim) {
        let mut words = line.split_whitespace();
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("FROM")) {
            continue;
        }
        let words: Vec<&str> = words.filter(|w| !w.starts_with("--")).collect();
        let Some(image) = words.first() else { continue };
        if let [_, kw, alias] = words[..]
            && kw.eq_ignore_ascii_case("AS")
        {
            stages.push(alias.to_ascii_lowercase());
        }
        let lower = image.to_ascii_lowercase();
        if lower != "scratch"
            && !stages[..stages.len().saturating_sub(1)].contains(&lower)
            && !bases.contains(&image.to_string())
        {
            bases.push(image.to_string());
        }
    }
    bases
}

/// Docker through its command line.
pub struct Docker;

fn docker(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("docker")
        .args(args)
        .output()
        .map_err(|e| format!("docker: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "docker {}: {}",
            args[0],
            err.lines().last().unwrap_or("failed")
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

impl Engine for Docker {
    fn build(
        &self,
        context: &Path,
        tag: &str,
        labels: &[(&str, &str)],
        build_args: &[(&str, &str)],
    ) -> Result<String, String> {
        let context = context.to_str().ok_or("context path is not UTF-8")?;
        let mut args = vec!["build", "-q", "-t", tag];
        let pairs: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
        for pair in &pairs {
            args.extend(["--label", pair]);
        }
        let values: Vec<String> = build_args.iter().map(|(k, v)| format!("{k}={v}")).collect();
        for value in &values {
            args.extend(["--build-arg", value]);
        }
        args.push(context);
        docker(&args)?;
        docker(&["image", "inspect", "-f", "{{.Id}}", tag])
    }
    fn sbom(&self, image: &str) -> Result<Vec<u8>, String> {
        let syft =
            std::env::var("GITFAB_SYFT_IMAGE").unwrap_or_else(|_| "anchore/syft:v1.18.1".into());
        let source = format!("docker:{image}");
        docker(&[
            "run",
            "--rm",
            "-v",
            "/var/run/docker.sock:/var/run/docker.sock",
            &syft,
            &source,
            "-o",
            "spdx-json",
        ])
        .map(String::into_bytes)
    }
    fn replace(
        &self,
        name: &str,
        image: &str,
        host_port: u16,
        container_port: u16,
    ) -> Result<(), String> {
        let _ = docker(&["rm", "-f", name]);
        let publish = format!("127.0.0.1:{host_port}:{container_port}");
        docker(&["run", "-d", "--name", name, "-p", &publish, image]).map(|_| ())
    }
    fn running(&self, name: &str) -> bool {
        docker(&["inspect", "-f", "{{.State.Running}}", name]).is_ok_and(|s| s == "true")
    }
    fn has_image(&self, image: &str) -> bool {
        docker(&["image", "inspect", image]).is_ok()
    }
    fn cosign_sign(&self, image_ref: &str, key: &Path) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(key)
            .map_err(|e| format!("cosign key: {e}"))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err("cosign key: must be mode 0600".into());
        }
        let dir = key
            .parent()
            .and_then(Path::to_str)
            .ok_or("cosign key: no directory")?;
        let file = key
            .file_name()
            .and_then(|f| f.to_str())
            .ok_or("cosign key: no file name")?;
        let mount = format!("{dir}:/k:ro");
        let key_arg = format!("/k/{file}");
        let image = cosign_image();
        // The key password comes from COSIGN_PASSWORD in the Runner's environment, never an argument.
        let mut args = vec![
            "run",
            "--rm",
            "--network",
            "host",
            "-e",
            "COSIGN_PASSWORD",
            "-v",
            &mount,
        ];
        let config = std::env::var("HOME")
            .map(|h| format!("{h}/.docker/config.json"))
            .unwrap_or_default();
        let auth = format!("{config}:/root/.docker/config.json:ro");
        if Path::new(&config).is_file() {
            args.extend(["-v", &auth]);
        }
        args.extend([
            image.as_str(),
            "sign",
            "--yes",
            "--key",
            &key_arg,
            "--tlog-upload=false",
        ]);
        if local_registry(image_ref) {
            args.push("--allow-insecure-registry");
        }
        args.push(image_ref);
        docker(&args).map(|_| ())
    }
    fn push(&self, tag: &str, remote: &str) -> Result<String, String> {
        docker(&["tag", tag, remote])?;
        docker(&["push", "-q", remote])?;
        let repository = remote.rsplit_once(':').map_or(remote, |(r, _)| r);
        let digests = docker(&[
            "image",
            "inspect",
            "-f",
            "{{range .RepoDigests}}{{println .}}{{end}}",
            remote,
        ])?;
        digests
            .lines()
            .map(str::trim)
            .find(|d| d.split_once('@').is_some_and(|(r, _)| r == repository))
            .map(str::to_owned)
            .ok_or_else(|| format!("{remote}: the registry returned no manifest digest"))
    }
    fn base_images(&self, context: &Path) -> Result<Vec<Value>, String> {
        let file = std::fs::read_to_string(context.join("Dockerfile"))
            .map_err(|e| format!("Dockerfile: {e}"))?;
        from_lines(&file)
            .into_iter()
            .map(|name| {
                let repo =
                    docker(&["image", "inspect", "-f", "{{index .RepoDigests 0}}", &name]).ok();
                let digest = repo
                    .as_deref()
                    .and_then(|r| r.split_once('@'))
                    .map(|(_, d)| d.to_owned());
                Ok(json!({"name": name, "digest": digest}))
            })
            .collect()
    }
}

/// One HTTP GET on 127.0.0.1; returns status and body.
pub fn http_get(port: u16, path: &str, timeout: Duration) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), timeout)
        .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("no HTTP status")?;
    Ok((status, body.to_owned()))
}

pub struct Probe {
    pub port: u16,
    pub container_port: u16,
    pub health_path: String,
    pub version_path: String,
    pub wait: Duration,
}

/// Polls the health endpoint until 200 or `wait` runs out, then reads the version endpoint, which must
/// name `tag`. Returns the healthcheck object for the receipt.
pub fn check(probe: &Probe, tag: &str) -> Result<Value, Value> {
    let started = Instant::now();
    let mut last;
    loop {
        match http_get(probe.port, &probe.health_path, Duration::from_secs(2)) {
            Ok((200, _)) => break,
            Ok((status, _)) => last = format!("HTTP {status}"),
            Err(e) => last = e,
        }
        if started.elapsed() >= probe.wait {
            return Err(
                json!({"command": format!("GET {}", probe.health_path), "exit_code": 1,
                "result": "failed", "detail": last}),
            );
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let version = http_get(probe.port, &probe.version_path, Duration::from_secs(2));
    match version {
        Ok((200, body)) if body.contains(tag) => Ok(json!({
            "command": format!("GET {} then GET {}", probe.health_path, probe.version_path),
            "exit_code": 0, "result": "passed", "version": tag,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        })),
        Ok((status, body)) => Err(
            json!({"command": format!("GET {}", probe.version_path), "exit_code": 1,
            "result": "failed", "detail": format!("HTTP {status}, version is not {tag}: {}", body.trim().chars().take(120).collect::<String>())}),
        ),
        Err(e) => Err(
            json!({"command": format!("GET {}", probe.version_path), "exit_code": 1,
            "result": "failed", "detail": e}),
        ),
    }
}

/// Where receipts are written and how they are signed.
pub struct Receipts {
    pub journal: PathBuf,
    pub key: Key,
    pub public_key_ref: String,
    pub runner_id: String,
}

impl Receipts {
    fn append(&self, record: &Value) -> Result<(), String> {
        let mut journal = Journal::open(&self.journal).map_err(|e| e.to_string())?;
        let signer = Signer {
            key: &self.key,
            public_key_ref: &self.public_key_ref,
        };
        journal
            .append(record, Some(&signer), "system")
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    /// Receipts of a project in order, newest last.
    pub fn of(&self, project: &str) -> Result<Vec<Value>, String> {
        receipts(&self.journal, project)
    }
}

pub fn receipts(journal: &Path, project: &str) -> Result<Vec<Value>, String> {
    if !journal.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(journal).map_err(|e| e.to_string())?;
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["schema_version"] == schema::RELEASE && e["project_id"] == project)
        .collect())
}

/// The last receipt of a project whose deployment passed.
pub fn last_good(receipts: &[Value]) -> Option<&Value> {
    receipts
        .iter()
        .rev()
        .find(|r| r["healthcheck"]["result"] == "passed")
}

pub struct Release {
    pub project: String,
    pub task: String,
    pub backend: String,
    pub repo: String,
    pub commit: String,
    pub tag: String,
    /// Where the image is published (`host[:port]/namespace`); its manifest digest then names it.
    pub registry: Option<String>,
    /// A cosign key (mode 0600) to sign the published image with; needs `registry`.
    pub cosign_key: Option<std::path::PathBuf>,
}

fn container(project: &str) -> String {
    format!("gitfab-{}", project.to_ascii_lowercase())
}

fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Schema of the signed SBOM attestation (`<sbom>.sig`).
pub const SBOM_ATTESTATION: &str = "gitfab.sbom-attestation.v1";

/// Writes the SBOM with its companions (Annex 3 §9): `<name>.sha256` in `sha256sum` format and
/// `<name>.sig`, a document naming the SBOM digest, the image and its base images, signed with the
/// Runner key. `gitfab verify-sbom` checks both offline.
fn write_sbom(
    dir: &Path,
    name: &str,
    sbom: &[u8],
    image: &str,
    bases: &[Value],
    release: &Release,
    out: &Receipts,
) -> Result<(), String> {
    let digest = sha256(sbom);
    std::fs::write(dir.join(name), sbom).map_err(|e| e.to_string())?;
    let hex = digest.trim_start_matches("sha256:");
    std::fs::write(
        dir.join(format!("{name}.sha256")),
        format!("{hex}  {name}\n"),
    )
    .map_err(|e| e.to_string())?;
    let attestation = json!({
        "schema_version": SBOM_ATTESTATION,
        "project_id": release.project,
        "tag": release.tag,
        "commit_sha": release.commit,
        "sbom_ref": format!("sbom/{name}"),
        "sbom_digest": digest,
        "image_digest": image,
        "base_images": bases,
        "runner_id": out.runner_id,
        "signature": null,
    });
    let signed = out
        .key
        .sign(&attestation, &out.public_key_ref)
        .map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{name}.sig")), format!("{signed}\n"))
        .map_err(|e| e.to_string())
}

/// Outcome of a deployment, for the command's report.
#[derive(Debug)]
pub struct Deployed {
    pub release_id: String,
    pub image: String,
    pub passed: bool,
    pub restored: Option<String>,
}

/// Builds, documents, starts and checks `release`. On failure the previous good Release is restored and
/// both receipts are written.
pub fn deploy(
    engine: &dyn Engine,
    release: &Release,
    context: &Path,
    sbom_dir: &Path,
    probe: &Probe,
    out: &Receipts,
) -> Result<Deployed, String> {
    let history = out.of(&release.project)?;
    let previous = last_good(&history).cloned();
    let image_tag = format!(
        "gitfab/{}:{}",
        release.project.to_ascii_lowercase(),
        release.tag
    );
    // GITFAB_RELEASE and GITFAB_COMMIT let the Dockerfile put the Release into the image (an `ARG`
    // for each); a Dockerfile that declares neither builds the same.
    let image = engine.build(
        context,
        &image_tag,
        &[
            ("org.hagency.gitfab.project", &release.project),
            ("org.hagency.gitfab.commit", &release.commit),
            ("org.hagency.gitfab.tag", &release.tag),
        ],
        &[
            ("GITFAB_RELEASE", &release.tag),
            ("GITFAB_COMMIT", &release.commit),
        ],
    )?;
    // A Release deployed again (after a restart, say) gets its own receipt and SBOM files; nothing
    // recorded earlier is overwritten.
    let first = format!("{}-{}", release.project, release.tag);
    let release_id = if history.iter().any(|r| r["release_id"] == first.as_str()) {
        format!("{first}-d{}", history.len() + 1)
    } else {
        first
    };
    // Published before it runs: the receipt names the image by its registry manifest digest.
    let image_ref = match &release.registry {
        Some(registry) => engine.push(
            &image_tag,
            &format!(
                "{}/{}",
                registry.trim_end_matches('/'),
                image_tag.trim_start_matches("gitfab/")
            ),
        )?,
        None => format!("{image_tag}@{image}"),
    };
    // Signed before it runs: a signing failure leaves the running service untouched.
    if let Some(key) = &release.cosign_key {
        if release.registry.is_none() {
            return Err("--cosign-key needs --registry: cosign signs a published image".into());
        }
        engine.cosign_sign(&image_ref, key)?;
    }
    let sbom = engine.sbom(&image)?;
    let bases = engine.base_images(context)?;
    std::fs::create_dir_all(sbom_dir).map_err(|e| e.to_string())?;
    let sbom_name = format!("{release_id}.spdx.json");
    write_sbom(sbom_dir, &sbom_name, &sbom, &image, &bases, release, out)?;
    let name = container(&release.project);
    // A replacement that fails part-way may already have removed the old container: it is a failed
    // deployment like a failed health check, restored and recorded, never an early return.
    let result = match engine.replace(&name, &image, probe.port, probe.container_port) {
        Ok(()) => check(probe, &release.tag),
        Err(e) => Err(json!({"command": format!("start container {name}"), "exit_code": 1,
            "result": "failed", "detail": e})),
    };
    let restored = match &result {
        Ok(_) => None,
        Err(_) => previous.as_ref().and_then(|p| {
            let image = p["image_digest"].as_str()?.to_owned();
            engine
                .replace(&name, &image, probe.port, probe.container_port)
                .ok()?;
            check(probe, p["tag"].as_str()?).ok()?;
            p["release_id"].as_str().map(str::to_owned)
        }),
    };
    let rollback = match (&result, &previous) {
        (Ok(_), Some(p)) => {
            json!({"target_release_id": p["release_id"], "command": format!("gitfab-runner rollback --project {}", release.project), "exit_code": null, "result": "available"})
        }
        (Ok(_), None) => {
            json!({"target_release_id": null, "command": null, "exit_code": null, "result": "no_previous_release"})
        }
        (Err(_), _) => {
            json!({"target_release_id": previous.as_ref().map(|p| p["release_id"].clone()), "command": "automatic restore",
            "exit_code": if restored.is_some() { 0 } else { 1 }, "result": if restored.is_some() { "restored" } else { "not_restored" }})
        }
    };
    let healthcheck = match &result {
        Ok(h) | Err(h) => h.clone(),
    };
    let record = json!({
        "schema_version": schema::RELEASE,
        "release_id": release_id,
        "project_id": release.project,
        "task_id": release.task,
        "git_backend": release.backend,
        "repo_ref": release.repo,
        "commit_sha": release.commit,
        "tag": release.tag,
        "artifact_digest": image,
        "image_ref": image_ref,
        "image_digest": image,
        "sbom_ref": format!("sbom/{sbom_name}"),
        "sbom_digest": sha256(&sbom),
        "deployed_at": now(),
        "runner_id": out.runner_id,
        "healthcheck": healthcheck,
        "rollback": rollback,
        "signature": null,
    });
    out.append(&record)?;
    Ok(Deployed {
        release_id,
        image,
        passed: result.is_ok(),
        restored,
    })
}

/// Redeploys the good Release before the current one and records it.
pub fn rollback(
    engine: &dyn Engine,
    project: &str,
    probe: &Probe,
    out: &Receipts,
) -> Result<Deployed, String> {
    let history = out.of(project)?;
    let good: Vec<&Value> = history
        .iter()
        .filter(|r| r["healthcheck"]["result"] == "passed")
        .collect();
    let (Some(current), Some(target)) = (good.last(), good.iter().rev().nth(1)) else {
        return Err(format!(
            "{project} has no earlier good Release to roll back to"
        ));
    };
    let image = target["image_digest"]
        .as_str()
        .ok_or("receipt without image")?
        .to_owned();
    if !engine.has_image(&image) {
        return Err(format!(
            "image {image} of {} is no longer present",
            target["release_id"]
        ));
    }
    let tag = target["tag"].as_str().ok_or("receipt without tag")?;
    engine.replace(
        &container(project),
        &image,
        probe.port,
        probe.container_port,
    )?;
    let result = check(probe, tag);
    let n = history.len();
    let release_id = format!(
        "{}-rollback-{n}",
        target["release_id"].as_str().unwrap_or(project)
    );
    let mut record = (*target).clone();
    record["release_id"] = json!(release_id);
    record["deployed_at"] = json!(now());
    record["runner_id"] = json!(out.runner_id);
    record["signature"] = Value::Null;
    record["healthcheck"] = match &result {
        Ok(h) | Err(h) => h.clone(),
    };
    record["rollback"] = json!({"from_release_id": current["release_id"], "target_release_id": target["release_id"],
        "command": format!("gitfab-runner rollback --project {project}"),
        "exit_code": if result.is_ok() { 0 } else { 1 }, "result": if result.is_ok() { "passed" } else { "failed" }});
    out.append(&record)?;
    Ok(Deployed {
        release_id,
        image,
        passed: result.is_ok(),
        restored: None,
    })
}
