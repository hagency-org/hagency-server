use super::*;
use std::sync::Mutex;

/// Answers each request with the first scripted reply for its method and
/// path, and records every request in order.
#[derive(Default)]
struct Fake {
    replies: Mutex<Vec<(&'static str, String, u16, Value)>>,
    sent: Mutex<Vec<Request>>,
}

impl Fake {
    fn on(self, method: &'static str, path: &str, status: u16, body: Value) -> Self {
        self.replies
            .lock()
            .unwrap()
            .push((method, path.into(), status, body));
        self
    }
    fn sent(&self) -> Vec<(String, String, Option<Value>)> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|r| (r.method.to_string(), r.path.clone(), r.body.clone()))
            .collect()
    }
}

impl Transport for &Fake {
    async fn send(&self, request: Request) -> Result<Response> {
        self.sent.lock().unwrap().push(request.clone());
        let mut replies = self.replies.lock().unwrap();
        let index = replies
            .iter()
            .position(|(m, p, _, _)| *m == request.method && *p == request.path)
            .unwrap_or_else(|| panic!("unexpected {} {}", request.method, request.path));
        let (_, _, status, body) = replies.remove(index);
        Ok(Response { status, body })
    }
}

fn repo() -> Repo {
    Repo::parse("octo/demo").unwrap()
}

#[test]
fn repository_names_cannot_escape_their_path_segment() {
    assert!(Repo::parse("octo/demo").is_ok());
    for bad in [
        "octo",
        "octo/",
        "/demo",
        "octo/../x",
        "octo/de mo",
        "o/d/e",
        "../demo",
        "octo/..",
    ] {
        assert!(Repo::parse(bad).is_err(), "{bad}");
    }
    assert_eq!(Backend::parse("gitcode"), Some(Backend::GitCode));
    assert_eq!(Backend::parse("gitlab"), None);
}

#[tokio::test]
async fn github_task_flow_creates_branch_pull_and_release_once() {
    let fake = Fake::default()
        .on("GET", "/repos/octo/demo/branches/gitfab%2FT-1", 404, json!({}))
        .on("POST", "/repos/octo/demo/git/refs", 201, json!({}))
        .on("GET", "/repos/octo/demo/branches/gitfab%2FT-1", 200, json!({"commit": {"sha": "abc"}}))
        .on("GET", "/repos/octo/demo/pulls?state=open&head=octo%3Agitfab%2FT-1&base=main&per_page=100", 200, json!([]))
        .on("POST", "/repos/octo/demo/pulls", 201, json!({"number": 7, "html_url": "https://github.com/octo/demo/pull/7", "head": {"sha": "abc"}}))
        .on("GET", "/repos/octo/demo/releases/tags/v0.1.0", 404, json!({}))
        .on("POST", "/repos/octo/demo/releases", 201, json!({"html_url": "https://github.com/octo/demo/releases/tag/v0.1.0"}));
    let forge = Forge::new(Backend::GitHub, &fake);
    assert_eq!(
        forge
            .ensure_branch(&repo(), "gitfab/T-1", "base")
            .await
            .unwrap(),
        "abc"
    );
    let pull = forge
        .upsert_pull(&repo(), "gitfab/T-1", "main", "T-1", "spec")
        .await
        .unwrap();
    assert_eq!((pull.number, pull.head_sha.as_str()), (7, "abc"));
    let release = forge
        .release(&repo(), "v0.1.0", "abc", "v0.1.0", "notes")
        .await
        .unwrap();
    assert_eq!(
        release.url,
        "https://github.com/octo/demo/releases/tag/v0.1.0"
    );
    let sent = fake.sent();
    assert_eq!(
        sent[1].2,
        Some(json!({"ref": "refs/heads/gitfab/T-1", "sha": "base"}))
    );
    assert_eq!(
        sent[4].2,
        Some(json!({"title": "T-1", "head": "gitfab/T-1", "base": "main", "body": "spec"}))
    );
}

#[tokio::test]
async fn an_open_pull_request_is_updated_not_duplicated() {
    let fake = Fake::default()
        .on("GET", "/repos/octo/demo/pulls?state=open&base=main&per_page=100", 200, json!([
            {"number": 3, "html_url": "https://gitcode.com/octo/demo/pulls/3", "head": {"ref": "other", "sha": "x"}},
            {"number": 4, "html_url": "https://gitcode.com/octo/demo/pulls/4", "head": {"ref": "gitfab/T-2", "sha": "def"}}
        ]))
        .on("PATCH", "/repos/octo/demo/pulls/4", 200, json!({"title": "new"}));
    let forge = Forge::new(Backend::GitCode, &fake);
    let pull = forge
        .upsert_pull(&repo(), "gitfab/T-2", "main", "new", "body")
        .await
        .unwrap();
    assert_eq!(pull.number, 4);
    assert_eq!(fake.sent().len(), 2, "no second Pull Request was created");
}

