# Configuration and development guide

[中文](guide.zh-CN.md) · [Quick start](../README.md) · [Documentation](README.md)

Palpo Matrix homeserver, Pasion authentication and Palpo web-admin in **one Rust
process and one HTTP listener**. The application uses Rust, Tokio, Salvo, Diesel/diesel-async and
PostgreSQL, matching Palpo's backend. No Node server runs in production.

The Cargo workspace follows Pasion's backend/frontend organization:

```text
crates/
├─ backend/   # hagency-server: embedded Palpo, Pasion and Hagency APIs
├─ agent-service/ # permanent Agent domain and transport
└─ frontend/  # hagency-frontend: Dioxus/WASM, based on copied Padmin source
xtask/       # development/configuration tools (workspace member)
resources/   # generated frontend and Pasion assets, ignored by Git
```

The frontend combines Padmin's generic Matrix/Pasion administration with
permanent Agent ownership, Project/Room bindings and creation policy.
The public site homepage is served at `/`; the authenticated member Home is at
`/hagency/home`, Projects and My agents at `/hagency/projects` and `/hagency/agents`,
and administrator pages under `/admin`. Matrix endpoints remain at `/_matrix`,
Palpo administration at `/_palpo`, discovery at `/.well-known/matrix`, and
native Hagency APIs at `/hagency/api/v1` and browser management at
`/hagency/api/browser/v1`. Pasion remains mounted at `/auth/`.
Browser login/session/logout use `/hagency/api/login/token`, `/hagency/api/session`
and `/hagency/api/logout`. Hagency custom APIs live under `/hagency/`;
`/_matrix/` is reserved for Matrix protocol endpoints, including the standard
Application Service callbacks. Former `/api/hagency/v1`, `/api/browser/hagency/v1`
and browser session URLs return 404. Upgrade Server, Client/Desktop and browser
assets together; this path change does not change the OAuth issuer or stored identities.
`/healthz` reports process HTTP availability.

## Contents

