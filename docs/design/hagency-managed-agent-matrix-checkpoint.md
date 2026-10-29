# Managed-agent Matrix membership checkpoint

Checkpoint2b keeps Matrix membership separate from native provider qualification.
The existing native schema76 provision record is runtime custody: `qualified`
permits subsequent call preparation. This workflow must never write that record
or manufacture its proof. Matrix verification remains a bounded observation;
shared agents stay suspended, without generic bindings or work/model support.

## Closed protocol and authority

Central schema10 adds a retained membership row per existing shared assignment
and append-only native-check challenges. A challenge freezes the assignment ID,
generation, canonical original reserve command/digest/receipt digest, native
child ID, exact existing puppet and project space, issued time and expiry. The
reserve already freezes manager, assigned project administrator, setup binding,
room, project/allocation/grant/resource/device identity. It never creates rooms.

Default-off `enable_agent_matrix_provisioning` gates fresh provisioning and new
`agent-matrix/poll`, `agent-matrix/{id}/recover`, `agent-matrix/{id}/result` routes.
A request-level read exposes membership separately from runtime. Recovery never
authorizes a new native check or extends the immutable challenge validity.
Only the same currently enrolled owner/device/generation can answer a fresh
check. Current generations may recover historical information but cannot use an
old-generation challenge to provision.

Native verification is a read-only canonical-ledger snapshot. It checks the
exact original successful reserve receipt, current child/parent/project/resource
and profile access fences, period/delegation expiry and device binding. It emits
a bounded eligible/refused observation, tied to the challenge digest and fixed
expiry. `observedLedgerRevision` is a snapshot revision, not a unique mutation
receipt and never changes capacity. No schema76 runtime authority is created.

Central admission of the response repeats live request, administrator, setup,
allocation, resource, owner and device/session checks in its serialized domain
transaction. The first exact response is retained. Replay cannot renew it;
expired challenges require a distinct new challenge and current native read.

## Matrix effects and cleanup

The central membership worker uses its existing namespace-confined appservice
credentials. It reuses the deterministic puppet already owned permanently by
the project manager. It checks authority before and after every Matrix await,
and records that effects may have begun before the first external mutation.
Identity verification, display-name update, invite and join are separate steps;
no combined helper may conceal intermediate awaits from the authority checks.

One state response per existing room supplies each observation: current manager
power/membership, assigned admin membership, private/unencrypted room, space link,
original setup marker and puppet membership must agree within those snapshots.
The final serialized check verifies the same assignment/challenge/current
authority before persisting `matrix_verified`. Public output always includes
`runtimeState: unsupported`, `workAvailable: false`.

Revocation permanently fences membership in the same transaction as the existing
agent cleanup obligation. Original puppet/room identity is retained. A timed-out
or late join is never considered definitely absent: cleanup repeatedly observes
and leaves that exact room, including after a prior absence observation, so a
late remote completion cannot resurrect membership. Expired native authority
blocks new joins; it does not free native capacity or qualify a runtime.

## Bounded implementation and acceptance

Server: new `management_agent_matrix{,_types,_schema,_tests}.rs/sql`, narrow
policy/config/module/migration/worker hooks and split Matrix-client calls. Native:
separate read-only canonical-child verification and transport; original reserve
and fence wire bytes are unchanged. Existing personal provision/binding paths
retain their behavior and shared bindings remain forbidden.

Acceptance includes real PostgreSQL transactions and loopback Matrix HTTP:
wrong roles/devices/generations, immutable replay and expiry, no native proof/no
join, every dependency revocation, cancellation while join is in flight, late
join cleanup, exact membership snapshots, no synthetic ready state, migration
rollback, personal isolation and native main/WAL unchanged during verification.
No live flags, provider calls, deployment, publishing or runtime constructors are
authorized by this checkpoint.

## Validated paired checkpoint (2026-10-16)

- Full isolated PostgreSQL/HTTP suite: **176 passed, 0 failed, 0 ignored**,
  164.98 seconds, `.run/managed-matrix-postgres-final.log`.
- `cargo clippy -p hagency-agent-service --all-targets -- -D warnings`: passed,
  `.run/managed-matrix-server-clippy-final.log`.
- `cargo build -p hagency-server --locked --offline`: passed,
  `.run/managed-matrix-server-build.log`; existing macOS compact-unwind linker
  warning remains.
- Native schema77 full store suite: **204 passed**, including five new read-only
  proof/unchanged-main-WAL/canonical/pin regressions. Four paired HTTP adapter
  tests pass. Native/store strict Clippy passes. See the client checkpoint doc.

Frozen server binary/source hashes and logs are in `.run/management-schema10-build/`.
Binary SHA-256: `17e3b3f81d67d331bddfd5eb264fac5e8cd7284f71d8413a4059059e1c42011f`.
The synthetic canonical fixture is `tests/fixtures/management-agent-matrix-v1.json`
under `crates/agent-service`; native has the byte-identical fixture under
`owner_resources/capacity/managed_agents`. No existing reserve/fence bytes changed.

These tests use real PostgreSQL, canonical SQLite and loopback Matrix HTTP,
including a delayed join raced against revoke. They do not claim deployed Matrix
membership, live native onboarding or a runnable provider. All activation flags
remain off/unadvertised; parent controls any later isolated/live activation.