#[tokio::test]
async fn gitcode_uses_its_own_field_names() {
    let fake = Fake::default()
        .on("GET", "/repos/octo/demo/branches/gitfab%2FT-3", 404, json!({}))
        .on("POST", "/repos/octo/demo/branches", 200, json!({"name": "gitfab/T-3"}))
        .on("GET", "/repos/octo/demo/branches/gitfab%2FT-3", 200, json!({"commit": {"id": "c0ffee"}}))
        .on("GET", "/repos/octo/demo/releases/v1", 404, json!({}))
        .on("POST", "/repos/octo/demo/releases", 200, json!({"tag_name": "v1"}))
        .on("GET", "/repos/octo/demo", 200, json!({"full_name": "octo/demo", "default_branch": "main", "web_url": "https://gitcode.com/octo/demo"}));
    let forge = Forge::new(Backend::GitCode, &fake);
    assert_eq!(
        forge
            .ensure_branch(&repo(), "gitfab/T-3", "main")
            .await
            .unwrap(),
        "c0ffee"
    );
    assert_eq!(
        fake.sent()[1].2,
        Some(json!({"refs": "main", "branch_name": "gitfab/T-3"}))
    );
    let release = forge
        .release(&repo(), "v1", "c0ffee", "v1", "n")
        .await
        .unwrap();
    assert_eq!(release.url, "https://gitcode.com/octo/demo/releases/v1");
    assert_eq!(forge.repo(&repo()).await.unwrap().default_branch, "main");
}

#[tokio::test]
async fn existing_branch_and_release_are_reused() {
    let fake = Fake::default()
        .on(
            "GET",
            "/repos/octo/demo/branches/main",
            200,
            json!({"commit": {"sha": "1"}}),
        )
        .on(
            "GET",
            "/repos/octo/demo/releases/tags/v2",
            200,
            json!({"html_url": "https://github.com/octo/demo/releases/tag/v2"}),
        );
    let forge = Forge::new(Backend::GitHub, &fake);
    assert_eq!(
        forge.ensure_branch(&repo(), "main", "x").await.unwrap(),
        "1"
    );
    assert_eq!(
        forge
            .release(&repo(), "v2", "1", "v2", "")
            .await
            .unwrap()
            .url,
        "https://github.com/octo/demo/releases/tag/v2"
    );
    assert!(fake.sent().iter().all(|(m, _, _)| m == "GET"));
}

#[tokio::test]
async fn github_checks_combine_statuses_and_check_runs() {
    let pull = PullRequest {
        number: 1,
        url: String::new(),
        head_sha: "abc".into(),
    };
    let case = |status: Value, runs: Value| {
        Fake::default()
            .on("GET", "/repos/octo/demo/commits/abc/status", 200, status)
            .on(
                "GET",
                "/repos/octo/demo/commits/abc/check-runs?per_page=100",
                200,
                runs,
            )
    };
    let none = case(
        json!({"total_count": 0, "state": "pending"}),
        json!({"check_runs": []}),
    );
    assert_eq!(
        Forge::new(Backend::GitHub, &none)
            .checks(&repo(), &pull)
            .await
            .unwrap(),
        Checks::None
    );
    let running = case(
        json!({"total_count": 0}),
        json!({"check_runs": [{"status": "in_progress"}]}),
    );
    assert_eq!(
        Forge::new(Backend::GitHub, &running)
            .checks(&repo(), &pull)
            .await
            .unwrap(),
        Checks::Pending
    );
    let passed = case(
        json!({"total_count": 1, "state": "success"}),
        json!({"check_runs": [{"status": "completed", "conclusion": "skipped"}]}),
    );
    assert_eq!(
        Forge::new(Backend::GitHub, &passed)
            .checks(&repo(), &pull)
            .await
            .unwrap(),
        Checks::Success
    );
    let failed = case(
        json!({"total_count": 1, "state": "success"}),
        json!({"check_runs": [{"status": "completed", "conclusion": "failure"}]}),
    );
    assert_eq!(
        Forge::new(Backend::GitHub, &failed)
            .checks(&repo(), &pull)
            .await
            .unwrap(),
        Checks::Failure
    );
}

