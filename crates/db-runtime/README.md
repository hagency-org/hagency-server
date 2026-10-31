# Database execution

`Database` is a cloneable handle backed by Horme 0.4 and a private Diesel async
deadpool provider. Creating/cloning a handle never checks out a connection. Use
`with_connection` for a database operation, and pass its connection through helpers
and transactions. Connections are released before results are published, including
when a completed future remains alive. HTTP and other external work belongs outside
the callback.

The runtime rejects same-task nested checkout of the same pool, including cloned
handles and join/select branches. A separately spawned task has its own task-local
scope; admission and deadlines bound that case rather than proving it impossible.
At most twice the pool size can be admitted, including waiters and cleanup. Checkout
is bounded to 5 seconds; the callback plus session reset to 30 seconds. PostgreSQL
statement, lock and idle-transaction timeouts also apply. Successful callbacks roll
back leftover transactions, release session advisory locks and reset session state.
Errors discard the session. Cancellation, deadline expiry and panic detach it,
issue PostgreSQL Cancel, attempt rollback and close it. Cleanup retains admission
and has its own deadline.

Agent business pools default to 8 connections **per Store/Domain/Transport/Inbox
handle family**; clones share their pool. `HAGENCY_AGENT_DB_POOL_SIZE` overrides
that per-pool size and must be a positive integer. The independent board pool has
4 connections. Budget the aggregate connections across all stores and replicas
against PostgreSQL's limit, plus dedicated infrastructure sessions.

The membership controller intentionally retains a dedicated unpooled session for
its leadership advisory lock. Crypto operations retain a separate serialized
session while their bounded HTTP authorization fence holds real rows. These do not
use business pool slots. Startup upgrades also use their own session, retain the
existing locks/version checks and execute the existing SQL files. Board migration
statements use a 120-second timeout with a 30-second lock timeout, independently of
request deadlines.

No schema or data rewrite is required by this change. Existing IDs, token hashes,
task versions, idempotency receipts, outbox states, ciphertext and attachment
storage remain in their current tables. The existing v6-to-v7 board migration
still backfills attachment metadata and removes already-deleted blob payloads;
the new populated-legacy-database regression verifies that behavior and idempotent
reopening. Do not replace deployed schemas with a fresh database or mark old
migrations as applied without running them.

Generate/check Rust definitions from the real startup/upgrade chain in a disposable
database (never a deployment database):

```powershell
$env:HAGENCY_SCHEMA_DATABASE_URL = 'postgres://user:password@localhost/schema_test'
python scripts/check-db-schema.py --write # omit --write to check in CI
```

The files in `crates/agent-service/src/schema/{agent,project}.rs` are Diesel CLI
output; do not edit them. Ordinary CRUD uses Diesel DSL and typed projections.
PostgreSQL CTEs, catalog checks, views and complex JSON projections retain raw SQL.
Bindings remain parameterized. SQL strings do not become compile-time checked
merely by placing them behind a Rust function or annotating a result type.

Run single-slot pool regressions with `DB_TEST_DATABASE_URL` pointing at a dedicated
test database:

```text
cargo test --locked -p hagency-db-runtime --features postgres-tests
```

Run agent regressions, including upgrade and concurrency tests, with a dedicated
`HAGENCY_AGENT_TEST_DATABASE_URL` and `--include-ignored --test-threads=1`. Some tests
create/drop their own isolated databases and require that permission. Put all local
build targets, PostgreSQL data, logs and temporary files under `D:/BuildCache`.
