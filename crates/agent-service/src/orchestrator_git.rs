//! Pull Requests, test state and Releases on a GitFab run's Git backend (design: "Git 后端").
//!
//! Agents only commit and push to the task branch. The orchestrator creates or updates the Pull
//! Request when Execute succeeds, reads the platform's test state when Test succeeds and publishes
//! the Release when the Release stage succeeds. Links are read back from the platform and recorded in
//! the signed events, so an Agent cannot supply them, and the token never reaches an Agent.

use std::path::Path;

use hagency_gitfab_forge::{
    Backend, Checks, Forge, Http, PullRequest, Repo, Request, Response, Review, Transport,
};

use crate::orchestrator_flow::Stage;
use crate::transport::OrchestratorRun;

/// The real HTTPS transport, or a scripted one in tests.
pub enum GitTransport {
    Http(Http),
    #[cfg(test)]
    Fake(std::sync::Arc<dyn Fn(&Request) -> Response + Send + Sync>),
}

impl Transport for GitTransport {
    async fn send(&self, request: Request) -> hagency_gitfab_forge::Result<Response> {
        match self {
            Self::Http(http) => http.send(request).await,
            #[cfg(test)]
            Self::Fake(answer) => Ok(answer(&request)),
        }
    }
}

pub type GitHost = Forge<GitTransport>;

/// One host per backend that has a token configured, and the repository each Hagency project may
/// target with them.
#[derive(Default)]
pub struct GitHosts {
    pub github: Option<GitHost>,
    pub gitcode: Option<GitHost>,
    /// Hagency project id (`prj_…`) → the one backend and repository its runs may use. A run's
    /// `gitBackend`/`repoRef` come from whoever starts it, so they must match this binding: otherwise
    /// any project member could point the server's tokens at any repository they reach.
    pub projects: std::collections::BTreeMap<String, (Backend, Repo)>,
}

impl GitHosts {
    /// Reads each token file (mode 0600, one token). `api` overrides the public API root.
    pub fn load(
        github: Option<(&Path, Option<&str>)>,
        gitcode: Option<(&Path, Option<&str>)>,
    ) -> Result<Self, String> {
        let host =
            |backend: Backend, (file, api): (&Path, Option<&str>)| -> Result<GitHost, String> {
                let token = read_token(file)?;
                Ok(Forge::new(
                    backend,
                    GitTransport::Http(Http::new(backend, api, token)),
                ))
            };
        Ok(Self {
            github: github.map(|c| host(Backend::GitHub, c)).transpose()?,
            gitcode: gitcode.map(|c| host(Backend::GitCode, c)).transpose()?,
            projects: Default::default(),
        })
    }
    /// Binds each project to its repository (`[gitfab.repos]`): project id, backend name, `owner/name`
    /// or a web address.
    pub fn with_projects<'s>(
        mut self,
        projects: impl IntoIterator<Item = (&'s str, &'s str, &'s str)>,
    ) -> Result<Self, String> {
        for (project, backend, repo) in projects {
            let b = Backend::parse(backend)
                .ok_or_else(|| format!("{project}: backend must be github or gitcode"))?;
            let r = Repo::from_ref(repo).map_err(|_| format!("{project}: repo must be owner/name"))?;
            self.projects.insert(project.to_owned(), (b, r));
        }
        Ok(self)
    }
    /// Whether `project` may target this backend and repository: only the one it is bound to.
    pub fn allows(&self, project: &str, backend: &str, repo: &str) -> Result<(), &'static str> {
        let backend = Backend::parse(backend).ok_or("gitfab_repo_invalid")?;
        let repo = Repo::from_ref(repo).map_err(|_| "gitfab_repo_invalid")?;
        match self.projects.get(project) {
            Some((b, r))
                if *b == backend
                    && r.owner.eq_ignore_ascii_case(&repo.owner)
                    && r.name.eq_ignore_ascii_case(&repo.name) =>
            {
                Ok(())
            }
            _ => Err("gitfab_repo_not_bound"),
        }
    }
    pub fn get(&self, backend: Backend) -> Option<&GitHost> {
        match backend {
            Backend::GitHub => self.github.as_ref(),
            Backend::GitCode => self.gitcode.as_ref(),
        }
    }
}