#[tokio::test]
async fn gitcode_checks_read_the_pipeline_state() {
    let pull = PullRequest {
        number: 9,
        url: String::new(),
        head_sha: String::new(),
    };
    for (state, want) in [
        (json!("success"), Checks::Success),
        (json!("failed"), Checks::Failure),
        (json!("running"), Checks::Pending),
        (Value::Null, Checks::None),
    ] {
        let fake = Fake::default().on(
            "GET",
            "/repos/octo/demo/pulls?state=all&per_page=100",
            200,
            json!([{"number": 9, "pipeline_status": state}]),
        );
        assert_eq!(
            Forge::new(Backend::GitCode, &fake)
                .checks(&repo(), &pull)
                .await
                .unwrap(),
            want
        );
    }
}

#[tokio::test]
async fn failures_name_the_call_but_never_the_query() {
    let fake = Fake::default().on(
        "GET",
        "/user/repos?per_page=100",
        401,
        json!({"message": "Bad credentials"}),
    );
    let error = Forge::new(Backend::GitHub, &fake)
        .list_repos()
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "GitHub answered 401 to GET /user/repos");
}

#[tokio::test]
async fn baseline_commit_and_pull_lookups() {
    let fake = Fake::default()
        .on("GET", "/repos/octo/demo/commits/abc", 200, json!({"sha": "abc"}))
        .on("GET", "/repos/octo/demo/commits/nope", 422, json!({}))
        .on("GET", "/repos/octo/demo/commits/gone", 404, json!({}))
        .on("GET", "/repos/octo/demo/pulls/5", 200, json!({"number": 5, "html_url": "https://github.com/octo/demo/pull/5", "head": {"sha": "abc"}}));
    let forge = Forge::new(Backend::GitHub, &fake);
    assert!(forge.commit_exists(&repo(), "abc").await.unwrap());
    assert!(!forge.commit_exists(&repo(), "nope").await.unwrap());
    assert!(!forge.commit_exists(&repo(), "gone").await.unwrap());
    assert_eq!(forge.pull(&repo(), 5).await.unwrap().head_sha, "abc");
}

#[test]
fn repository_references_accept_web_and_clone_addresses() {
    for reference in [
        "octo/demo",
        "https://github.com/octo/demo",
        "https://gitcode.com/octo/demo.git",
        "https://github.com/octo/demo/",
    ] {
        assert_eq!(Repo::from_ref(reference).unwrap(), repo(), "{reference}");
    }
    assert!(Repo::from_ref("https://github.com/octo").is_err());
}

#[tokio::test]
async fn without_checks_access_actions_runs_decide() {
    let pull = PullRequest { number: 1, url: String::new(), head_sha: "abc".into() };
    let fake = Fake::default()
        .on("GET", "/repos/octo/demo/commits/abc/status", 200, json!({"total_count": 0}))
        .on("GET", "/repos/octo/demo/commits/abc/check-runs?per_page=100", 403, json!({"message": "Resource not accessible by personal access token"}))
        .on("GET", "/repos/octo/demo/actions/runs?head_sha=abc&per_page=100", 200, json!({"workflow_runs": [{"status": "completed", "conclusion": "failure"}]}));
    assert_eq!(Forge::new(Backend::GitHub, &fake).checks(&repo(), &pull).await.unwrap(), Checks::Failure);
}

#[tokio::test]
async fn a_gitcode_pull_request_created_with_only_web_url_is_read() {
    let fake = Fake::default()
        .on("GET", "/repos/octo/demo/pulls?state=open&base=main&per_page=100", 200, json!([]))
        .on("POST", "/repos/octo/demo/pulls", 200, json!({"number": 1, "web_url": "https://gitcode.com/octo/demo/merge_requests/1", "head": {"sha": "c"}}));
    let pull = Forge::new(Backend::GitCode, &fake).upsert_pull(&repo(), "b", "main", "t", "").await.unwrap();
    assert_eq!(pull.url, "https://gitcode.com/octo/demo/merge_requests/1");
}

