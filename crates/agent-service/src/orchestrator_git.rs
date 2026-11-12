//! Pull Requests, test state and Releases on a GitFab run's Git backend (design: "Git 后端").
//!
//! Agents only commit and push to the task branch. The orchestrator creates or updates the Pull
//! Request when Execute succeeds, reads the platform's test state when Test succeeds and publishes
//! the Release when the Release stage succeeds. Links are read back from the platform and recorded in
//! the signed events, so an Agent cannot supply them, and the token never reaches an Agent.

use std::path::Path;

use hagency_gitfab_forge::{
    Backend, Checks, Forge, Http, PullRequest, Repo, Request, Response, Transport,
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

/// One host per backend that has a token configured.
#[derive(Default)]
pub struct GitHosts {
    pub github: Option<GitHost>,
    pub gitcode: Option<GitHost>,
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
        })
    }
    pub fn get(&self, backend: Backend) -> Option<&GitHost> {
        match backend {
            Backend::GitHub => self.github.as_ref(),
            Backend::GitCode => self.gitcode.as_ref(),
        }
    }
}

fn read_token(file: &Path) -> Result<String, String> {
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
    let host = hosts?.get(Backend::parse(context(run, "gitBackend"))?)?;
    Some((host, Repo::from_ref(context(run, "repoRef")).ok()?))
}

/// The platform action for a stage that just succeeded. Failures become a note and no links; the
/// run itself goes on, and an acceptance run sees the missing links.
pub(crate) async fn step(
    host: &GitHost,
    repo: &Repo,
    run: &OrchestratorRun,
    stage: Stage,
    round: u32,
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
                    refs: vec![pull.url],
                }
            }
            Stage::Test => match host.open_pull(repo, &branch, &base).await? {
                Some(pull) => {
                    let state = host.checks(repo, &pull).await?;
                    GitStep {
                        note: Some(format!(
                            "平台测试状态：{}（{}）",
                            checks_text(state),
                            pull.url
                        )),
                        refs: vec![pull.url],
                    }
                }
                None => no_pull(&branch),
            },
            Stage::Release => match host.open_pull(repo, &branch, &base).await? {
                Some(PullRequest { url, head_sha, .. }) if !head_sha.is_empty() => {
                    let tag = release_tag(run, round);
                    let notes = format!("任务 {}，运行 {}，PR {url}", run.init.task_id, run.id);
                    let release = host.release(repo, &tag, &head_sha, &tag, &notes).await?;
                    GitStep {
                        note: Some(format!("已发布 Release {}：{}", release.tag, release.url)),
                        refs: vec![release.url, url],
                    }
                }
                _ => no_pull(&branch),
            },
            _ => GitStep::default(),
        })
    }
    .await;
    result.unwrap_or_else(|e| GitStep {
        refs: Vec::new(),
        note: Some(format!("Git 平台操作未完成：{e}")),
    })
}

fn no_pull(branch: &str) -> GitStep {
    GitStep {
        refs: Vec::new(),
        note: Some(format!("没有找到任务分支 {branch} 的 PR。")),
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

/// Whether a stage's success calls the platform.
pub(crate) fn drives(stage: Stage) -> bool {
    matches!(stage, Stage::Execute | Stage::Test | Stage::Release)
}
