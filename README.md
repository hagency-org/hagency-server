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
and sign in as `admin` using the password in `secrets/admin-password`. That
account is also the management administrator (`[management] administrator` in
`config/dev/hagency.toml`): it admits resources, and in Hagency Desktop it can
use the Resource Owner, Server Admin and Agent manager workspaces. To make a
different account the administrator, change that line and restart.
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

The schema 34 native CLI cutover requires matching current Desktop and owner-daemon
builds. Actual worker requests declare `native-context-v3;claude-v4`; the declaration
expires after five minutes. An outdated or offline worker leaves incoming messages
pending with a visible update/reconnect reason. Reconnecting resumes the original
message, and declaration expiry never blocks settlement of work already started.
Treat it as a forward-only deployment: the SQL files named `*_down.sql`
are test fixtures, not a supported production rollback. In particular, lowering
the version does not restore the previous context-validation trigger, and a
Claude runtime cannot be represented by the previous schema. Returning to an
older binary requires restoring a pre-cutover Hagency database or starting with
a fresh Hagency database; replacing the executable alone is not a rollback.
Matrix and Pasion databases are separate from that Hagency reset.

Schema 35 lets an owner device replace its local ledger database. The device
declares the new database's id with
`POST /hagency/api/v1/management/executor-ledger/declare`; from then on the server
checks its receipt numbers for reuse only against receipts numbered by that
database. The migration is additive: existing receipts stay in epoch 0, and a
device that never declares behaves as before, so the server can be deployed
before any client changes. It is forward-only like schema 34: an older binary
refuses a schema 35 database with `management_schema_incompatible`, and
`management_executor_ledgers_down.sql` is a test fixture that refuses to run
once any ledger has been declared.

A declaration that replaces a declared database, or a first declaration sent
with `"fresh": true`, ends what the earlier database held for that device, in
the same transaction: its pools and grants are revoked, its pooled Agents are
fenced, its project allocations and parent settlements are closed, and its
unanswered commands are never delivered or answered again
(`executor_ledger_replaced`). Recorded receipts and history are kept as they
are, and usage the device never reported is not added. Other devices, other
owners and the owner's local Agents are not touched. A first declaration
without `fresh` only names the database the device already had and ends
nothing. For what such a device keeps, ordering rules still compare bare
numbers across ledgers: a grant, Agent or runtime created before the
declaration accepts later receipts only above the numbers already reported.

Registration is disabled by default. Initializers refuse to overwrite existing
configuration. For existing data, see [migration instructions](docs/guide.md#upgrading-the-former-combined-database).

Detailed configuration, registration, local source development and migration:
[English](docs/guide.md) · [中文](docs/guide.zh-CN.md).

Hagency workflow architecture: [English](docs/OPERATIONS.md) · [中文](docs/OPERATIONS.zh-CN.md).

Local operations: [connection/TLS diagnostics](docs/LOCAL_DEPLOYMENT.md) ·
[isolated testing](docs/TESTING.md).

[GitHub CI and releases](docs/github-ci.md)