#[tokio::test]
async fn a_pull_request_created_without_its_head_commit_is_read_back() {
    let fake = Fake::default()
        .on("GET", "/repos/octo/demo/pulls?state=open&base=main&per_page=100", 200, json!([]))
        .on("POST", "/repos/octo/demo/pulls", 200, json!({"number": 2, "web_url": "https://gitcode.com/octo/demo/merge_requests/2"}))
        .on("GET", "/repos/octo/demo/pulls/2", 200, json!({"number": 2, "html_url": "https://gitcode.com/octo/demo/merge_requests/2", "head": {"sha": "93d7"}}));
    let pull = Forge::new(Backend::GitCode, &fake).upsert_pull(&repo(), "b", "main", "t", "").await.unwrap();
    assert_eq!((pull.number, pull.head_sha.as_str()), (2, "93d7"));
    // A read that fails keeps the created Pull Request, without a commit.
    let fake = Fake::default()
        .on("GET", "/repos/octo/demo/pulls?state=open&base=main&per_page=100", 200, json!([]))
        .on("POST", "/repos/octo/demo/pulls", 200, json!({"number": 2, "web_url": "https://gitcode.com/octo/demo/merge_requests/2"}))
        .on("GET", "/repos/octo/demo/pulls/2", 500, json!({}));
    let pull = Forge::new(Backend::GitCode, &fake).upsert_pull(&repo(), "b", "main", "t", "").await.unwrap();
    assert_eq!((pull.url.as_str(), pull.head_sha.as_str()), ("https://gitcode.com/octo/demo/merge_requests/2", ""));
}

#[tokio::test]
async fn github_review_takes_each_reviewers_latest_verdict() {
    let pull = PullRequest {
        number: 4,
        url: String::new(),
        head_sha: String::new(),
    };
    let review = |user: &str, state: &str| json!({"user": {"login": user}, "state": state});
    for (reviews, want) in [
        (json!([]), Review::None),
        (json!([review("a", "COMMENTED")]), Review::None),
        (json!([review("a", "CHANGES_REQUESTED"), review("a", "APPROVED")]), Review::Approved),
        (json!([review("a", "APPROVED"), review("b", "CHANGES_REQUESTED")]), Review::ChangesRequested),
        (json!([review("a", "CHANGES_REQUESTED"), review("a", "DISMISSED")]), Review::None),
    ] {
        let fake = Fake::default().on("GET", "/repos/octo/demo/pulls/4/reviews?per_page=100", 200, reviews);
        let got = Forge::new(Backend::GitHub, &fake).review(&repo(), &pull).await.unwrap();
        assert_eq!(got, want);
    }
}

#[tokio::test]
async fn gitcode_review_is_approved_when_every_assigned_reviewer_accepted() {
    let pull = PullRequest {
        number: 9,
        url: String::new(),
        head_sha: String::new(),
    };
    let reviewer = |accept: bool| json!({"login": "r", "assigness": true, "code_owner": false, "accept": accept});
    for (assignees, want) in [
        (json!([]), Review::None),
        (json!([reviewer(true), reviewer(false)]), Review::Pending),
        (json!([reviewer(true), reviewer(true)]), Review::Approved),
    ] {
        let fake = Fake::default().on(
            "GET",
            "/repos/octo/demo/pulls/9",
            200,
            json!({"number": 9, "assignees": assignees}),
        );
        let got = Forge::new(Backend::GitCode, &fake).review(&repo(), &pull).await.unwrap();
        assert_eq!(got, want);
    }
}