- [Architecture](#architecture)
- [Development](#development)
- [Unified frontend authentication](#unified-frontend-authentication)
- [Public homepage](#public-homepage)
- [Configuration and initial administrator](#configuration-and-initial-administrator)
- [Compose deployment](#compose-deployment)
- [Retained behavior and limits](#behavior-retained-in-the-rust-port)
- [Validation](#validation)
- [Embedded Pasion](#embedded-pasion)
- [Embedded Palpo upgrade](#upgrading-embedded-palpo)
- [Database migration](#upgrading-the-former-combined-database)
- [Configuration migration](#moving-the-previous-single-configuration-into-component-files)

All shell commands below run from the repository root.

## Architecture

```text
hagency-server (one process, one port)
├─ MatrixServer: Palpo initialization, Matrix/admin/discovery routes and workers
├─ BrowserAuth/BFF: personal sessions and Project/Agent management
├─ required Agent Appservice: durable inbox, fenced leases and reply outbox
├─ PasionServer: OAuth/OIDC, account UI/API and workers at /auth/
└─ integrated Padmin + Hagency Dioxus/WASM assets
           │
           └─ PostgreSQL service
              ├─ hagency: management state + durable outbound queue
              ├─ palpo: Matrix homeserver tables
              └─ pasion: authentication tables
```

Palpo is linked as a Rust library, not launched as a separate process or reached
through an external reverse proxy. Its existing authentication and operations
remain authoritative. For web-admin operations, the Rust adapter calls the
mounted Matrix/admin API over the fixed loopback origin in the **same process**.
This preserves actual Matrix authorization and App Service semantics. Internal
URLs are derived from `listen`; public transport URLs from `public_origin`.

The embedded Palpo API currently permits one MatrixServer per process because
Palpo's configuration and pools are singletons. The host owns Tokio, tracing,
HTTP shutdown and TLS termination. Run with 8 MiB Tokio worker stacks as this
binary does. Matrix background workers stop with the runtime.

## Development

Requirements: Rust >=1.99, [just](https://github.com/casey/just), PostgreSQL client library (`libpq`), and Docker for
the PostgreSQL service. Embedded Pasion also needs the wasm32-unknown-unknown
target and Dioxus 0.7.5 assets; resource preparation downloads the matching
Dioxus CLI when necessary. Git and curl are used for fetching build tools/source.
Both frontends build with Rust and the Dioxus CLI, without Node.js or npm.
Node.js is optional for JavaScript tests in `tests/`. Python 3 is required for
the current Agent PostgreSQL/integration and OpenAPI scripts in `scripts/`.
`rust-toolchain.toml` pins Rust 1.99.0 with rustfmt, Clippy and the WASM target;
Docker build stages use the matching Rust 1.99 images.

`just --list` shows the commands. Just orchestrates them; the workspace’s Rust
`xtask` implements private configuration generation, asset preparation and the
development watcher without compiling the server to run these tools.

```sh
just init-dev
just db-up
just prepare-pasion
just prepare-frontend
# On a new database, bootstrap an administrator as described below first.
just dev
```

Open `http://127.0.0.1:8088`. This development config uses
`localhost:8088` as the stable Matrix server name. Rust changes compile and then
gracefully restart the process. A compilation failure keeps the last working
server running. Frontend Rust/CSS changes rebuild the WASM assets, which the
running host serves immediately; refresh the browser to load them. Frontend-only
changes do not restart the backend. No Docker image rebuild is required.
Browser access/refresh tokens remain in memory. On reload, Project/Agent pages
revalidate the HttpOnly session through `/hagency/api/session` and use the restricted BFF.
Generic Matrix administration re-enters Pasion PKCE when a Matrix bearer is needed.
Expired or revoked sessions require fresh login; tokens are not saved in browser storage.

For Palpo source development, use a local checkout with the MatrixServer API:

```sh
just dev --palpo-source /absolute/path/to/palpo-checkout
```

The development tool writes a local Cargo patch under ignored `.run/`, watches the local
Palpo sources, and rebuilds against them. It does not modify the upstream
checkout or require a rebuilt Docker image. Cargo may update its lockfile when
switching between the pinned Git dependency and a local patch; return to the
normal dependency with `cargo update -p palpo` after removing the override.

The copied Padmin also contains optional `palpo_admin` sidecar screens for
maintenance, scheduled commands and notifications. That separate sidecar is
not embedded here; its menu stays disabled instead of sending requests to
nonexistent Palpo endpoints. Matrix user/room/media/registration/App Service
administration uses the mounted Palpo APIs directly.

## Unified frontend authentication

Pasion is the default identity provider. The host publishes nonsecret runtime
settings at `/config.json`; the integrated frontend has one **Sign in with
Pasion** entry point and uses Authorization Code + PKCE. It reads the signed-in
Pasion account's role and requests administrative scopes only for an
administrator. A fresh administrator login first establishes identity, then
continues authorization for the same Pasion account. Members never receive
administrative scopes.

Pasion owns passwords, registration, account status and administrator roles.
Its provisioning worker synchronizes the corresponding Matrix identities and
administrator flags to Palpo. Manage people at `/admin/pasion/accounts`; `/admin/users`
shows their Matrix records and service identities. Password changes and human
account creation use Pasion. Hagency stores permanent Agent owners, Project/Room creation rights and durable
transport, without another password/account database.

The host provisions the reserved public OAuth client and derives its
`/oauth/callback` URL; no client secret or separate frontend deployment is needed.
The resulting token is verified at `POST /hagency/api/login/token` before issuing an
HttpOnly session cookie and CSRF token. Palpo administration additionally checks
Pasion's live introspection and the exact `urn:palpo:admin:*` scope at the host
boundary; a member-only token is insufficient even for an administrator account.
Introspection caching is disabled so role/token revocation affects the next
request. Pasion's own admin endpoints enforce their scope and current role.

Pasion delegated authentication is required by this integrated deployment. There
is no Hagency native-password or account-approval compatibility mode.

Frontend source provenance and upstream licensing are recorded in `NOTICE`
(Padmin commit `83d4567470ada808b89914aa0c786cdc3a7ac89a`).

## Native client login and Agent ownership

`chrislearn/hagency-client` uses the user's own Matrix account through Pasion PKCE.
Native `/hagency/api/v1` verifies Pasion proof and Matrix whoami, issues short user
sessions, and registers local device credentials. Cookie/Origin requests are
rejected on native APIs. Device rotation/revocation and real-time session expiry
are rechecked during sensitive transactions.

The integrated Appservice is required and server managed. No per-user Fleet or
Appservice enrollment is needed. Agents have permanent owners and independent
Project/Room bindings. Space managers set default creation permission with
allow/deny lists; deny wins and disabling creation does not pause existing Agents.
The local client controls Codex, resource budgets and tool risk decisions.

The browser uses the closed `/hagency/api/browser/v1` BFF and pages
`/hagency/projects` and `/hagency/agents`; it never receives a native device or
session credential. See [Agent architecture](OPERATIONS.md) for the full boundary.

## Public homepage

The public `/` and `/login` routes display the same sign-in page, using the
console's shared Soft Orbit styles, language selector and light/dark appearance.
Sign in with your personal account to enter `/hagency/home`; a verified existing
browser session can continue directly to the workspace. Account creation and
account-center links are available when Pasion is enabled. Management pages are
only available after their existing authorization checks.

The homepage is part of the bundled frontend. There is no `home_page` host
setting, local HTML override or remote-page fetch. Remove a previous `home_page`
entry from hagency.toml before starting this version. The embedded Palpo
homepage setting does not control the Hagency homepage.

## Configuration and initial administrator

Configuration is split into files owned by each component:

```text
config/
├─ examples/       # tracked, native configuration templates
│  ├─ hagency.toml
│  ├─ palpo.toml
│  └─ pasion.toml
├─ dev/            # generated, ignored, mode 0600 files
│  ├─ hagency.toml
│  ├─ palpo.toml
│  └─ pasion.toml
└─ docker/         # generated, ignored, mode 0600 files
   ├─ hagency.toml
   ├─ palpo.toml
   └─ pasion.toml
```

Start with `--config config/dev/hagency.toml` (the CLI default). Hagency owns the
listener, admin connection and data directory, and references the other files:

```toml
palpo_config = "palpo.toml"
pasion_config = "pasion.toml"
```

Palpo uses its native top-level ServerConfig, including `[db]`, `[storage]` and
`[well_known]`; there is no `[matrix]` wrapper. Pasion uses its native sections,
including `[database]`, `[account]`, `[email]`, `[[clients]]` and
`[[upstream_oauth2.providers]]`; there is no `[pasion.settings]` wrapper. Its
additional `[hagency]` section contains `resources_dir` and
`delegate_matrix_auth` for embedding.

| File and setting | Database | Contents |
| --- | --- | --- |
| `hagency.toml`: `database_url` | `hagency` | new Agent identities, domain, AS inbox and reply outbox |
| `palpo.toml`: `db.url` | `palpo` | Matrix users, rooms, events and homeserver state |
| `pasion.toml`: `database.uri` | `pasion` | Accounts, OAuth/OIDC tokens and sessions |

Database names must be distinct. The current integrated Agent server requires
`pasion_config` and `[hagency] delegate_matrix_auth = true` in Pasion configuration;
omission or disabled delegation is rejected, including by `--check-config`.
Paths resolve relative to the
file that declares them, including component references, media and supported
native secret-file references. The development watcher monitors all three files,
including references outside the main configuration directory.

`--check-config` loads/checks all referenced files without starting a server;
`--list-config-files` prints their paths as JSON without connecting to databases.
Hagency's listener replaces Palpo/Pasion listener configuration in embedded mode.
Without an explicit `[keypair]` in `palpo.toml`, a signing key is generated once and
persisted in `data_dir/matrix-signing-key.json` (mode 0600). Back up that directory
and PostgreSQL together. `server_name` is an identity and must not be changed
when reusing a database.

Registration is disabled by default. For a new database, create an administrator
with a password file, then start the same application:

```sh
# Put a strong password in a protected file named secrets/admin-password.
just run --config config/dev/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file secrets/admin-password
```

With the default Pasion delegation, bootstrap creates the first administrator
in Pasion using its configured password policy, and queues the normal Matrix
identity/role synchronization. Sign in through Pasion as `admin`; the Matrix
identity is `@admin:localhost:8088`. Bootstrap refuses to overwrite a Pasion
account or run after an administrator already exists. Grant subsequent roles
in Account management.

For a deployment previously bootstrapped with native Matrix authentication,
explicitly link its active human administrator using the same username and a
password file chosen for Pasion:

```sh
just run --config config/dev/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file secrets/admin-password --link-existing-matrix-admin
```

Linking preserves the Matrix ID, rooms and Hagency ownership. It refuses guest,
service, suspended, deactivated and non-administrator Matrix identities. Other
native accounts/passwords are not silently imported; provision or migrate their
Pasion accounts before enabling delegation. An existing native password file may
be reused if it satisfies Pasion policy. Stop the development watcher before
bootstrap; there must be one server process per database. Subsequent starts
omit all bootstrap options.

Legacy Fleet/account-approval/retirement-token/notification configuration is
rejected. Human registration and account administration belong to Pasion.

## Compose deployment

```sh
just init-docker --origin https://palpo.instance \
  --server-name palpo.instance
just db-up
just docker-build
```

This creates `config/docker/{hagency,palpo,pasion}.toml` and a protected `.env`
with a generated database password. Development generation similarly creates
`config/dev/`; `--output-dir` can select another directory. The generator refuses
to overwrite an existing directory.
The first command prepares configuration without starting anything; the next two
prepare PostgreSQL and the image for administrator bootstrap below. Compose deploys
PostgreSQL plus the single hagency-server process. The Docker build uses one
compiler job and a lower optimization level for the large Palpo crate to reduce
peak memory; its crypto/HTTP/database dependencies retain full optimization.
Put the loopback-exposed
HTTP listener behind your HTTPS proxy. Preserve the public Host header and
forward `/`, `/api`, `/_matrix`, `/_palpo`, `/auth/`, `/.well-known/matrix` and `/healthz`
to port 8088. Browser/admin/native clients use the configured public origin;
The integrated Agent Appservice uses the derived internal origin. No separate
web-admin origin, port or relay URL needs configuration.

Compose mounts `config/docker/` at `/app/config/`. The entrypoint copies this
configuration tree into `/run/hagency/config/` (directories mode 0700, files mode
0600) before dropping to UID 10001. References between component files and
secret files inside this tree stay relative. The Rust process runs without root.
Generated production paths are absolute; use absolute paths for optional mounted
account configuration and token files, readable by UID 10001.

For bootstrap in Compose, after PostgreSQL is healthy:

```sh
docker compose run --rm --service-ports \
  -v "$PWD/secrets/admin-password:/app/bootstrap-password:ro" server \
  --config /app/config/hagency.toml --bootstrap-admin admin \
  --bootstrap-password-file /run/hagency/bootstrap-password
```

Stop that initial process after creating the account, then use
`docker compose up -d server`. For an existing initialized deployment,
`docker compose up -d --build` is sufficient. Bootstrap refuses an existing account. Do not add
a shared default administrator password to the image or config.

## Behavior retained in the Rust port

Palpo/Pasion generic users, Rooms/Spaces, media, reports, appservices, federation,
server actions/notices/notifications and identity administration continue through
their existing APIs and authority checks. New Hagency behavior is documented in
[Agent architecture](OPERATIONS.md) and [the capability list](WEB_ADMIN_PARITY.md).

Fleet/Hafleet, Engagement, resource allocation approvals, account-approval rooms,
old enrollment, Miniapp aliases and authority import are removed. No old mode or
data conversion remains. New domain/queue transactions use `hagency_agent_v1`.
Encrypted Room execution is deferred until client crypto is available; ciphertext
persistence alone does not execute an Agent. Offline local tools cannot be forcibly
undone after server lease revocation.

## Validation

```sh
just check-tools
just check-agents
just check-agents-postgres
cargo check --all-targets --locked
cargo test --locked -p hagency-server
cargo clippy --locked -p hagency-server --all-targets -- -D warnings
cargo check --locked -p hagency-frontend --target wasm32-unknown-unknown
```

The new PostgreSQL runner creates/removes a random isolated test database. Do not
point integration checks at an existing business database. Real integrated Pasion
PKCE/Matrix/new Agent transport checks use `scripts/test-agent-integration.py`;
set `HAGENCY_TEST_SERVER_BINARY` to this checkout's built executable. See
[the runnable testing guide](TESTING.md) for prerequisites, environment variables and cleanup. Generic Pasion/Compose checks
remain `tests/pasion-integration.mjs` and `tests/docker-smoke.mjs` and require their
dedicated test resources/databases. Removed Fleet contract scripts and their old
fixture server are not valid commands for this release.

BrowserAuth tests check live identity/admin role, cookie/CSRF/origin, logout,
closed BFF capabilities and unmounted old paths. Complete-host smoke must also
verify old endpoints return 404/410 and generic Palpo/Pasion APIs still work.
Historical evidence and limitations are recorded in [validation](VALIDATION.md).

Palpo embedding change: [upstream PR #505](https://github.com/palpo-im/palpo/pull/505).

### Embedded Pasion

Pasion now shares the Rust process and listener with Palpo and web-admin.
The embedding API and subpath support are proposed upstream in
[Pasion PR #102](https://github.com/meldry-com/pasion/pull/102):

| Component | URL |
| --- | --- |
| Existing web-admin | `/` |
| Matrix client/federation/admin APIs | Existing Matrix/Palpo paths |
| Pasion account UI | `/auth/` and `/auth/login` |
| OIDC discovery | `/auth/.well-known/openid-configuration` |
| OIDC issuer | `<public_origin>/auth/` |
| OAuth token / JWKS | `/auth/oauth2/token`, `/auth/oauth2/keys.json` |

No separate Pasion daemon, public IP, Node server or reverse-proxy rewrite is
needed. Pasion background tasks call Palpo's protected MAS APIs through the
same listener's loopback address. The host creates/persists OAuth signing keys,
cookie encryption key and the shared Matrix secret in mode-0600
`data/pasion-secrets.json`. Keep this file together with the database backups.

All three components use **separate databases** on one PostgreSQL service.
Compose creates `hagency` through `POSTGRES_DB`; `deploy/databases.sql` creates
`palpo` and `pasion` when initializing a fresh volume. PostgreSQL initialization
scripts do not rerun for existing volumes. See [the database split instructions](#upgrading-the-former-combined-database)
below before upgrading an old combined Hagency/Palpo database.

Enable embedding by referencing `pasion.toml` through `pasion_config` in
`hagency.toml`. Generated profiles include that reference. Configure the Pasion
DB in `[database].uri` and assets in `[hagency].resources_dir` in its own file.
Native email, SMS, account registration, clients, upstream OAuth providers, pool
limits, rate limits and branding settings live directly in `pasion.toml`.
Networking/issuer, Matrix connections, templates, storage and key ownership are
still derived by Hagency; those sections cannot override the host wiring. Native
database pool and TLS settings are retained. All three databases remain separate.

For native development, build Pasion's Dioxus WASM frontend/resources once:

```sh
just prepare-pasion
just dev --pasion-source /path/to/pasion --palpo-source /path/to/palpo
```

`just prepare-pasion` uses the pinned source by default; `--source` takes a local
checkout. It installs Rust's `wasm32-unknown-unknown` target if needed.
The Rust tool reuses `dx` 0.7.5 or fetches a matching, checksum-verified CLI into
`.run/tools/` without replacing a globally installed version.
The development watcher rebuilds edited Pasion backend/frontend code and
copies templates/translations/policies, then gracefully replaces the server.
No image rebuild is involved. Docker builds and packages those resources as
part of its image.

For local registration tests, enable `[account].password_registration_enabled`
and set the following native options in `config/dev/pasion.toml`:

```toml
[experimental]
fixed_verification_code = "123456"
[email.provider]
type = "blackhole"
[sms.provider]
type = "blackhole"
```

Email and SMS contact verification then store `123456` and skip outbound
verification notifications. Set `[account].password_registration_contact_required = false` to make
contact information optional at registration. Enter that code on the verification page; resend
once if a registration started before this setting was enabled. The watcher
reloads this file. Leave the fixed-code option unset outside local tests.

Pasion delegation is enabled by default in generated configurations. The host
wires Palpo discovery, token introspection and compatible Matrix password login
to the mounted Pasion service. `/auth/` provides the account center;
`/auth/register` is registration, and `/login` is the integrated console entry.
Registration uses Pasion's own access/registration policy. The old
`/account-request`, custom account-approval worker and menu are removed;
there is no native-auth compatibility mode.

Pasion does not implement legacy Matrix SSO redirects. OAuth clients use the
mounted issuer; legacy Matrix password clients use the delegated password flow.
See [the web-admin parity record](WEB_ADMIN_PARITY.md) for retained workflows
and intentional authentication changes.

Project manifests declare Apache-2.0 and the sole project license file is
`LICENSE`. `NOTICE` retains Palpo, Padmin and Pasion provenance and upstream
license references. Padmin source and Pasion dependencies retain their upstream
AGPL licenses; changing the manifests does not relicense that code.

### Upgrading embedded Palpo

The Palpo main update to `296f6bdb` includes the 2026-10-30 publication
migrations. Before starting the new binary, stop every old Hagency/Palpo process,
watcher and worker sharing its Matrix database. Take a consistent stopped backup
of all three databases and private files as described in [RECOVERY.md](RECOVERY.md).
Start one upgraded instance, wait for migrations and startup to finish, then start
the remaining upgraded instances and resume traffic. Keep all old writers stopped;
this update does not support a rolling deployment with mixed binary versions.

Clients must preserve opaque sync and pagination tokens. Old sync positions remain
accepted, but resuming a pre-upgrade sync boundary may require a full sync for
late-published history. Existing history cursors need not be discarded.
See the [upstream upgrade procedure](https://github.com/palpo-im/palpo/blob/296f6bdb55919f2bfef5fb7afc322b7c8fbde68d/docs/UPGRADING.md).

The updated outbound transport rejects active forward-proxy configuration and
proxy environment variables at startup. A configured PostgreSQL TLS requirement
also takes precedence over weaker `sslmode` values in the connection URL. Keep
these policies consistent with the deployment before starting the new binary.

### Upgrading the former combined database

This version does not migrate old Hagency document/Fleet/Engagement data.
Preserve Palpo/Pasion database URLs, Matrix server identity and signing/media
files. Configure a separate Hagency database for the new `hagency_agent_v1`
schema. No startup command renames, deletes or copies existing business tables.

An older combined database requires a separately reviewed backup and component
separation procedure before enabling this host; copying `hagency_admin_state`
into the new service is not a supported migration.

### Moving the previous single configuration into component files

For an existing three-database configuration, keep the database URLs, server
identity, key/media paths and authentication settings unchanged while splitting:

1. Put host settings into `hagency.toml`; replace its inline component sections
   with `palpo_config` and required `pasion_config` file references.
2. Move `[matrix]` into `palpo.toml` at the top level; remove the `matrix.` prefix
   from its subsection names, such as `[matrix.db]` → `[db]`.
3. Move `[pasion].database_url` to `[database].uri` in `pasion.toml`. Move
   `resources_dir`/`delegate_matrix_auth` into its `[hagency]` section and unwrap
   `[pasion.settings.*]` into the corresponding native sections.
4. Adjust relative paths to each new file location, or keep absolute paths;
   check with `hagency-server --config <path>/hagency.toml --check-config`.

The previous private `config.dev.toml`/`config.docker.toml` files are not rewritten
or imported automatically. Preserve them during the move. If their databases
are still combined, review component separation independently before using the new files.


## Member pages and administrator pages

The signed-in home `/hagency/home`, Projects `/hagency/projects` and My agents
`/hagency/agents` are available to ordinary members and administrators.
Server-wide user/room moderation, identity-provider administration, server
operations and the administrative dashboard live under `/admin`. Navigation and
direct access verify the signed-in account when the page loads, without periodic
polling. Refresh after a role change; a newly promoted administrator is asked
for the missing OAuth management authorization for the same account. A pathname
or client-side stored flag never grants administrative API access.
The account center and OAuth/OIDC endpoints are mounted at `/auth/`.

### Upgrade from the old auth mount

For an existing database, stop the old server and back up its databases and
state files. With the new binary and the same deployment configuration, run:

```sh
hagency-server --config config/dev/hagency.toml --migrate-auth-path
```

This one-way transaction accepts only the same server and public origin,
changes the Agent deployment/user issuer from `/_pasion/` to `/auth/`, retains
user IDs and Agent ownership, restores identity immutability, and revokes old
Agent sessions/devices. Run it once before starting the upgraded server;
repeating it is harmless. Fresh installations need no migration.
Upgrade Client/Desktop together and sign in again. Native account IDs and
local Agent ledger paths keep their existing storage namespace; a matching
legacy profile marker is updated only after its original scoped ledger is verified.
Old OAuth issuers are not accepted by the upgraded clients.
