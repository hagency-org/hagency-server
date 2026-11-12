//! Git hosting operations GitFab needs from GitHub and GitCode (Annex 1, Git
//! backend): list and select repositories, create task branches, create or
//! update Pull Requests, read their test state and publish Releases.
//!
//! Cloning, committing and pushing happen with `git` in the executor's
//! workspace; this crate covers what only the hosting API can do. Every call
//! goes through a [`Transport`], so the flows are tested without a token or
//! network. The token never appears in a URL or an error.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};
use std::future::Future;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    GitHub,
    GitCode,
}

impl Backend {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "github" => Some(Self::GitHub),
            "gitcode" => Some(Self::GitCode),
            _ => None,
        }
    }
    /// Public API root of the hosted service.
    pub fn default_api(self) -> &'static str {
        match self {
            Self::GitHub => "https://api.github.com",
            Self::GitCode => "https://api.gitcode.com/api/v5",
        }
    }
    fn default_web(self) -> &'static str {
        match self {
            Self::GitHub => "https://github.com",
            Self::GitCode => "https://gitcode.com",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("repository must be written as owner/name")]
    InvalidRepo,
    #[error("{backend:?} answered {status} to {method} {path}")]
    Status {
        backend: Backend,
        status: u16,
        method: &'static str,
        path: String,
    },
    #[error("{backend:?} response to {path} lacks {field}")]
    Shape {
        backend: Backend,
        path: String,
        field: &'static str,
    },
    #[error("could not reach the hosting service")]
    Network,
}

pub type Result<T> = std::result::Result<T, Error>;

/// `owner/name`, checked so it can only form a single API path segment each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    pub fn parse(full: &str) -> Result<Self> {
        let (owner, name) = full.split_once('/').ok_or(Error::InvalidRepo)?;
        let ok = |s: &str| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        };
        if !ok(owner) || !ok(name) {
            return Err(Error::InvalidRepo);
        }
        Ok(Self {
            owner: owner.into(),
            name: name.into(),
        })
    }
    /// `owner/name`, or a web or clone address ending in it.
    pub fn from_ref(reference: &str) -> Result<Self> {
        let trimmed = reference.trim().trim_end_matches('/');
        let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
        match trimmed.split_once("://") {
            // scheme://host/owner/name: exactly two segments after the host.
            Some((_, rest)) => match rest.split_once('/') {
                Some((_, path)) => Self::parse(path),
                None => Err(Error::InvalidRepo),
            },
            None => Self::parse(trimmed),
        }
    }
    fn path(&self) -> String {
        format!("/repos/{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub full_name: String,
    pub default_branch: String,
    pub web_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub head_sha: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checks {
    /// Nothing reported a test result for this change.
    None,
    Pending,
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: &'static str,
    /// Path below the API root, query included.
    pub path: String,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Value,
}

pub trait Transport: Sync {
    fn send(&self, request: Request) -> impl Future<Output = Result<Response>> + Send;
}

/// Real HTTPS transport. The token goes only into the Authorization header.
pub struct Http {
    client: reqwest::Client,
    api: String,
    token: String,
    backend: Backend,
}

impl Http {
    pub fn new(backend: Backend, api: Option<&str>, token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api: api
                .unwrap_or(backend.default_api())
                .trim_end_matches('/')
                .into(),
            token,
            backend,
        }
    }
}

impl Transport for Http {
    async fn send(&self, request: Request) -> Result<Response> {
        let method =
            reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|_| Error::Network)?;
        let mut builder = self
            .client
            .request(method, format!("{}{}", self.api, request.path))
            .bearer_auth(&self.token)
            .header("User-Agent", "hagency-gitfab");
        if self.backend == Backend::GitHub {
            builder = builder
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
        }
        if let Some(body) = request.body {
            builder = builder.json(&body);
        }
        let response = builder.send().await.map_err(|_| Error::Network)?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|_| Error::Network)?;
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok(Response { status, body })
    }
}

/// Branch, tag and query values become one encoded path or query component.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');
fn enc(value: &str) -> String {
    utf8_percent_encode(value, COMPONENT).to_string()
}

pub struct Forge<T> {
    backend: Backend,
    web: String,
    transport: T,
}

impl<T: Transport> Forge<T> {
    pub fn new(backend: Backend, transport: T) -> Self {
        Self {
            backend,
            web: backend.default_web().into(),
            transport,
        }
    }
    /// For self-hosted instances whose web root differs from the public one.
    pub fn with_web(mut self, web: &str) -> Self {
        self.web = web.trim_end_matches('/').into();
        self
    }
    pub fn backend(&self) -> Backend {
        self.backend
    }