/// One token from a file readable by its owner only.
pub fn read_token(file: &Path) -> Result<String, String> {
    let name = file.display();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(file)
            .map_err(|_| format!("{name}: unreadable"))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(format!("{name}: must be mode 0600"));
        }
    }
    let text = std::fs::read_to_string(file).map_err(|_| format!("{name}: unreadable"))?;
    let token = text.trim();
    if token.is_empty() || token.contains(char::is_whitespace) {
        return Err(format!("{name}: must hold one token"));
    }
    Ok(token.to_owned())
}

/// What a platform step adds to the stage's event and Thread.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct GitStep {
    pub refs: Vec<String>,
    pub note: Option<String>,
    /// The Pull Request head the platform acted on.
    pub commit: Option<String>,
    /// Why the platform did not do it (refused, unreachable, unexpected answer).
    pub error: Option<String>,
    /// Git blob SHA of the run's Spec file at that head, so an edit to the confirmed Spec in the
    /// repository shows as a different blob between the run's events.
    pub spec_blob: Option<String>,
}

/// A head SHA the platform reported, when it is one.
fn commit(sha: &str) -> Option<String> {
    (matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| sha.to_ascii_lowercase())
}

fn context<'r>(run: &'r OrchestratorRun, key: &str) -> &'r str {
    run.init.context[key].as_str().unwrap_or("")
}

/// Branch names allow only these characters here; anything else becomes `-`.
fn segment(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// `agent/<project_id>/<task_id>` (Annex 3 §5).
pub(crate) fn task_branch(run: &OrchestratorRun) -> String {
    let project = Some(context(run, "projectId"))
        .filter(|p| !p.is_empty())
        .unwrap_or(&run.init.project_id);
    format!("agent/{}/{}", segment(project), segment(&run.init.task_id))
}

fn release_tag(run: &OrchestratorRun, round: u32) -> String {
    match context(run, "releaseTag") {
        "" => format!("gitfab-{}-r{round}", segment(&run.init.task_id)),
        tag => segment(tag),
    }
}

/// The run's backend and repository, when its host has a token.
pub(crate) fn target<'h>(
    hosts: Option<&'h GitHosts>,
    run: &OrchestratorRun,
) -> Option<(&'h GitHost, Repo)> {
    let hosts = hosts?;
    let (backend, repo) = (context(run, "gitBackend"), context(run, "repoRef"));
    // Checked when the run started; checked again here in case the binding changed since.
    if let Err(code) = hosts.allows(&run.init.project_id, backend, repo) {
        tracing::warn!(run = %run.id, code, "gitfab run target is not the project's repository; no platform call");
        return None;
    }
    let host = hosts.get(Backend::parse(backend)?)?;
    Some((host, Repo::from_ref(repo).ok()?))
}

/// Refuses a new run whose repository is not its project's, whenever the server holds Git tokens.
/// Without tokens nothing reaches a platform, so any reference is accepted.
pub(crate) fn check_target(
    hosts: Option<&GitHosts>,
    init: &crate::transport::OrchestratorRunInit,
) -> crate::Result<()> {
    let Some(hosts) = hosts else { return Ok(()) };
    let text = |k: &str| init.context[k].as_str().unwrap_or("");
    hosts
        .allows(&init.project_id, text("gitBackend"), text("repoRef"))
        .map_err(crate::Error::Unauthorized)
}

