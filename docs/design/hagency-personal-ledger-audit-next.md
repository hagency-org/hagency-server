# Personal agent ledger audit without changing evidence

Date: 2026-10-16. Status: native library primitive implemented and validated; no host route, live profile inspection, source census, migration or adoption authorization.

This complements `hagency-client/docs/management-ledger-reconciliation-next.md` and its existing `ProfileLedgerAudit`. It does not extend the server protocol or advertise allocation support.

## Existing ownership and accounting

The personal database is `owned-agent-owners/owner_<profile>/agent-local.sqlite`. The profile digest binds authenticated server origin, issuer namespace, subject and owner. Its `identity` and `profile_identity` tables are permanent owner/profile bindings, not evidence of provider-account identity or proof that another copy cannot execute.

There is no personal equivalent of the single `DomainStore` worker. `hagency-agent-local::Ledger` holds one `rusqlite::Connection`. Runtime binding tasks keep their own handles; personal administration routes open short-lived handles. Multiple connections may therefore operate on one profile database.

The existing opening helpers are unsuitable for an audit:

- `console/owned_agents.rs::ledger_path` creates directories/files, initializes the scoped ledger and may rewrite an issuer identity marker.
- `Ledger::open` uses `BEGIN IMMEDIATE`, creates or upgrades tables, and changes connection configuration. `open_scoped` can initialize the permanent profile binding.
- Runtime startup probes a provider, acquires a server execution lease and calls `recover_agent_executions`. An audit must not start a runtime to obtain its connection.
- `recover_interrupted`, `recover_local_executions` and `recover_agent_executions` change pending calls and executions. Recovery can settle calls with `unreported-zero-v1`, release their estimated holds and write cumulative-usage gap markers. That zero is provisional missing usage, not evidence of no consumption. A later genuine receipt can replace it.

`accounts` records the same call across agent, room and requester policy scopes and period windows. Summing all `spent` or `held` rows would multiply liabilities. A bounded inventory must not present that sum as provider usage, available capacity or a migration charge.

## Smallest implementation

Add a pure method on an **already-open** handle:

```rust
pub fn reconciliation_audit(
    &mut self,
    authority: &PersonalAuditAuthority,
) -> Result<PersonalLedgerAudit>;
```

`PersonalAuditAuthority` is non-deserializable and contains a native-derived owner, exact authenticated profile digest, finite monotonic deadline and a live native callback/retirement fence. The callback cannot block or perform I/O. No script supplies the owner, digest, path, provider identifier or approval flag. Construction is host-only by convention/type visibility, matching other trusted native proof constructors.

Check authority before entering the query, after waiting for any host queue, during bounded table scans, and before returning. Validate the database's current application ID, exact supported schema shape, single owner row and exact permanent profile row. Reject unscoped, wrong-profile or unsupported databases; never initialize them. A closed authority returns an authorization failure, not an empty audit.

Use one deferred read transaction on that same connection. Issue only fixed read queries; no schema upgrades, recovery, cleanup, checkpoint, pragma writes, credential access, provider observation or path resolution. An existing transaction should produce a bounded failure rather than silently nesting or committing the caller's work. The snapshot includes committed WAL frames visible to this connection. It is an observation, not a promise that other connections cannot write afterward.

Do not add a source opener or a mini-app route in this slice. The current `NativeManagement` handle owns only the domain worker, so it cannot truthfully complete the personal audit yet. Keep `PersonalLedgerNotInspected` in its existing result until the separate host adapter actually obtains a personal snapshot.

## Safe result contract

Use serialize-only closed Rust types; accepting a JSON copy can never grant audit authority. Suggested public shape:

```text
PersonalLedgerAudit {
  version: 1,
  schemaVersion: 1,
  scope: "current_profile_personal_snapshot",
  adoption: "not_assessed",
  observedAtMs,
  tablesObserved,
  countsComplete,
  history: PersonalLedgerHistory,
  blockers: [PersonalAuditBlocker]
}
AuditCount { observed: u32, capped: bool }
```

`PersonalLedgerHistory` contains bounded counts only:

| Category | Counts and interpretation |
| --- | --- |
| Calls | Total observed; pending; unknown; settled; settled with missing/malformed usage; unreported-zero; reported usage records; unknown accounting modes. Categories may overlap and are not additive liabilities. |
| Execution inbox | Total observed; received/acknowledged; prepared/running; reply-ready; terminal; unknown; unexpected state. A failed/terminal label alone does not prove no provider charge. |
| Accounting | Account rows; rows with positive held; rows with positive spent; invalid numeric values. No sum across policy layers/windows. |
| Usage continuity | Codex usage and external-session rows; cumulative counters explicitly `null`; missing/oversized/malformed counters. Do not return session identifiers or raw counters. |
| Private history | Binding, policy, approval, context, model-profile and dispatch row counts. No raw scopes, room IDs, requesters, argument payloads, credential references or workspace paths. |
| Other | Populated unclassified table count and bounded row count. Table names remain private. |

The known current usage encodings include `unreported-zero-v1`, `no-provider-call-v1`, `codex-app-server-0.160/cumulative-input-output-v1`, `claude-reported-v1` and `octos-reported-v1`. An unknown encoding is not silently treated as reported usage. Merely naming `no-provider-call-v1` is not enough to certify an unused source; any later importer must validate its original execution/digest evidence. This initial audit never makes that certification.