    async fn call(
        &self,
        method: &'static str,
        path: String,
        body: Option<Value>,
        allow_404: bool,
    ) -> Result<Option<Value>> {
        let response = self
            .transport
            .send(Request {
                method,
                path: path.clone(),
                body,
            })
            .await?;
        match response.status {
            200..=299 => Ok(Some(response.body)),
            404 if allow_404 => Ok(None),
            status => Err(Error::Status {
                backend: self.backend,
                status,
                method,
                path: strip_query(&path),
            }),
        }
    }
    async fn get(&self, path: String) -> Result<Value> {
        Ok(self
            .call("GET", path, None, false)
            .await?
            .unwrap_or(Value::Null))
    }
    fn field<'a>(&self, value: &'a Value, path: &str, field: &'static str) -> Result<&'a str> {
        field
            .split('.')
            .try_fold(value, |v, key| v.get(key))
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Shape {
                backend: self.backend,
                path: strip_query(path),
                field,
            })
    }

    fn repo_info(&self, value: &Value, path: &str) -> Result<RepoInfo> {
        let web = match self.backend {
            Backend::GitHub => "html_url",
            Backend::GitCode => "web_url",
        };
        Ok(RepoInfo {
            full_name: self.field(value, path, "full_name")?.into(),
            default_branch: self.field(value, path, "default_branch")?.into(),
            web_url: self.field(value, path, web)?.into(),
        })
    }

    /// Repositories the token's user can see (first 100), for selection.
    pub async fn list_repos(&self) -> Result<Vec<RepoInfo>> {
        let path = "/user/repos?per_page=100".to_string();
        let list = self.get(path.clone()).await?;
        list.as_array()
            .ok_or(Error::Shape {
                backend: self.backend,
                path: "/user/repos".into(),
                field: "[]",
            })?
            .iter()
            .map(|item| self.repo_info(item, &path))
            .collect()
    }

    /// Whether `sha` names a commit in the repository (a baseline check).
    pub async fn commit_exists(&self, repo: &Repo, sha: &str) -> Result<bool> {
        let path = format!("{}/commits/{}", repo.path(), enc(sha));
        // GitHub answers 422 for a malformed SHA; both mean "no such commit".
        match self.call("GET", path, None, true).await {
            Ok(found) => Ok(found.is_some()),
            Err(Error::Status { status: 422, .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// One Pull Request by number.
    pub async fn pull(&self, repo: &Repo, number: u64) -> Result<PullRequest> {
        let path = format!("{}/pulls/{number}", repo.path());
        let value = self.get(path.clone()).await?;
        self.pull_from(&value, &path)
    }

    pub async fn repo(&self, repo: &Repo) -> Result<RepoInfo> {
        let path = repo.path();
        let value = self.get(path.clone()).await?;
        self.repo_info(&value, &path)
    }

    /// Head commit of a branch, or `None` when the branch does not exist.
    pub async fn branch_head(&self, repo: &Repo, branch: &str) -> Result<Option<String>> {
        let path = format!("{}/branches/{}", repo.path(), enc(branch));
        let Some(value) = self.call("GET", path.clone(), None, true).await? else {
            return Ok(None);
        };
        // GitCode names it commit.id on a single branch, commit.sha elsewhere.
        let sha = match self.backend {
            Backend::GitHub => self.field(&value, &path, "commit.sha")?,
            Backend::GitCode => self
                .field(&value, &path, "commit.id")
                .or_else(|_| self.field(&value, &path, "commit.sha"))?,
        };
        Ok(Some(sha.into()))
    }

    /// Create the task branch from `from` unless it already exists, and
    /// return its head. GitHub needs `from` as a commit SHA; GitCode documents
    /// a branch name.
    pub async fn ensure_branch(&self, repo: &Repo, branch: &str, from: &str) -> Result<String> {
        if let Some(head) = self.branch_head(repo, branch).await? {
            return Ok(head);
        }
        let (path, body) = match self.backend {
            Backend::GitHub => (
                format!("{}/git/refs", repo.path()),
                json!({"ref": format!("refs/heads/{branch}"), "sha": from}),
            ),
            Backend::GitCode => (
                format!("{}/branches", repo.path()),
                json!({"refs": from, "branch_name": branch}),
            ),
        };
        self.call("POST", path, Some(body), false).await?;
        let path = format!("{}/branches/{}", repo.path(), enc(branch));
        self.branch_head(repo, branch).await?.ok_or(Error::Shape {
            backend: self.backend,
            path,
            field: "commit",
        })
    }

    fn pull_from(&self, value: &Value, path: &str) -> Result<PullRequest> {
        Ok(PullRequest {
            number: value
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::Shape {
                    backend: self.backend,
                    path: strip_query(path),
                    field: "number",
                })?,
            url: self.field(value, path, "html_url")?.into(),
            head_sha: self
                .field(value, path, "head.sha")
                .unwrap_or_default()
                .into(),
        })
    }

    /// The open Pull Request from `head` into `base`, if any.
    pub async fn open_pull(
        &self,
        repo: &Repo,
        head: &str,
        base: &str,
    ) -> Result<Option<PullRequest>> {
        // GitHub filters by head; GitCode does not document that filter, so
        // its open Pull Requests into `base` are matched here by source branch.
        let path = match self.backend {
            Backend::GitHub => format!(
                "{}/pulls?state=open&head={}&base={}&per_page=100",
                repo.path(),
                enc(&format!("{}:{head}", repo.owner)),
                enc(base)
            ),
            Backend::GitCode => format!(
                "{}/pulls?state=open&base={}&per_page=100",
                repo.path(),
                enc(base)
            ),
        };
        let list = self.get(path.clone()).await?;
        let Some(items) = list.as_array() else {
            return Err(Error::Shape {
                backend: self.backend,
                path: strip_query(&path),
                field: "[]",
            });
        };
        items
            .iter()
            .find(|item| item.pointer("/head/ref").and_then(Value::as_str) == Some(head))
            .map(|item| self.pull_from(item, &path))
            .transpose()
    }

    /// Create the task's Pull Request, or update the open one for `head`.
    pub async fn upsert_pull(
        &self,
        repo: &Repo,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest> {
        if let Some(existing) = self.open_pull(repo, head, base).await? {
            let path = format!("{}/pulls/{}", repo.path(), existing.number);
            self.call(
                "PATCH",
                path,
                Some(json!({"title": title, "body": body})),
                false,
            )
            .await?;
            return Ok(existing);
        }
        let path = format!("{}/pulls", repo.path());
        let created = self
            .get_body(
                "POST",
                path.clone(),
                json!({"title": title, "head": head, "base": base, "body": body}),
            )
            .await?;
        self.pull_from(&created, &path)
    }

    async fn get_body(&self, method: &'static str, path: String, body: Value) -> Result<Value> {
        Ok(self
            .call(method, path, Some(body), false)
            .await?
            .unwrap_or(Value::Null))
    }

    /// Test state of the Pull Request's head commit.
    pub async fn checks(&self, repo: &Repo, pull: &PullRequest) -> Result<Checks> {
        match self.backend {
            Backend::GitHub => {
                let sha = enc(&pull.head_sha);
                let status = self
                    .get(format!("{}/commits/{sha}/status", repo.path()))
                    .await?;
                let runs = self
                    .get(format!(
                        "{}/commits/{sha}/check-runs?per_page=100",
                        repo.path()
                    ))
                    .await?;
                let mut states: Vec<Checks> = Vec::new();
                if status
                    .get("total_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
                {
                    states.push(match status.get("state").and_then(Value::as_str) {
                        Some("success") => Checks::Success,
                        Some("failure" | "error") => Checks::Failure,
                        _ => Checks::Pending,
                    });
                }
                for run in runs
                    .get("check_runs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    states.push(
                        match (
                            run.get("status").and_then(Value::as_str),
                            run.get("conclusion").and_then(Value::as_str),
                        ) {
                            (Some("completed"), Some("success" | "neutral" | "skipped")) => {
                                Checks::Success
                            }
                            (Some("completed"), _) => Checks::Failure,
                            _ => Checks::Pending,
                        },
                    );
                }
                Ok(combine(&states))
            }
            Backend::GitCode => {
                // No per-commit status endpoint is documented; Pull Request
                // listings carry the pipeline state.
                let path = format!("{}/pulls?state=all&per_page=100", repo.path());
                let list = self.get(path).await?;
                let item =
                    list.as_array().into_iter().flatten().find(|item| {
                        item.get("number").and_then(Value::as_u64) == Some(pull.number)
                    });
                Ok(
                    match item
                        .and_then(|i| i.get("pipeline_status"))
                        .and_then(Value::as_str)
                    {
                        Some("success" | "passed") => Checks::Success,
                        Some("failed" | "failure" | "canceled" | "error") => Checks::Failure,
                        Some("") | None => Checks::None,
                        Some(_) => Checks::Pending,
                    },
                )
            }
        }
    }

    /// Publish a Release for `tag` at `target`, reusing one that exists.
    pub async fn release(
        &self,
        repo: &Repo,
        tag: &str,
        target: &str,
        name: &str,
        notes: &str,
    ) -> Result<Release> {
        let lookup = match self.backend {
            Backend::GitHub => format!("{}/releases/tags/{}", repo.path(), enc(tag)),
            Backend::GitCode => format!("{}/releases/{}", repo.path(), enc(tag)),
        };
        let found = self.call("GET", lookup, None, true).await?;
        if found.is_none() {
            let path = format!("{}/releases", repo.path());
            self.get_body(
                "POST",
                path,
                json!({"tag_name": tag, "target_commitish": target, "name": name, "body": notes}),
            )
            .await
            .map(|created| self.release_from(repo, tag, &created))
        } else {
            Ok(self.release_from(repo, tag, found.as_ref().unwrap_or(&Value::Null)))
        }
    }
    fn release_from(&self, repo: &Repo, tag: &str, value: &Value) -> Release {
        // GitCode documents no URL on the release object; its page is stable.
        let url = value
            .get("html_url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{}/{}/{}/releases/{}",
                    self.web,
                    repo.owner,
                    repo.name,
                    enc(tag)
                )
            });
        Release {
            tag: tag.into(),
            url,
        }
    }
}

/// Any failure fails; any pending keeps it pending; all passing passes.
fn combine(states: &[Checks]) -> Checks {
    if states.is_empty() {
        Checks::None
    } else if states.contains(&Checks::Failure) {
        Checks::Failure
    } else if states.contains(&Checks::Pending) {
        Checks::Pending
    } else {
        Checks::Success
    }
}

fn strip_query(path: &str) -> String {
    path.split('?').next().unwrap_or_default().into()
}