/// The platform action for a stage that just succeeded. Failures become a note and no links; the
/// run itself goes on, and an acceptance run sees the missing links.
pub(crate) async fn step(
    host: &GitHost,
    repo: &Repo,
    run: &OrchestratorRun,
    stage: Stage,
    round: u32,
    reviewed: Option<&str>,
) -> GitStep {
    let branch = task_branch(run);
    let result = async {
        let base = host.repo(repo).await?.default_branch;
        Ok::<_, hagency_gitfab_forge::Error>(match stage {
            Stage::Execute => {
                let title = format!(
                    "[GitFab] {}: {}",
                    run.init.task_id,
                    context(run, "goal").lines().next().unwrap_or("")
                );
                let body = format!(
                    "任务 {}\nSpec：{}\n运行：{}",
                    run.init.task_id,
                    context(run, "specRef"),
                    run.id
                );
                let pull = host
                    .upsert_pull(repo, &branch, &base, &title, &body)
                    .await?;
                GitStep {
                    note: Some(format!("已创建或更新 PR：{}", pull.url)),
                    commit: commit(&pull.head_sha),
                    refs: vec![pull.url],
                    error: None,
                    spec_blob: None,
                }
            }
            Stage::Test => match host.open_pull(repo, &branch, &base).await? {
                Some(pull) => {
                    let state = host.checks(repo, &pull).await?;
                    // Informational: the run's verdict comes from the Reviewer Agent, so a review
                    // that cannot be read does not fail the step.
                    let review = host.review(repo, &pull).await.ok();
                    let conflict = host.conflict(repo, &pull).await.ok().flatten();
                    GitStep {
                        note: Some(format!(
                            "平台测试状态：{}（{}）；平台审查：{}；合并：{}",
                            checks_text(state),
                            pull.url,
                            review_text(review),
                            conflict_text(conflict)
                        )),
                        commit: commit(&pull.head_sha),
                        refs: vec![pull.url],
                        error: None,
                        spec_blob: None,
                    }
                }
                None => no_pull(&branch),
            },
            Stage::Release => match host.open_pull(repo, &branch, &base).await? {
                // Only the commit the round's Test recorded, which the Reviewer reviewed, is released;
                // a commit pushed after the review is not.
                Some(PullRequest { url, head_sha, .. }) if commit(&head_sha) != reviewed.and_then(commit) => {
                    let error = match reviewed {
                        None => format!("no reviewed commit recorded for round {round}; not releasing"),
                        Some(sha) => format!("Pull Request head {head_sha} is not the reviewed commit {sha}; not releasing"),
                    };
                    GitStep {
                        note: Some(format!("未发布 Release：PR 当前的头提交不是评审过的提交（{error}）。请在新一轮 Execute/Test/Review 后再发布。")),
                        commit: commit(&head_sha),
                        refs: vec![url],
                        error: Some(error),
                        spec_blob: None,
                    }
                }
                Some(PullRequest { url, head_sha, .. }) if !head_sha.is_empty() => {
                    let tag = release_tag(run, round);
                    let notes = format!("任务 {}，运行 {}，PR {url}", run.init.task_id, run.id);
                    let release = host.release(repo, &tag, &head_sha, &tag, &notes).await?;
                    GitStep {
                        note: Some(format!("已发布 Release {}：{}", release.tag, release.url)),
                        commit: commit(&head_sha),
                        refs: vec![release.url, url],
                        error: None,
                        spec_blob: None,
                    }
                }
                _ => no_pull(&branch),
            },
            _ => GitStep::default(),
        })
    }
    .await;
    let mut step = result.unwrap_or_else(|e| GitStep {
        note: Some(format!("Git 平台操作未完成：{e}")),
        error: Some(e.to_string()),
        ..GitStep::default()
    });
    // Informational: a Spec file that cannot be read leaves the field out, and acceptance reports it.
    if let (Some(head), Some(path)) = (step.commit.clone(), spec_path(run)) {
        match host.file_blob(repo, &path, &head).await {
            Ok(blob) => step.spec_blob = blob.as_deref().and_then(commit),
            Err(e) => tracing::warn!(error = %e, "gitfab Spec file was not read"),
        }
    }
    step
}

/// The repository path of the run's Spec: `specRef` without its `@version`, when it is a relative
/// path inside the repository.
pub(crate) fn spec_path(run: &OrchestratorRun) -> Option<String> {
    let path = context(run, "specRef").split('@').next()?.trim();
    (!path.is_empty()
        && !path.contains("://")
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != ".."))
    .then(|| path.to_owned())
}

fn no_pull(branch: &str) -> GitStep {
    GitStep {
        note: Some(format!("没有找到任务分支 {branch} 的 PR。")),
        error: Some(format!("no open pull request for {branch}")),
        ..GitStep::default()
    }
}

fn checks_text(state: Checks) -> &'static str {
    match state {
        Checks::Success => "通过",
        Checks::Failure => "失败",
        Checks::Pending => "进行中",
        Checks::None => "平台未上报测试",
    }
}

