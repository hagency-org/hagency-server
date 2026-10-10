# hagency-server

[中文](README.zh-CN.md) · [Detailed documentation](docs/README.md)

Palpo Matrix, Pasion authentication and Padmin/Hagency management in one Rust server.

The integrated Agent Appservice is required. Users sign in through Pasion with
their own Matrix account and permanently own their Agents. Project maps to a
Space; its Rooms retain independent membership. The server manages creation
rights and message transport; local hagency-client runs Codex and controls
resource/tool policy. Browser pages are `/hagency/projects` and `/hagency/agents`.

Fleet/Hafleet, Engagement/allocation approvals and authority import are removed.
There is no old-data import or compatibility mode. Palpo/Pasion generic
administration remains available through its existing component APIs.

## Local development

Install Rust ≥ 1.99, [just](https://github.com/casey/just), Docker,
Git, curl and `libpq`. The password example below also uses OpenSSL.
Run these commands from the repository root on first setup:

```sh
just init-dev
just db-up
just prepare-pasion
just prepare-frontend
```

Configurations are generated in `config/dev/`: `hagency.toml`, `palpo.toml`
and `pasion.toml`. One PostgreSQL service holds three separate databases:
`hagency`, `palpo` and `pasion`.

Create a protected password file and the first Pasion administrator:

```sh
umask 077
mkdir -p secrets
openssl rand -base64 32 > secrets/admin-password
just run --config config/dev/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file secrets/admin-password
```

Once the server starts, stop it with `Ctrl+C`, then start the development watcher:

```sh
just dev
```

Open [the console](http://127.0.0.1:8088/login), choose **Sign in with Pasion**,
and sign in as `admin` using the password in `secrets/admin-password`.
Rust changes rebuild automatically; refresh after frontend changes.
For later starts, use `just db-up` and `just dev`, without initialization or bootstrap.

## Docker deployment

For a fresh deployment, use the same prerequisites and create the protected
password file above. Replace `palpo.instance` with your domain:

```sh
just init-docker --origin https://palpo.instance --server-name palpo.instance
just db-up
just docker-build
docker compose run --rm --service-ports \
  -v "$PWD/secrets/admin-password:/app/bootstrap-password:ro" server \
  --config /app/config/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file /run/hagency/bootstrap-password
```

Once the server starts, stop it with `Ctrl+C`, then run:

```sh
docker compose up -d server
```

Configuration lives in `config/docker/`; `.env` holds the generated database password.
Proxy `https://palpo.instance` to `127.0.0.1:8088`, preserving paths and the public
Host header. Open `https://palpo.instance/login` and sign in through Pasion as `admin`.
For later starts, use `just docker-up`.

Registration is disabled by default. Initializers refuse to overwrite existing
configuration. For existing data, see [migration instructions](docs/guide.md#upgrading-the-former-combined-database).

Detailed configuration, registration, local source development and migration:
[English](docs/guide.md) · [中文](docs/guide.zh-CN.md).

Hagency workflow architecture: [English](docs/OPERATIONS.md) · [中文](docs/OPERATIONS.zh-CN.md).

Local operations: [connection/TLS diagnostics](docs/LOCAL_DEPLOYMENT.md) ·
[isolated testing](docs/TESTING.md).

[GitHub CI and releases](docs/github-ci.md)