/// A local HTTP server that answers each request with the next canned head and counts requests.
fn canned(answers: Vec<&'static str>) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = seen.clone();
    std::thread::spawn(move || {
        for (stream, head) in listener.incoming().zip(answers) {
            let Ok(mut s) = stream else { continue };
            let mut raw = Vec::new();
            let mut buf = [0u8; 1024];
            while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => raw.extend_from_slice(&buf[..n]),
                }
            }
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let body = "{\"full_name\":\"octo/demo\",\"default_branch\":\"main\",\"html_url\":\"https://github.com/octo/demo\"}";
            let _ = write!(s, "{head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    (api, seen)
}

#[tokio::test]
async fn a_rate_limited_call_waits_as_asked_and_retries() {
    use std::sync::atomic::Ordering::SeqCst;
    let (api, seen) = canned(vec!["HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0", "HTTP/1.1 200 OK"]);
    let forge = Forge::new(Backend::GitHub, Http::new(Backend::GitHub, Some(&api), "t".into()));
    assert_eq!(forge.repo(&repo()).await.unwrap().default_branch, "main");
    assert_eq!(seen.load(SeqCst), 2);

    // GitHub's primary limit is a 403 with no requests left; it is retried too.
    let (api, seen) = canned(vec![
        "HTTP/1.1 403 Forbidden\r\nx-ratelimit-remaining: 0\r\nx-ratelimit-reset: 0",
        "HTTP/1.1 200 OK",
    ]);
    let forge = Forge::new(Backend::GitHub, Http::new(Backend::GitHub, Some(&api), "t".into()));
    assert!(forge.repo(&repo()).await.is_ok());
    assert_eq!(seen.load(SeqCst), 2);
}

#[tokio::test]
async fn a_refusal_or_a_long_rate_limit_is_returned_not_waited_for() {
    use std::sync::atomic::Ordering::SeqCst;
    // A plain 403 is a refusal: no retry.
    let (api, seen) = canned(vec!["HTTP/1.1 403 Forbidden", "HTTP/1.1 200 OK"]);
    let forge = Forge::new(Backend::GitHub, Http::new(Backend::GitHub, Some(&api), "t".into()));
    let error = forge.repo(&repo()).await.unwrap_err();
    assert!(matches!(error, Error::Status { status: 403, .. }), "{error}");
    assert_eq!(seen.load(SeqCst), 1);

    // A wait beyond the limit becomes the error at once.
    let (api, seen) = canned(vec!["HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3600", "HTTP/1.1 200 OK"]);
    let http = Http::new(Backend::GitCode, Some(&api), "t".into()).with_max_wait(std::time::Duration::from_secs(5));
    let started = std::time::Instant::now();
    let error = Forge::new(Backend::GitCode, http).repo(&repo()).await.unwrap_err();
    assert!(matches!(error, Error::Status { status: 429, .. }), "{error}");
    assert_eq!(seen.load(SeqCst), 1);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));

    // Retries stop after three.
    let limited = "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0";
    let (api, seen) = canned(vec![limited; 5]);
    let forge = Forge::new(Backend::GitHub, Http::new(Backend::GitHub, Some(&api), "t".into()));
    assert!(forge.repo(&repo()).await.is_err());
    assert_eq!(seen.load(SeqCst), 4);
}

#[tokio::test]
async fn conflicts_are_read_from_each_platform() {
    let pull = PullRequest { number: 5, url: String::new(), head_sha: String::new() };
    for (body, want) in [
        (json!({"mergeable": true, "mergeable_state": "clean"}), Some(false)),
        (json!({"mergeable": false, "mergeable_state": "dirty"}), Some(true)),
        (json!({"mergeable": null}), None),
    ] {
        let fake = Fake::default().on("GET", "/repos/octo/demo/pulls/5", 200, body);
        assert_eq!(Forge::new(Backend::GitHub, &fake).conflict(&repo(), &pull).await.unwrap(), want);
    }
    for (body, want) in [
        (json!({"mergeable": true, "mergeable_state": {"conflict_passed": true}}), Some(false)),
        (json!({"mergeable": true, "mergeable_state": {"conflict_passed": false}}), Some(true)),
        (json!({"mergeable": false}), Some(true)),
        (json!({}), None),
    ] {
        let fake = Fake::default().on("GET", "/repos/octo/demo/pulls/5", 200, body);
        assert_eq!(Forge::new(Backend::GitCode, &fake).conflict(&repo(), &pull).await.unwrap(), want);
    }
}

#[tokio::test]
async fn a_file_is_named_by_its_blob_on_both_platforms() {
    let blob = "b".repeat(40);
    for backend in [Backend::GitHub, Backend::GitCode] {
        let prefix = "/repos/octo/demo";
        let fake = Fake::default()
            .on(
                "GET",
                &format!("{prefix}/contents/spec/DEMO-02.md?ref=abc"),
                200,
                json!({"type": "file", "path": "spec/DEMO-02.md", "sha": blob}),
            )
            .on(
                "GET",
                &format!("{prefix}/contents/spec/missing.md?ref=abc"),
                404,
                json!({}),
            )
            .on(
                "GET",
                &format!("{prefix}/contents/spec?ref=abc"),
                200,
                json!([{"type": "file"}]),
            );
        let forge = Forge::new(backend, &fake);
        assert_eq!(
            forge
                .file_blob(&repo(), "spec/DEMO-02.md", "abc")
                .await
                .unwrap(),
            Some(blob.clone())
        );
        assert_eq!(
            forge
                .file_blob(&repo(), "spec/missing.md", "abc")
                .await
                .unwrap(),
            None
        );
        assert!(matches!(
            forge.file_blob(&repo(), "spec", "abc").await,
            Err(Error::Shape { .. })
        ));
        for bad in ["", "/etc/passwd", "spec/../x", "spec//x", "./spec"] {
            assert!(
                matches!(
                    forge.file_blob(&repo(), bad, "abc").await,
                    Err(Error::InvalidPath)
                ),
                "{bad}"
            );
        }
    }
}