fn conflict_text(conflict: Option<bool>) -> &'static str {
    match conflict {
        Some(false) => "与默认分支无冲突",
        Some(true) => "与默认分支有冲突，需在任务分支上解决后重新执行（GitFab 不自动合并）",
        None => "平台尚未给出冲突状态",
    }
}

fn review_text(state: Option<Review>) -> &'static str {
    match state {
        Some(Review::None) => "无人审查",
        Some(Review::Pending) => "待审查",
        Some(Review::Approved) => "已批准",
        Some(Review::ChangesRequested) => "要求修改",
        None => "未能读取",
    }
}

/// Whether a stage's success calls the platform.
pub(crate) fn drives(stage: Stage) -> bool {
    matches!(stage, Stage::Execute | Stage::Test | Stage::Release)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::orchestrator_flow::State;
    use crate::transport::OrchestratorRunInit;

    fn run() -> OrchestratorRun {
        OrchestratorRun {
            id: "orun_1".into(),
            init: OrchestratorRunInit {
                project_id: "prj".into(),
                task_id: "task-1".into(),
                room_id: "!r:example.test".into(),
                thread_root: "$root".into(),
                requester_mxid: "@h:example.test".into(),
                bindings: json!({}),
                context: json!({"gitBackend": "github", "repoRef": "https://github.com/o/r",
                    "projectId": "DEMO-02", "goal": "g", "specRef": "spec/DEMO-02.md@v1"}),
            },
            state: State::Completed,
            revision: 1,
            dispatch_id: None,
            handoff: None,
            pending_events: json!([]),
            updated_at_ms: 0,
        }
    }

    fn host(answer: impl Fn(&Request) -> Response + Send + Sync + 'static) -> GitHost {
        Forge::new(Backend::GitHub, GitTransport::Fake(Arc::new(answer)))
    }

    fn repo_answer(r: &Request) -> Option<Response> {
        (r.method == "GET" && r.path == "/repos/o/r").then(|| Response {
            status: 200,
            body: json!({"full_name": "o/r", "default_branch": "main",
                "html_url": "https://github.com/o/r"}),
        })
    }

    #[test]
    fn the_spec_path_is_the_spec_reference_inside_the_repository() {
        let with = |spec: &str| {
            let mut r = run();
            r.init.context["specRef"] = json!(spec);
            spec_path(&r)
        };
        assert_eq!(
            with("spec/DEMO-02.md@v1").as_deref(),
            Some("spec/DEMO-02.md")
        );
        assert_eq!(with("docs/spec.md").as_deref(), Some("docs/spec.md"));
        for outside in [
            "",
            "@v1",
            "/etc/spec.md",
            "../spec.md",
            "https://example.test/spec.md",
            "spec//x.md",
        ] {
            assert_eq!(with(outside), None, "{outside}");
        }
    }

    #[tokio::test]
    async fn a_pull_request_step_names_the_head_commit() {
        let head = "A".repeat(40);
        let pull = json!({"number": 3, "html_url": "https://github.com/o/r/pull/3",
            "head": {"ref": "agent/DEMO-02/task-1", "sha": head}});
        let host = host(move |r| {
            repo_answer(r).unwrap_or_else(|| match r.method {
                "GET" => Response {
                    status: 200,
                    body: json!([]),
                },
                "POST" => Response {
                    status: 201,
                    body: pull.clone(),
                },
                _ => Response {
                    status: 200,
                    body: json!({}),
                },
            })
        });
        let repo = Repo::from_ref("https://github.com/o/r").unwrap();
        let step = step(&host, &repo, &run(), Stage::Execute, 1, None).await;
        assert_eq!(step.refs, vec!["https://github.com/o/r/pull/3".to_owned()]);
        assert_eq!(step.commit.as_deref(), Some("a".repeat(40).as_str()));
        assert_eq!(step.error, None);
    }

    #[tokio::test]
    async fn a_release_publishes_only_the_reviewed_commit() {
        let reviewed = "b".repeat(40);
        let release = |head: &'static str| {
            let posted = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
            let seen = posted.clone();
            let pull = json!({"number": 3, "html_url": "https://github.com/o/r/pull/3",
                "head": {"ref": "agent/DEMO-02/task-1", "sha": head}});
            let host = host(move |r| {
                repo_answer(r).unwrap_or_else(|| match (r.method, r.path.as_str()) {
                    ("GET", p) if p.starts_with("/repos/o/r/pulls?") => Response { status: 200, body: json!([pull.clone()]) },
                    ("POST", "/repos/o/r/releases") => {
                        seen.lock().unwrap().push(r.body.clone().unwrap());
                        Response { status: 201, body: json!({"html_url": "https://github.com/o/r/releases/tag/gitfab-task-1-r1", "tag_name": "gitfab-task-1-r1"}) }
                    }
                    _ => Response { status: 404, body: json!({}) },
                })
            });
            (host, posted)
        };
        let repo = Repo::from_ref("https://github.com/o/r").unwrap();
        // The head is the reviewed commit: released at it.
        let (h, posted) = release("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let released = step(&h, &repo, &run(), Stage::Release, 1, Some(&reviewed)).await;
        assert_eq!(released.error, None, "{:?}", released.note);
        assert_eq!(posted.lock().unwrap()[0]["target_commitish"], reviewed.as_str());
        // A commit pushed after the review: nothing is released, and the step says why.
        let (h, posted) = release("cccccccccccccccccccccccccccccccccccccccc");
        let moved = step(&h, &repo, &run(), Stage::Release, 1, Some(&reviewed)).await;
        assert!(posted.lock().unwrap().is_empty());
        assert!(moved.error.as_deref().is_some_and(|e| e.contains("is not the reviewed commit")), "{:?}", moved.error);
        assert_eq!(moved.refs, vec!["https://github.com/o/r/pull/3".to_owned()]);
        // No reviewed commit recorded for the round: nothing is released either.
        let (h, posted) = release("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let unreviewed = step(&h, &repo, &run(), Stage::Release, 1, None).await;
        assert!(posted.lock().unwrap().is_empty());
        assert!(unreviewed.error.as_deref().is_some_and(|e| e.contains("no reviewed commit")), "{:?}", unreviewed.error);
    }

    #[tokio::test]
    async fn a_refused_platform_write_is_named_without_links() {
        // A token without write access to the repository: the platform answers 403.
        let host = host(|r| {
            repo_answer(r).unwrap_or_else(|| match r.method {
                "GET" => Response {
                    status: 200,
                    body: json!([]),
                },
                _ => Response {
                    status: 403,
                    body: json!({"message": "Resource not accessible"}),
                },
            })
        });
        let repo = Repo::from_ref("https://github.com/o/r").unwrap();
        let step = step(&host, &repo, &run(), Stage::Execute, 1, None).await;
        assert!(step.refs.is_empty());
        assert_eq!(step.commit, None);
        let error = step.error.unwrap();
        assert!(error.contains("403") && error.contains("POST"), "{error}");
        assert!(step.note.unwrap().starts_with("Git 平台操作未完成"));
    }

    #[tokio::test]
    async fn the_test_step_names_the_platform_review_and_survives_an_unreadable_one() {
        for (reviews, want) in [
            (
                Response {
                    status: 200,
                    body: json!([{"user": {"login": "m"}, "state": "APPROVED"}]),
                },
                "平台审查：已批准",
            ),
            (
                Response {
                    status: 403,
                    body: json!({"message": "Resource not accessible"}),
                },
                "平台审查：未能读取",
            ),
        ] {
            let head = "b".repeat(40);
            let pull = json!({"number": 3, "html_url": "https://github.com/o/r/pull/3",
                "head": {"ref": "agent/DEMO-02/task-1", "sha": head}});
            let host = host(move |r| {
                repo_answer(r).unwrap_or_else(|| {
                    let p = r.path.as_str();
                    if p.starts_with("/repos/o/r/pulls?state=open") {
                        Response {
                            status: 200,
                            body: json!([pull]),
                        }
                    } else if p.ends_with("/status") {
                        Response {
                            status: 200,
                            body: json!({"total_count": 1, "state": "success"}),
                        }
                    } else if p.contains("/check-runs") {
                        Response {
                            status: 200,
                            body: json!({"check_runs": []}),
                        }
                    } else if p.starts_with("/repos/o/r/pulls/3/reviews") {
                        reviews.clone()
                    } else if p == "/repos/o/r/pulls/3" {
                        Response {
                            status: 200,
                            body: json!({"number": 3, "mergeable": false}),
                        }
                    } else if p
                        == format!("/repos/o/r/contents/spec/DEMO-02.md?ref={}", "b".repeat(40))
                    {
                        Response {
                            status: 200,
                            body: json!({"type": "file", "sha": "e".repeat(40)}),
                        }
                    } else {
                        panic!("unexpected call {} {p}", r.method)
                    }
                })
            });
            let repo = Repo::from_ref("https://github.com/o/r").unwrap();
            let step = step(&host, &repo, &run(), Stage::Test, 1, None).await;
            let note = step.note.unwrap();
            assert!(note.starts_with("平台测试状态：通过"), "{note}");
            assert!(note.contains(want), "{note}");
            assert!(
                note.ends_with(
                    "合并：与默认分支有冲突，需在任务分支上解决后重新执行（GitFab 不自动合并）"
                ),
                "{note}"
            );
            assert_eq!(step.error, None);
            assert_eq!(step.commit.as_deref(), Some("b".repeat(40).as_str()));
            assert_eq!(step.spec_blob.as_deref(), Some("e".repeat(40).as_str()));
        }
    }

    #[test]
    fn a_project_may_target_only_its_bound_repository() {
        let hosts = GitHosts::default()
            .with_projects([("prj_a", "gitcode", "Owner/Demo")])
            .unwrap();
        assert_eq!(hosts.allows("prj_a", "gitcode", "owner/demo"), Ok(()));
        assert_eq!(hosts.allows("prj_a", "gitcode", "https://gitcode.com/Owner/Demo.git"), Ok(()));
        for (project, backend, repo) in [
            ("prj_a", "gitcode", "owner/other"),
            ("prj_a", "github", "owner/demo"),
            ("prj_b", "gitcode", "owner/demo"),
        ] {
            assert_eq!(hosts.allows(project, backend, repo), Err("gitfab_repo_not_bound"), "{project} {backend} {repo}");
        }
        assert_eq!(hosts.allows("prj_a", "gitlab", "owner/demo"), Err("gitfab_repo_invalid"));
        assert_eq!(hosts.allows("prj_a", "gitcode", "no-slash"), Err("gitfab_repo_invalid"));
        assert!(GitHosts::default().with_projects([("prj_a", "svn", "o/r")]).is_err());
    }

    #[test]
    fn a_new_run_is_checked_against_the_binding_only_when_tokens_are_configured() {
        let mut r = run();
        r.init.project_id = "prj_a".into();
        r.init.context["gitBackend"] = json!("github");
        r.init.context["repoRef"] = json!("https://github.com/o/r");
        assert!(check_target(None, &r.init).is_ok(), "no tokens: nothing reaches a platform");
        let hosts = |repo: &str| {
            GitHosts {
                github: Some(host(|r| panic!("no platform call expected: {r:?}"))),
                gitcode: None,
                projects: Default::default(),
            }
            .with_projects([("prj_a", "github", repo)])
            .unwrap()
        };
        let other = hosts("o/elsewhere");
        assert!(matches!(check_target(Some(&other), &r.init), Err(crate::Error::Unauthorized("gitfab_repo_not_bound"))));
        assert!(target(Some(&other), &r).is_none(), "an unbound target gets no host");
        let bound = hosts("o/r");
        assert!(check_target(Some(&bound), &r.init).is_ok());
        let (_, repo) = target(Some(&bound), &r).expect("the bound repository is the target");
        assert_eq!((repo.owner.as_str(), repo.name.as_str()), ("o", "r"));
    }

    #[test]
    fn only_a_hex_sha_is_taken_as_the_commit() {
        assert_eq!(commit(""), None);
        assert_eq!(commit("main"), None);
        assert_eq!(commit(&"g".repeat(40)), None);
        assert_eq!(commit(&"F".repeat(64)), Some("f".repeat(64)));
    }
}
