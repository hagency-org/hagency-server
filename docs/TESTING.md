# Local testing

[中文](TESTING.zh-CN.md) · [Documentation](README.md)

Run commands from the server checkout. Use its pinned Rust toolchain, Docker
Compose, Python 3 for `scripts/*.py`, and the assets/configuration prepared by
[the setup guide](guide.md#development). Node.js is only needed for `tests/*.mjs`.
A successful build, HTTP health, transport readiness and a real model response
are different checks; record which one you ran.

## Static and Rust checks

```sh
just check-tools
just check-agents
python3 scripts/validate-agent-openapi.py
cargo fmt --all -- --check
cargo test --locked -p hagency-server
cargo clippy --locked -p hagency-server --all-targets -- -D warnings
cargo check --locked -p hagency-frontend --target wasm32-unknown-unknown
```

`check-agents` runs the Agent library tests and strict Clippy. Database tests
marked ignored are not coverage unless run with the PostgreSQL runner below.
OpenAPI validation checks source/contract drift; it does not replace HTTP tests.

## Isolated PostgreSQL regressions

```sh
just db-up
just check-agents-postgres
```

The runner reads credentials from the Compose PostgreSQL container, creates a
random `hagency_agent_test_*` database, runs Agent tests including ignored tests,
and drops that database in `finally`. It uses Cargo `--offline`, so fetch/build
locked dependencies first on a fresh checkout. It does not migrate the application
Agent database. The selected PostgreSQL service must be disposable test infrastructure.

For a different Compose project or port, set both variables to its actual values:

```sh
HAGENCY_TEST_POSTGRES_CONTAINER=my-test-postgres-1 \
HAGENCY_TEST_POSTGRES_PORT=55439 just check-agents-postgres
```

If interrupted externally, inspect remaining databases before removing only the
fixture databases from that run. Do not delete the Compose data volume to clean tests.

## Actual embedded Pasion/Palpo integration

Prepare a local dev profile and assets once using the root README, then:

```sh
cargo build --locked -p hagency-server --bin hagency-server
HAGENCY_TEST_SERVER_BINARY="$PWD/target/debug/hagency-server" \
HAGENCY_TEST_NATIVE_PKCE=1 python3 scripts/test-agent-integration.py
```

Always set `HAGENCY_TEST_SERVER_BINARY`: the script's current default points to
a developer's external checkout. It uses environment variables, not a CLI
argument parser. The script reads `config/dev/palpo.toml` and `pasion.toml` as
fixture templates, overrides databases/identity/storage, starts a separate backend
on a random loopback port and bootstraps a disposable owner. It uses the prepared
`resources/frontend/public` and `resources/pasion` directories.

| Variable | Meaning |
| --- | --- |
| `HAGENCY_TEST_SERVER_BINARY` | Absolute path to the binary built from the checkout being tested |
| `HAGENCY_TEST_POSTGRES_CONTAINER` | Default `hagency-server-postgres-1` |
| `HAGENCY_TEST_POSTGRES_PORT` | Default `55438`, host loopback port |
| `HAGENCY_TEST_PASION_RESOURCES` / `HAGENCY_TEST_FRONTEND_RESOURCES` | Optional absolute asset directories from the current build, to keep a running deployment untouched |
| `HAGENCY_TEST_HOLD_BROWSER=1` | Wait for real UI checks using the private browser fixture; finish with private `browser-done.json` containing `{"passed":true}` |
| `HAGENCY_TEST_NATIVE_PKCE=1` | Real native Pasion DCR/PKCE, Agent and Matrix roundtrip |
| `HAGENCY_TEST_BACKUP_RESTORE=1` | Also enables PKCE and the stopped three-database/file restoration rehearsal |
| `HAGENCY_TEST_HOLD_CLIENT=1` / `HAGENCY_TEST_HOLD_PKCE=1` | Interactive fixtures; intentionally wait for an operator, not unattended CI |

Run the restoration rehearsal with the same explicit binary:

```sh
HAGENCY_TEST_SERVER_BINARY="$PWD/target/debug/hagency-server" \
HAGENCY_TEST_BACKUP_RESTORE=1 python3 scripts/test-agent-integration.py
```

The script creates random `hagency_smoke_*` databases, attempts cleanup for every
original/restored database, stops its own process and removes private fixtures on
success. Failure retains private logs/configuration for diagnosis; those files can
contain credentials. These checks exercise actual authentication, provisioning,
transport and recovery without invoking Codex or a tool runner. See the
[restore boundaries](BACKUP_RESTORE_VALIDATION.md).

## Desktop/client acceptance

Use [local deployment diagnostics](LOCAL_DEPLOYMENT.md), then the
[Desktop development guide](../../hagency-desktop/docs/local-development.md) or
[owner-client guide](../../hagency-client/docs/local-development.md). Verify browser
login, Projects/Rooms and owner permissions before explicitly starting an Agent.
Inference requires its own provider account and consumes its configured resources.
A Matrix reply produced by a protocol fixture does not prove real model execution.

Historical test counts and earlier product results live in [VALIDATION.md](VALIDATION.md);
they are evidence for the recorded revision, not a current test plan.
