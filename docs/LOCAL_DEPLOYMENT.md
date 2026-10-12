# Local deployment and connection diagnostics

[中文](LOCAL_DEPLOYMENT.zh-CN.md) · [Documentation](README.md)

Use [the root README](../README.md) for first bootstrap and [the configuration
guide](guide.md) for the full setup. This page covers subsequent starts and the
connection from browsers, Desktop and the owner client. Commands run in this repo.

## Choose one server mode

| Mode | Configuration | Listener / database | Start |
| --- | --- | --- | --- |
| Source development | `config/dev/*.toml` | `127.0.0.1:8088` / PostgreSQL `127.0.0.1:55438` | `just db-up`, then `just dev` |
| Compose deployment | `config/docker/*.toml`, `.env` | Published `127.0.0.1:8088` / container `postgres:5432` | `just docker-up` |
| Protocol test fixture | Generated private temporary configs | Random loopback port / random databases | [Testing guide](TESTING.md) |

Source dev and Compose server share the default host port. Run only the intended
server, and one writer per set of databases. If Docker initialization generated
`.env` with a new database password, compare the private dev URLs with the actual
PostgreSQL credentials: an existing database volume retains its original password.
Changing `.env` does not change that password. Initialization SQL only runs on new
volumes; do not use `down -v` to solve a missing database or password mismatch.

```sh
just check-config config/dev/hagency.toml
curl --fail --silent --show-error http://127.0.0.1:8088/healthz
curl --fail --silent --show-error http://127.0.0.1:8088/readyz
curl --fail --silent --show-error http://127.0.0.1:8088/api/hagency/v1/discovery
curl --fail --silent --show-error http://127.0.0.1:8088/_matrix/client/versions
curl --fail --silent --show-error http://127.0.0.1:8088/_matrix/client/v1/auth_metadata
curl --fail --silent --show-error http://127.0.0.1:8088/auth/.well-known/openid-configuration
```

`--check-config` reads referenced configuration and requires delegated Pasion;
it does not connect to PostgreSQL or verify TLS. `/healthz` is listener health.
`/readyz` requires the integrated Appservice's real startup send/receive probe;
This is a startup latch, not a continuous transport-health probe.
200 health alone is insufficient for Agent creation. Readiness also does not
prove a human's Matrix identity has completed asynchronous provisioning.

Open `/login` for the web console. Desktop/client sign-in uses the **server origin**,
for example `http://127.0.0.1:8088`, not `/login` or `/auth/`. HTTP development
OAuth is limited to loopback IPs; a local hostname needs trusted HTTPS. No Fleet
registration, `[fleet_access]`, per-user Appservice secret or credential import is needed.

## HTTPS and container access

For a new HTTPS deployment, generate Docker config with the final public origin
and stable Matrix server name as in the README. Proxy every path to port 8088,
preserving the public Host and avoiding subpath rewrites. The issuer must be
`<public_origin>/auth/`; discovery, token, callback and Matrix metadata must agree.
Keep an existing `server_name` and signing keys when reusing a database.

A private hostname such as `hagency.local` must resolve and pass certificate
validation for the browser, native client **and the server container**. Host
`127.0.0.1` is not container loopback. `host.docker.internal` or a Compose
`extra_hosts` entry may be needed for the container to reach the host proxy.
Trust the issuing local CA in all relevant trust stores; do not disable TLS checks.
The proxy must listen on the interface selected by that route. A host-only IPv6
success does not establish container IPv4 access.

```sh
curl -4 --fail --silent --show-error https://hagency.local/auth/.well-known/openid-configuration
curl -6 --fail --silent --show-error https://hagency.local/auth/.well-known/openid-configuration
docker compose exec server curl --fail --silent --show-error \
  https://hagency.local/auth/.well-known/openid-configuration
docker compose exec server curl --fail --silent --show-error \
  https://hagency.local/_matrix/client/v1/auth_metadata
```

Only test IPv6 where configured. For a proxy in another container/network, use
that network's actual route instead. If using a host LAN IP, document it in private
local setup notes and update the proxy bind and container mapping when it changes.
An unrelated service on IPv4 port 443 may serve another certificate while IPv6
works. Diagnose address/port ownership before changing other projects' services.

## Deployment lifecycle

```sh
docker compose ps
docker compose logs --tail 100 server
# After editing host-mounted config, recreate the server runtime copy.
docker compose up -d --force-recreate server
# After source changes, build the image and replace the server.
just docker-build
docker compose up -d server
# Stop while retaining both named data volumes.
just docker-down
```

The entrypoint copies `config/docker/` into protected `/run/hagency/config/` and
runs as UID 10001. Editing the mounted files does not update that copy until a
restart/recreation. Symlinks in this tree are refused. Referenced files must be
available/readable inside the container, not only on the host.

Before upgrades, save the source/image revision and follow [RECOVERY.md](RECOVERY.md)
for a consistent stopped backup of all three databases, configs, keys and media.
The `database` and `server-data` volumes both matter. Restart without bootstrap
flags; initialization/bootstrap are first-install operations. Old Fleet business
data has no automatic import. The guide's former combined-database split applies
to component storage, not revival of retired Hagency business data.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Address already in use | Existing source watcher, Compose server or proxy on the same interface/port |
| Database missing/password rejected | Volume initialization history and all three private connection URLs |
| Empty page/assets 404 | Run both resource preparation commands; check `public_dir` and Pasion resources |
| `/healthz` 200, `/readyz` 503 | Server logs, internal Appservice registration/send/receive; do not bypass readiness |
| Matrix `auth_metadata` 400 or issuer fetch fails | Server/container DNS, TLS trust, IPv4/IPv6 and public issuer reachability |
| Login succeeds, Agent operation denied | Current account, independent Space/Room membership, creation policy and service pause |
| Binding suspended after owner/member loss | Restore actual authorization and explicitly re-adopt/resume where allowed; old dispatches remain fenced |
| Model never starts | Local provider/model/limits, active binding and explicit scope start; server startup is not inference |
