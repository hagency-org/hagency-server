use anyhow::{Context, Result, ensure};
use clap::Args;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Args)]
pub struct Options {
    #[arg(long)]
    pub dev: bool,
    #[arg(long, default_value = "http://127.0.0.1:8088")]
    pub origin: String,
    #[arg(long, default_value = "localhost:8088")]
    pub server_name: String,
    #[arg(long)]
    pub output_dir: Option<PathBuf>,
}

pub fn generate(root: &Path, options: &Options) -> Result<()> {
    let output = options.output_dir.clone().unwrap_or_else(|| {
        root.join(if options.dev {
            "config/dev"
        } else {
            "config/docker"
        })
    });
    let output = std::path::absolute(output)?;
    ensure!(
        !output.try_exists()?,
        "Configuration directory already exists: {}",
        output.display()
    );
    let env = root.join(".env");
    ensure!(
        options.dev || !env.try_exists()?,
        "An .env file already exists; preserve it and configure Compose manually."
    );
    let password = if options.dev {
        std::env::var("HAGENCY_DB_PASSWORD")
            .ok()
            .or(read_password(&env)?)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "hagency_dev".into())
    } else {
        let bytes: [u8; 32] = rand::random();
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    };
    let configs = render(root, options, &password)?;
    fs::create_dir_all(output.parent().context("output needs a parent directory")?)?;
    DirBuilder::new().mode(0o700).create(&output)?;
    let result: Result<()> = (|| {
        for (name, text) in configs {
            write_private(&output.join(format!("{name}.toml")), &text)?;
        }
        if !options.dev {
            write_private(&env, &format!("HAGENCY_DB_PASSWORD={password}\n"))?;
        }
        Ok(())
    })();
    if result.is_err() {
        // This directory was created exclusively above; never remove an existing profile.
        let _ = fs::remove_dir_all(&output);
    }
    result?;
    println!("Created component configurations in {}", output.display());
    Ok(())
}

fn read_password(path: &Path) -> Result<Option<String>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    Ok(fs::read_to_string(path)?
        .lines()
        .find_map(|line| line.strip_prefix("HAGENCY_DB_PASSWORD="))
        .map(|value| value.trim().trim_matches(['\'', '"']).to_owned()))
}

fn quoted(value: &str) -> Result<String> {
    // TOML serialization handles quotes/control characters without shell expansion.
    Ok(toml::Value::String(value.into()).to_string())
}

fn render(root: &Path, options: &Options, password: &str) -> Result<Vec<(&'static str, String)>> {
    let password = utf8_percent_encode(password, NON_ALPHANUMERIC);
    let endpoint = if options.dev {
        "127.0.0.1:55438"
    } else {
        "postgres:5432"
    };
    let path = |relative: &str, production: &str| -> Result<String> {
        if options.dev {
            quoted(
                root.join(relative)
                    .to_str()
                    .context("project path must be UTF-8")?,
            )
        } else {
            quoted(production)
        }
    };
    ["hagency", "palpo", "pasion"]
        .into_iter()
        .map(|name| {
            let mut text = fs::read_to_string(root.join(format!("config/examples/{name}.toml")))?
                .replace("\"http://127.0.0.1:8088\"", &quoted(&options.origin)?)
                .replace("\"localhost:8088\"", &quoted(&options.server_name)?)
                // The management administrator carries the server name as its
                // Matrix domain; keep it in step with `--server-name`.
                .replace(
                    "@admin:localhost:8088",
                    &format!("@admin:{}", options.server_name),
                )
                .replace(
                    "hagency:hagency_dev@127.0.0.1:55438/",
                    &format!("hagency:{password}@{endpoint}/"),
                );
            match name {
                "hagency" => {
                    text = text.replace(
                        "data_dir = \"../../data\"",
                        &format!("data_dir = {}", path("data", "/app/data")?),
                    );
                    text = text.replace(
                        "public_dir = \"../../resources/frontend/public\"",
                        &format!(
                            "public_dir = {}",
                            path(
                                "resources/frontend/public",
                                "/app/resources/frontend/public"
                            )?
                        ),
                    );
                    if !options.dev {
                        text = text
                            .replace("listen = \"127.0.0.1:8088\"", "listen = \"0.0.0.0:8088\"");
                    }
                }
                "palpo" => {
                    text = text.replace(
                        "root = \"../../data/media\"",
                        &format!("root = {}", path("data/media", "/app/data/media")?),
                    );
                }
                "pasion" => {
                    text = text.replace(
                        "resources_dir = \"../../resources/pasion\"",
                        &format!(
                            "resources_dir = {}",
                            path("resources/pasion", "/app/resources/pasion")?
                        ),
                    );
                }
                _ => unreachable!(),
            }
            Ok((name, text))
        })
        .collect()
}

pub(crate) fn write_private(path: &Path, value: &str) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(value.as_bytes())?;
    Ok(())
}
