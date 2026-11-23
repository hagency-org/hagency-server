//! `gitfab-forge`: GitHub and GitCode operations for GitFab acceptance runs.
//! Exit 0 on success, 1 when the hosting service refuses or a check fails,
//! 2 for usage, token or network errors. Results go to stdout as JSON.
//!
//! The token is read from the file named by `GITFAB_TOKEN_FILE`, which must
//! not be readable by group or others. It is never printed.

use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use hagency_gitfab_forge::{Backend, Checks, Error, Forge, Http, Repo, Review};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "gitfab-forge",
    about = "GitHub and GitCode operations for GitFab"
)]
struct Cli {
    /// github or gitcode
    #[arg(long)]
    backend: String,
    /// API root, for a self-hosted instance
    #[arg(long)]
    api: Option<String>,
    /// Web root, for a self-hosted instance
    #[arg(long)]
    web: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Repositories the token can see.
    Repos,
    /// One repository: full name, default branch and web address.
    Repo { repo: String },
    /// Head commit of a branch (exit 1 when it does not exist).
    Branch { repo: String, branch: String },
    /// Whether a commit exists (exit 1 when it does not).
    Commit { repo: String, sha: String },
    /// Create the branch from `--from` unless it exists; print its head.
    EnsureBranch {
        repo: String,
        branch: String,
        #[arg(long)]
        from: String,
    },
    /// Create the Pull Request for `--head`, or update the open one.
    Pull {
        repo: String,
        #[arg(long)]
        head: String,
        #[arg(long)]
        base: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        body_file: Option<std::path::PathBuf>,
    },
    /// Test state of a Pull Request (exit 1 unless it passed).
    Checks { repo: String, number: u64 },
    /// Review state of a Pull Request on the platform: none, pending, approved or changes_requested.
    /// Reads only; exit 1 unless approved.
    Review { repo: String, number: u64 },
    /// Git blob SHA of a file at a commit or branch (exit 1 when there is no such file). Reads only.
    File {
        repo: String,
        #[arg(long)]
        path: String,
        #[arg(long = "ref")]
        git_ref: String,
    },
    /// The published Release for a tag (exit 1 when there is none). Reads only.
    ReleaseInfo {
        repo: String,
        #[arg(long)]
        tag: String,
    },
    /// Publish a Release, reusing one with the same tag.
    Release {
        repo: String,
        #[arg(long)]
        tag: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        notes_file: Option<std::path::PathBuf>,
    },
}

enum Failure {
    /// The service answered, and the answer is a failed check.
    Check(Value),
    /// Usage, token or transport problem.
    Setup(String),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        match error {
            Error::Status { .. } | Error::Shape { .. } => {
                Failure::Check(json!({"error": error.to_string()}))
            }
            Error::InvalidRepo | Error::InvalidPath | Error::Network => {
                Failure::Setup(error.to_string())
            }
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return finish(Err(Failure::Setup(
                "could not start the async runtime".into(),
            )));
        }
    };
    finish(runtime.block_on(run(cli)))
}

fn finish(result: Result<Value, Failure>) -> ExitCode {
    match result {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(Failure::Check(value)) => {
            println!("{value}");
            ExitCode::from(1)
        }
        Err(Failure::Setup(message)) => {
            eprintln!("{}", json!({"error": message}));
            ExitCode::from(2)
        }
    }
}