Always include `provider_mapping_not_established`, `source_census_not_established` and `migration_not_assessed`. Add bounded enum blockers for pending calls, uncertain executions, missing/unreported usage, retained accounting, execution history, private history, unknown history, unsupported accounting and observation limits. Even a complete empty snapshot remains `adoption:not_assessed`.

No provider-level held/remaining total, adoption permission, migration digest, source fence proof or physical source identity belongs in this display projection. Future private migration custody must retain original records separately.

## Bounds and schema handling

Bound discovery to 64 tables and row observations to 10,000 per table plus one overflow sentinel. Tables/columns needed by supported queries must be ordinary tables with the expected shape; reject unexpected views/virtual tables in required positions. Discover oversized names without loading them; reject or return an explicit incomplete result. Unknown ordinary populated tables remain blocking.

Scan at most the bounded row prefix before classifying state; do not use an unindexed filtered count that traverses unlimited rows to find 10,000 matches. SQL expressions must bound selected text before returning it. Usage JSON may be inspected only below a small fixed byte limit (for example 2 KiB), then parsed using the existing closed `Usage` type and integer/subcategory checks. Never select private envelopes, prompts, model profiles, workspace paths or approval proposals. Oversized, malformed, negative or non-exact quantities add a blocker or fail the audit; they never become zero. Query budgets and a finite authority deadline bound availability; no unbounded `SUM` is needed.

The library's `user_version=1` includes additive schema changes, so checking that number alone is insufficient. Validate the required table/column set for this audit and classify additional populated history conservatively. The audit must never repair missing columns.

## Native queue boundary

The eventual adapter must capture the current SDK-backed profile/device and resource-read permission, then submit a bounded typed audit job to an existing owner of a live `Ledger`. The owner rechecks the captured live fence before and after the query; the native caller rechecks the current SDK account and immutable device/profile identity after the await before exposing any result.

The runtime's existing `BindingWork` channel starts provider work and is not an audit queue. Do not reuse it by fabricating a binding/start configuration. A separate read-only control job can be added later to an already-running binding actor; it must be serviced between ledger operations with a bounded queue/deadline. It observes the shared database through that actor's actual handle, without claiming all writers are quiesced. If no suitable handle exists or the actor is occupied, return `personal_ledger_unavailable`/busy, preserving incomplete status. Do not start/recover an agent, open a new SQLite connection, stop a runtime or adopt a copied database as a fallback.

Offline inspection and a general persistent personal-ledger worker require separate review. A read-only WAL opener can still touch shared-memory reader marks; `immutable=1` can omit live WAL commits. Neither is needed for the first already-owned-handle primitive.

## Acceptance before host integration

Use only newly created synthetic test ledgers; never open a real profile for these tests.

1. Seed pending/unknown calls, positive account holds, prepared/running/unknown inbox records and pending approvals. Audit preserves every state/value and reports the corresponding blockers.
2. Apply historical recovery deliberately during fixture setup, then audit the resulting settled `unreported-zero-v1` calls and `null` gap markers. They remain blocking despite zero account holds. Distinguish a genuine reported zero without claiming it authorizes adoption.
3. Seed one call charged to three policy layers; demonstrate that the projection does not report triple provider usage or refunded capacity.
4. Enable WAL during fixture setup, leave committed frames uncheckpointed, hold all fixture connections open, snapshot main/WAL bytes, schema and relevant rows, then audit. Verify committed frames are observed and main/WAL/schema/rows are unchanged. Do not claim byte-for-byte immutability of `-shm` reader bookkeeping.
5. Wrong owner/profile, missing profile binding, unsupported schema/required shape and an active caller transaction fail without modifying evidence. Unknown populated tables and malformed/oversized usage block.
6. Table/row/text limits return explicit capped/incomplete observations or a bounded error. Authority retirement/expiry before or during the query exposes no result; a future queue test retires authority while queued and proves no query runs afterward.
7. Assert safe serialized output excludes fixture credentials, paths, raw prompts, session/call/room/requester IDs and private table names. Keep output bounded and no input deserialization implementation.

Passing these tests completes only the personal snapshot primitive. Host queue wiring, offline inspection, source census, provider mapping, writer fences, migration and allocator activation remain separate work.

## Validation checkpoint

Implemented in `hagency-client/native/hagency-agent-local/src/reconciliation.rs`, with synthetic tests in `reconciliation/tests.rs`. The only existing production-file change is its module declaration in the crate's `lib.rs`. No SQLite opener, schema migration, runtime start, native management operation or mini-app route was added.

The seven focused audit tests pass. The full `hagency-agent-local` suite passes 85 unit tests (three existing opt-in tests remain ignored) and eight runtime fixture tests; strict all-target Clippy with warnings denied, formatting and diff checks pass. Commands used `CARGO_BUILD_JOBS=2 cargo test --locked --offline -p hagency-agent-local -- --test-threads=1` and `cargo clippy --locked --offline -p hagency-agent-local --all-targets -- -D warnings`. Logs are `.run/personal-ledger-audit-{tests-1,all-tests-1,clippy-1}.log` in the client checkout.

The current native `ProfileLedgerAudit` still correctly reports `PersonalLedgerNotInspected`: this new primitive is not wired to a live already-owned personal connection. No real personal database was opened, copied, recovered or migrated for validation.