fn token() -> Result<String, Failure> {
    let path = std::env::var_os("GITFAB_TOKEN_FILE")
        .ok_or(Failure::Setup("set GITFAB_TOKEN_FILE".into()))?;
    let path = Path::new(&path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map_err(|_| Failure::Setup("cannot read GITFAB_TOKEN_FILE".into()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(Failure::Setup("GITFAB_TOKEN_FILE must be mode 0600".into()));
        }
    }
    let token = std::fs::read_to_string(path)
        .map_err(|_| Failure::Setup("cannot read GITFAB_TOKEN_FILE".into()))?;
    let token = token.trim();
    if token.is_empty() || token.contains(char::is_whitespace) {
        return Err(Failure::Setup(
            "GITFAB_TOKEN_FILE must hold one token".into(),
        ));
    }
    Ok(token.to_owned())
}

fn text(file: Option<&Path>) -> Result<String, Failure> {
    file.map_or(Ok(String::new()), |path| {
        std::fs::read_to_string(path)
            .map_err(|_| Failure::Setup(format!("cannot read {}", path.display())))
    })
}

async fn run(cli: Cli) -> Result<Value, Failure> {
    let backend = Backend::parse(&cli.backend)
        .ok_or(Failure::Setup("--backend must be github or gitcode".into()))?;
    let mut forge = Forge::new(backend, Http::new(backend, cli.api.as_deref(), token()?));
    if let Some(web) = &cli.web {
        forge = forge.with_web(web);
    }
    let repo = |name: &str| Repo::from_ref(name).map_err(Failure::from);
    let info = |r: hagency_gitfab_forge::RepoInfo| json!({"fullName": r.full_name, "defaultBranch": r.default_branch, "webUrl": r.web_url});
    Ok(match cli.command {
        Command::Repos => Value::Array(forge.list_repos().await?.into_iter().map(info).collect()),
        Command::Repo { repo: name } => info(forge.repo(&repo(&name)?).await?),
        Command::Branch { repo: name, branch } => {
            match forge.branch_head(&repo(&name)?, &branch).await? {
                Some(sha) => json!({"branch": branch, "sha": sha}),
                None => return Err(Failure::Check(json!({"branch": branch, "exists": false}))),
            }
        }
        Command::Commit { repo: name, sha } => {
            if !forge.commit_exists(&repo(&name)?, &sha).await? {
                return Err(Failure::Check(json!({"sha": sha, "exists": false})));
            }
            json!({"sha": sha, "exists": true})
        }
        Command::EnsureBranch {
            repo: name,
            branch,
            from,
        } => {
            json!({"branch": branch, "sha": forge.ensure_branch(&repo(&name)?, &branch, &from).await?})
        }
        Command::Pull {
            repo: name,
            head,
            base,
            title,
            body_file,
        } => {
            let body = text(body_file.as_deref())?;
            let pull = forge
                .upsert_pull(&repo(&name)?, &head, &base, &title, &body)
                .await?;
            json!({"number": pull.number, "url": pull.url, "headSha": pull.head_sha})
        }
        Command::Checks { repo: name, number } => {
            let repo = repo(&name)?;
            let pull = forge.pull(&repo, number).await?;
            let state = forge.checks(&repo, &pull).await?;
            let report = json!({"number": number, "headSha": pull.head_sha, "checks": format!("{state:?}").to_lowercase()});
            if state != Checks::Success {
                return Err(Failure::Check(report));
            }
            report
        }
        Command::Review { repo: name, number } => {
            let repo = repo(&name)?;
            let pull = forge.pull(&repo, number).await?;
            let state = forge.review(&repo, &pull).await?;
            let state = match state {
                Review::None => "none",
                Review::Pending => "pending",
                Review::Approved => "approved",
                Review::ChangesRequested => "changes_requested",
            };
            let report = json!({"number": number, "review": state});
            if state != "approved" {
                return Err(Failure::Check(report));
            }
            report
        }
        Command::File {
            repo: name,
            path,
            git_ref,
        } => match forge.file_blob(&repo(&name)?, &path, &git_ref).await? {
            Some(blob) => json!({"path": path, "ref": git_ref, "blob": blob}),
            None => {
                return Err(Failure::Check(
                    json!({"path": path, "ref": git_ref, "exists": false}),
                ));
            }
        },
        Command::ReleaseInfo { repo: name, tag } => {
            match forge.find_release(&repo(&name)?, &tag).await? {
                Some(release) => json!({"tag": release.tag, "url": release.url}),
                None => return Err(Failure::Check(json!({"tag": tag, "exists": false}))),
            }
        }
        Command::Release {
            repo: name,
            tag,
            target,
            name: title,
            notes_file,
        } => {
            let notes = text(notes_file.as_deref())?;
            let title = title.unwrap_or_else(|| tag.clone());
            let release = forge
                .release(&repo(&name)?, &tag, &target, &title, &notes)
                .await?;
            json!({"tag": release.tag, "url": release.url})
        }
    })
}
