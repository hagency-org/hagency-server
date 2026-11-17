# Managed agent stage 3: first production turn

Date: 2026-10-16. Status: checkpoint 1 implemented, with validation recorded below; remaining checkpoints are a proposed integration scope. No capability, deployment, provider call or new execution authority is enabled by this document. Central admission is schema 8 and the separate executor delivery checkpoint is schema 9; the native child ledger is schema 75 and its unwired custody primitives are schema 76. Their tests establish storage and protocol behavior, not a working managed agent.

## Checkpoint 1: server delivery and receipt ingestion

`ManagementConfig.enable_agent_executor` is a separate host-only, default-false flag. It is not advertised in session features. All agent-executor reads/writes require this flag; fresh reservations additionally require current admission, setup and reservation policy. Disabling issuance does not revoke a valid native hold. No shared work lease, model request, agent-room binding, Matrix provisioning or native executor adapter is enabled.

Routes below use the existing authenticated device grant under `/hagency/api/v1/management/`:

| Route | Closed request and result |
| --- | --- |
| `POST agent-executor/poll` | Existing `ExecutorPollInput`; optional one `AgentExecutorDelivery`. Exact owner/device only; cleanup first. A reserve pins the original enrolled generation. |
| `POST agent-executor/{commandId}/recover` | Existing `ExecutorRecoverInput` with exact digest; original command and optional exact recorded receipt. **Always `executionAllowed=false`**; a finite token permits upload of already-existing evidence. |
| `POST agent-executor/{commandId}/result` | `AgentExecutorResultInput {commandDigest, token, receipt}`; immutable acknowledgement plus current optional pending assignment. First upload requires an unexpired ticket; exact stored-receipt replay cannot replace evidence. |
| `GET agent-requests/{requestId}/assignment` | Current scoped `PendingManagedAssignment`, or `null`. Lifecycle is `pending_native`/`fenced`, generation is fixed at one, and `executionAvailable` is always false. Manager, assigned-admin visibility and exact enrolled executor visibility remain separate. |
| `POST agent-requests/{requestId}/revoke` | Existing revisioned `RevokeInput`; only the current active administrator assigned to this project. Manager cancellation remains the existing `/cancel` operation. Neither coordinator nor server-admin status alone confers agent approval/revocation. |

The queue references the immutable schema-8 intent instead of rewriting its payload. `agent.reserve` uses the unchanged `management-agent-admission-v1.json` command/receipt. Cleanup uses `management-agent-fence-v1.json`: a distinct closed `agent.fence` command containing the exact original reserve command, its digest, and the queue's fixed first-fence timestamp. Its native implementation must persist a semantic tombstone over original request/agent/project/grant/period identity before returning `absent_fenced`; an absent database lookup is insufficient. This native adapter is not implemented by the server checkpoint.

Cancellation, assigned-admin revoke, resource/parent withdrawal, project/setup disable, account/device/session retirement and stale authority fence the intent and enqueue one cleanup obligation transactionally. Polling also sweeps a bounded page of expired/stale intents, including already-acknowledged reservations. New reserve delivery never resumes a fenced intent. Current device generation may recover/upload/clean up the original installation's history; it cannot use a historical delivery as fresh reserve authority.

A valid reserved receipt creates the permanent manager-owned agent identity in `suspended` state and its `pending_native` assignment in one transaction. Composite SQL FKs keep the manager ownership and resource executor ownership distinct. Shared agents retain `execution_device_id=NULL`; all generic personal bind/device/identity paths reject them, and SQL prevents a shared binding or active identity. A receipt arriving after cancellation is retained without creating an identity. No receipt means no identity and no central claim that native capacity was reserved.

Both original reserve and cleanup receipts remain immutable. `retainedTokens=null`, `processState=unknown` or incomplete usage is unresolved evidence, not zero. A recorded cleanup command means that its original fence acknowledgement was saved; it does not establish process termination, billing completeness or released capacity. A later distinct native cleanup/settlement observation protocol is still required. The server does not subtract an unknown fence from the retained original reservation. Cross-command ledger revision equality is rejected between agent, project, parent and grant evidence in both arrival orders; receipt replay preserves original bytes.

Migration 8→9 backfills existing immutable reserve/fence obligations, keeps existing personal agents and their owner/device constraints, and rolls back atomically on failure. No running backend has been upgraded by this checkpoint.

Validated on 2026-10-16 with `CARGO_BUILD_JOBS=2`, the repository build-tools path and `.run/libpq/lib`:

- `HAGENCY_TEST_POSTGRES_CONTAINER=hagency-latest-test-postgres-1 python3 scripts/test-agent-service-postgres.py`: **169 passed, 0 failed, 0 ignored**, 92.80 seconds; `.run/agent-executor-postgres-final-3.log`. This runs actual PostgreSQL/HTTP and existing personal transport/worker regressions in a disposable database. An intermediate downgrade fixture left the newly added scope index behind; its test-only teardown was corrected and the complete suite rerun.
- `cargo clippy -p hagency-agent-service --all-targets -- -D warnings`: passed; `.run/agent-executor-clippy-final-2.log`.
- `cargo build -p hagency-server --locked --offline`: passed; `.run/agent-executor-server-build.log`. The macOS debug link emits its existing large `__eh_frame` compact-unwind warning.
- Canonical reserve vectors remain unchanged. Fence command SHA-256 is `be3dbf9884b959b4729256dad25bbb6749f5be284079054398e84bda184c7e3b`; fence receipt SHA-256 is `4ac6718705cdd1dfbfbd084bb9aebf0737585f260738e34f0b173c317f4f7e20`. Rust canonical validation and independent sorted-JSON SHA-256 agree.

Frozen artifact/source checkpoint: `.run/management-schema9-build/`. This server-only checkpoint does not claim native transport, actual Matrix agent provisioning, provider execution or full ADR E2E acceptance. The earlier native schema76 **181/181** store suite remains a separate release8 storage checkpoint.

## Smallest complete outcome

An approved request is delivered to its exact enrolled executor. A durable native child reservation permits creation of the permanent manager-owned Matrix identity. The enrolled machine qualifies the existing owner-consented Codex account and tools-disabled runtime. The real project room admits the puppet. One manager mention produces one bounded Codex turn, durable observed usage, and one recoverable Matrix reply. Manager stop, assigned-project-admin revoke, resource withdrawal and native shutdown fence new work and stop the retained process. Missing usage or uncertain cleanup remains charged.

This first execution path has one fixed assigned executor, one verified plaintext project discussion room, one assignment generation, one call at a time, and a fresh ephemeral Codex thread per call. It has no host tools, workspace attachment, MCP, web-search tool, encrypted transport, owner DM, extra rooms, live reassignment or resume-after-stop. A current short lease bounds each call; it is not stretched to accommodate a long model turn. These limits must be explicit capability metadata. Tools/coding parity, conversation-history assembly, top-up, resume, reassignment and renewal remain named follow-ons, not implied support. Existing personal agents retain their current behavior.

The administrator assigned to the project alone approves agent creation, including when also the manager. The coordinator's earlier project capacity approval is sufficient: there is no second coordinator/resource-owner agent vote. Runtime/account/workspace consent is a different boundary from business approval.

## Verified source boundaries

| Existing implementation | Reuse and required change |
| --- | --- |
| Server `management_agents.rs::decide_managed_agent` | Freezes `ManagedAgentReserveCommand` and inserts `management_agent_intents` atomically. Schema 9 adds a separate default-off executor queue and exact receipt ingestion. `management_agent_outbox` remains notification references, **not an executor queue**. |
| Server `management_agent_types.rs` and `tests/fixtures/management-agent-admission-v1.json` | Reuse exact existing reserve command and reservation receipt. Preserve historical bytes/digests. Add separate delivery/provision/stop/stats contracts rather than altering v1 canonical meaning. |
| Server `execution_devices.rs::require_assigned`; `transport.rs::{agent_active,lease,acquire_lease,dispatch,claim_reply}` | These require `p.user_id` to equal the permanent human owner and that owner's execution device. They cannot authorize a resource owner's device. `transport_schema.sql::validate_transport_scope` independently enforces that ownership. Do not spoof a manager Principal or broaden an SQL owner predicate. |
| Server `transport_history.rs`, `processing.rs`, `workers.rs::{route,send,send_processing}` | Reuse history witness, exact dispatch/execution identity, original Matrix transaction ID and uncertain-send reconciliation under a distinct verified managed authority. Worker claim/send needs current managed assignment checks even though no executor HTTP request is present. |
| Server `domain.rs::{create_agent,identity_provisioning_candidates,confirm_identity_provisioned,verify_provisioning_trusted}` and membership loop in `workers.rs` | Reuse real puppet registration/invite/join verification below their personal entry points. A shared identity must be created by an internal native-receipt transaction, with manager as owner and `execution_device_id=NULL`. Managed provisioning candidates and confirmations require current request/allocation/assignment authority. |
| Native `console/native/allocation_transport{.rs,/executor.rs}` | Reuse authenticated profile/device pinning, fresh poll vs historical recovery distinction, exact command/receipt digest checks, bounded upload tickets and post-I/O checks. Existing operations cover project/parent commands, not agents. |
| Native `console/owned_runtime{.rs,/service.rs}` and `console/device_execution.rs` | Useful behavior reference for supervisor cancellation, history, ACK/start/reply recovery. They are personal-agent services using `agent-local.sqlite`, personal owner leases and personal startup consent; do not route a managed agent into them. |
| Native `hagency-execution::{FactoryRuntime,Host}` | Legacy `OwnedProvisionScope`/`OwnedDispatchScope` requires real Registration, engagement/project FKs and runtime-home authority. It is not a managed-agent adapter. No fabricated legacy registration, owner room, engagement or RunnerCapability. |
| Native `hagency-runtime::owned::OwnedSession` and `codex::session::SessionDriver` | Reusable retained process ownership, scoped thread/turn validation, interruption, bounded stop, and non-deserializable `ObservationSource`/`Observation`. These carry runtime evidence, not project authority. |
| Native `hagency-agent-local::codex::{Profile,Session}` | Sealed launch/configuration checks are useful extraction candidates. `Session::open_context/run_with_started` is tied to the personal Ledger and its intentional unreported-zero handling; it cannot settle managed calls. `Settings::read_only` in the other runtime is insufficient to promise no tools. |
| Native `console/owner_provider/identity.rs::verify` | A real authenticated quota subject is available. Current proof comes from a separate metadata process; actual execution must verify the selected account on its retained inference process and match the canonical resource seat, not merely reuse an earlier path/reference. |
| Native `hagency-execution/src/usage{.rs,/capture.rs}` | Reuse normalization/source-sequence rules below `UsageRun::bind`; the latter requires legacy owned-dispatch authority. Its comment correctly says terminal protocol evidence does not by itself prove all usage arrived. |
| Native `owner_resources/capacity/managed_agents/custody` | Already enforces exact original reserve/project/grant, provider seat, permanent fences, single issue, unknown liability and evidence-only late cleanup. Authority/observation constructors remain fixture-only. Production construction and actual effect gates are still required. |

## Central implementation contract

Keep the management flags off until every shared execution boundary below is implemented and tested. Checkpoint 1 uses central schema 9; further migrations must be reserved separately.

1. Add a separate immutable `management_agent_executor_commands` record keyed by the existing reserve intent ID. Store exact command/digest, delivery kind, exact executor owner/device/original generation, finite token hash/deadline, original native receipt/digest and delivery state. Reference the existing admission intent; do not rewrite it or repurpose notification outbox rows. Reserve freshness requires current approved request, assigned-admin decision revision, unchanged live setup/allocation/grant/resource and enrolled generation. Cancellation atomically fences the reserve and queues one logical cleanup command even if reservation outcome is unknown.
2. Add immutable `management_shared_assignments`: assignment ID, proposed agent ID, permanent manager user, request/decision, domain project/room, setup binding and authority revisions, exact project reservation receipt, exact native agent child and reserve receipt digest, resource owner, device/enrollment/generation, resource/grant/period, initial assignment generation and lifecycle state. Use separate composite FKs for `(agent_id,manager_user_id)` and `(executor_device_id,executor_owner_user_id)`. Validate every cross-reference in SQL. Initial generation is fixed; no implicit takeover/reassignment route.
3. A successful native reservation receipt creates the `agents` row and assignment atomically only if still eligible. Replace the admission-only blanket shared-identity rejection with exact receipt/assignment validation, retaining immutable `execution_kind` and the original personal device-owner FK. If cancellation won, retain the receipt/hold, enqueue cleanup and create no runnable identity. A stale receipt never restores readiness.
4. Keep personal `execution_leases` and its owner-device constraint intact. Add `management_shared_leases(assignment_id,agent_id,generation,executor_owner_user_id,device_id,device_generation,epoch,expires_at_ms)` with explicit manager/assignment association. Add nullable managed-assignment snapshots to shared events, replies and processing intents; personal rows require these to be null. Immutable execution snapshots stay fixed through revocation. A crate-private `ExecutionAuthority::{Personal,Managed}` is constructed only by the appropriate authenticated route or trusted worker lookup; common transitions consume it, never a caller-supplied authority tag.
5. Add original provisioning/stop receipts and bounded latest native usage projections tied to assignment and global ledger revision. Do not infer active status from the reservation receipt. Actual identity, native qualification and Matrix membership must all be ready. Append observations; lower revisions cannot replace newer balances, and equality across different native mutations is rejected.

Proposed closed routes under `/hagency/api/v1/management/`:

```text
POST agent-executor/poll                 {after?}
POST agent-executor/{command}/recover    {commandDigest}
POST agent-executor/{command}/result     {commandDigest,token,receipt}
GET  agent-executor/assignments          bounded current metadata, no prompts
POST agent-assignments/{id}/provision    {commandId,generation,lease,receipt}
POST agent-assignments/{id}/history      {generation,cursor?,snapshot?}
POST agent-assignments/{id}/leases/acquire {generation,historySnapshot,ttlMs}
POST agent-assignments/{id}/leases/renew   {lease,ttlMs}
POST agent-assignments/{id}/leases/release {lease}
POST agent-assignments/{id}/events/poll  {lease,bindingId,limit}
POST agent-assignments/{id}/events/ack   {lease,dispatchId}
POST agent-assignments/{id}/events/start {lease,dispatchId,executionId,inputDigest}
POST agent-assignments/{id}/events/finish {lease,dispatchId,executionId,outcome}
POST agent-assignments/{id}/replies      {lease,dispatchId,executionId,body}
POST agent-assignments/{id}/replies/reconcile-known {lease,dispatchId,executionId,body}
POST agent-assignments/{id}/observations {uploadTicket,canonicalReceipt}
POST agents/{id}/stop                   {commandId,expectedAssignmentGeneration}
POST agents/{id}/revoke                 {commandId,expectedAssignmentGeneration}
GET  agents/{id}/management             authorized lifecycle/budget/stats only
```

These are proposals, not available endpoints. Keep exact camelCase closed schemas, JSON-safe counters, 16 KiB command/receipt envelopes and bounded response pages. Prompts/replies use the existing separately bounded text transport rather than fitting raw content into a command receipt.

The executor delivery envelope follows the allocation envelope: original immutable command plus canonical digest, observed server time, executionAllowed, finite lease/upload token and original recorded receipt if any. Recover always grants historical evidence upload only. A fresh reserve is exact original-generation work; cleanup can use current authorization of the same original device/installation with a later generation. Cleanup never permits model execution, provisioning or replacement assignment.

Assignment metadata is not fresh execution permission. A native reserve receipt plus pending assignment may qualify a runtime but cannot consume a chat prompt. Fresh assignment leases have distinct provisioning/work states. Work lease admission checks both actual Matrix readiness and native qualification. Client-provided room/owner/provider fields cannot establish either.

## Native production bridge and custody

Add a separate `console/native/managed_agents` module, owned by the native host lifecycle. Use the storage-independent management authority/profile pin and canonical owner registry, with the native synchronous retirement callback and before/after-I/O checks. A mini-app can request/approve/stop through closed central operations but cannot mint a provider, assignment, runtime or usage proof.

There are four distinct production proof families:

- Reserve delivery: exact existing `ManagedAgentReserveCommand`, original project command/receipt from the registry, current enrollment and fresh executor poll ticket. The adapter may construct `VerifiedManagedAgentDelivery` only after all three canonical commitments agree.
- Provision/assignment: exact native child reservation receipt plus current central pending/ready assignment and short lease. The store verifies the child, manager/project/definition, actual account, resource, period and permanent fence. A read/recovery projection cannot construct it. The runtime plan exposes only trusted native configuration, never arbitrary executable, environment, directory or credential JSON.
- Prepared versus fresh-started call: split production prepare authority from issue authority. Current fixture-only `ManagedCallAuthority` works for storage tests but does not distinguish ACK from `newlyStarted=true`. A fresh start proof must bind the exact dispatch/input commitment, execution ID, assignment/lease epoch and finite deadline. Replayed `newlyStarted=false` can only recover. Persist prepared hold before central start; issue the call only after a fresh start acknowledgement and a final local gate.
- Runtime observations: accept only the original retained `OwnedSession`'s source and monotonic observation sequence. No public `new(usage,complete=true)` or Deserialize constructor. A capture adapter creates the ledger observation from source-bound evidence, retains the exact pending evidence during every write, and uses evidence-only cleanup after its original effect deadline. Restart cannot reconstruct source custody from stored IDs.

The production store API must supply a final synchronous/live check immediately before model admission, and a native cancellation fence watched during inference. Stage 2's returned custody handle is one-shot storage evidence; possession alone cannot authorize a delayed effect after revocation. No await may cross a profile/device/enrollment/assignment/deadline change unnoticed. The process owner is retained outside any cancellable spawn future, using the existing `spawn_prepared` late-child handoff pattern and guardian Drop backstop.

The first version deliberately uses finite, non-renewed per-call custody bounded by the current lease/enrollment. The supervisor may renew its idle assignment lease, but it must not extend an already-issued handle or turn just by replacing a cached proof. Long-running calls require a separate reviewed renewable-live-gate protocol.

## Provider and no-tools boundary

Extract only registration-independent sealed launch/config/identity inspection from personal Codex code. Keep personal Ledger and execution loop unchanged. Introduce a closed tools-disabled policy in the low-level session: a private empty runtime directory, explicit no shell/unified-exec/file/image/browser/web-search/apps/hooks/plugins/MCP/skills/remote-control capability, no inherited personal instructions, no external tools, no arbitrary config, and actual returned configuration checks. A read-only sandbox alone does not establish this policy. Any unexpected tool item/request is refused and interrupts the turn while retaining possible usage.

Before inference, verify public configuration, trusted backend origin and stable provider quota subject on the **same retained process** that will receive the prompt. Match it to the canonical registry's provider digest and resource preset. Repeat bounded identity checks around turn admission as supported; account/reference change retires the live gate. The earlier metadata-only identity probe is useful enrollment evidence, not authority to execute under a different process/account. Unknown provider identity or unsupported binary/config vocabulary fails closed. No automatic login, new credential copy, or selection based on email/path.

Use `OwnedSession::{spawn,initialize,start_thread,start_turn,next_observed_update,interrupt,stop}` and the existing guardian. Current `Settings` lacks this complete sealed mode and public account/config read bridge; those are explicit additions, not existing guarantees. Do not weaken the legacy runtime's settings to make the new path fit.

## One event through the system

1. Appservice verifies the Matrix transaction; routing requires the exact active managed binding, current manager membership, actual puppet membership/send authority, explicit mention and plaintext text. Persist the original event and managed assignment snapshot. No Matrix credentials or provider identity enter the prompt.
2. Executor loads original execution history and compares it to its private managed inbox and canonical call ledger before acquiring a work lease. Missing issued history blocks new work. Polling obtains a prompt only for the exact assignment/device/profile. Store the immutable dispatch/input/execution commitment locally before ACK.
3. Prepare a bounded per-call hold in the canonical registry. ACK means local persistence, not a model start. Central start commits the immutable execution ID; its `newlyStarted=true` response is consumed once by the native issue gate. Lost central start response becomes uncertain historical work, never automatic model retry. A call prepared but definitely not issued can be cancelled only with durable no-issue evidence.
4. Commit native `issued` and retain its process/call custody before the model turn. Check current account, assignment, profile and deadline immediately before transmission. A crash after issue but before/after transmission remains unknown; do not infer zero from absence of output. Poll/lease failure or native retirement interrupts and joins the retained process.
5. Capture every scoped runtime observation in order. Persist normalized cumulative high-water usage with the original observation IDs before advancing or publishing final output. Missing, malformed, discontinuous, out-of-order or unacknowledged evidence remains unknown and blocks new work on that physical provider seat. Stop does not equal settled billing.
6. A completed protocol turn plus an earlier usage update is **not automatically complete coverage**. Complete usage requires a pinned, tested final-coverage rule for this exact provider protocol. Until that rule is demonstrated, expose known consumed tokens with `usageComplete=false`; do not free the remainder. Final text may still be retained/replied as completed output while accounting remains incomplete. A second turn must not quietly bypass that block.
7. Persist final reply text/digest and usage reference in a private managed transport journal before central submit. The central reply outbox keeps its original sender/room/thread/Matrix transaction ID. Trusted send workers recheck current managed assignment, generation, lease, membership and send permission. Revoke cancels definitely unsent replies; already-sending/unknown outcomes retain their original identity for observation-only reconciliation. Never regenerate a reply after restart.

The private transport journal is new work: schema 76 has call/evidence metadata but no immutable prompt/reply spool or central ACK/start/send receipts. It should live in the canonical registry with profile-scoped access and bounded retention for body data; permanent execution/accounting commitments survive any permitted payload pruning. Do not store bodies in the public stats/notification outbox.

## Stop, revoke, usage and removal

Central stop/revoke first fences assignment work, expires leases, prevents new candidate/ACK/start/processing/tool/reply authorizations, and records one cleanup command in the same transaction. Manager may stop/retire owned agents; the current assigned project admin may revoke project admission. Owner withdrawal stops capacity; it does not become an extra human approval. Assignment revision/admin replacement races must be checked at command delivery and response commit.

Native cleanup records the permanent child fence before touching processes, cancels prepared calls, interrupts/joins any original process and preserves issued calls until genuine usage/cleanup evidence arrives. A process absence after restart is not proof that a turn never ran. Late evidence is accepted using the original installation binding with fresh cleanup authority and can raise retained liability. An old generation cannot regain work. Initial stop is terminal for this child; a future resume/replacement requires an explicit new contract, not removal of schema 75/76 immutable pins.

Stats must be separate from the last reserve receipt: exact budget, high-water consumed, prepared/in-flight holds, unknown flag, nullable remaining, native ledger revision, observation timestamp and runtime/stop state. The manager and assigned admin see scoped agent stats; coordinator/resource-owner capacity views omit conversation bodies and provider account IDs. A central stale snapshot is labelled stale, not zero remaining/consumption.

Removal is completed only after central fencing, native stop acknowledgement and actual Matrix membership cleanup. The permanent agent identity, manager ownership, original commands, used tokens and unknown execution history remain. Releasing a child only releases proven unused capacity under the existing parent derived-liability collector. Stage 2 has fencing and project release; it does **not** yet expose a truthful standalone managed-child settlement or top-up operation. Add those typed operations before a UI claims capacity refunded or token request completed. Top-up must remain an assigned-project-admin decision and cannot enlarge the parent/project allocation automatically.

## Implementation checkpoints and ownership proposal

1. **Server executor/admission bridge:** new agent command delivery/result/recovery, exact receipt ingestion, shared identity+assignment schema and guards, pending provisioning projection, stop queue. Keep work unavailable. Server source only: `management_agent_executor*.rs`, managed schema migration, typed fixtures, narrow domain/worker hooks and real PostgreSQL/HTTP tests. This is the smallest next independently complete implementation checkpoint.
2. **Native reserve/provision bridge:** new `console/native/managed_agents/{transport,provider,provision,journal}.rs`, production proof factories and exact runtime qualification, no prompt delivery yet. Store/runtime changes owned by one ledger/runtime agent; native transport/host lifecycle by a separate native agent. Coordinate Cargo/module/migration edits and preserve source census's conservative history blockers. Qualifying a runtime does not yet claim a chat turn works.
3. **Shared transport and first turn, one activation unit:** server authority adapter at every transport/history/worker/SQL path; native immutable inbox/reply journal, split call prepare/start proof, retained process supervisor, source-bound usage capture and actual Matrix reply. Stop/revoke and honest stats are part of this checkpoint, not deferred cleanup. No capability advertisement until all acceptance below passes. Top-up/child-settlement are a separate bounded follow-on if excluded explicitly from this initial UI.

Do not parallel-edit `transport.rs`, `domain.rs`, runtime session policy or native schema registration without designated owners. Root owns live fixture upgrades, desktop builds, activation and real-provider E2E.

## Activation acceptance

- Real PostgreSQL/HTTP: four separate manager/admin/coordinator/executor identities; assigned admin alone may decide, including manager/admin same-account case. Server admin/coordinator/resource owner alone denied. Wrong installation/device/generation/profile, changed setup/allocation/resource/provider, expired period, cancelled request and revoked grant never issue fresh work.
- Transaction rollback and replay: decision-to-delivery, reserve receipt-to-identity, identity-to-assignment, Matrix join-to-ready and revoke-to-cleanup. Lost replies recover exact original bytes; late successful reservation after cancellation retains a cleanup obligation without becoming runnable. Two command IDs cannot create duplicate semantic holds/identities.
- Every personal generic bind/device/start route rejects managed agent/project/room bypass; unrelated existing personal regression suites pass unchanged. SQL direct mutations cannot bypass manager/device/assignment FKs or revive terminal fences.
- Actual local process fixtures: no model before issued commit, late spawn stopped after timeout, shutdown/profile retirement waits for process owner, replay/restart/unknown central start never reruns, no-tools configuration verified and unexpected tools rejected. Same retained provider process account mismatch blocks inference.
- Actual reader/writer usage fixtures: exact source/sequence, cumulative cache-inclusive counters, final coverage or honest unknown, complete zero vs absent counters, write/reply loss, oversized counts, late overrun, provider-seat cross-profile blocking. No complete/refunded claim from terminal output or process death alone.
- End-to-end isolated Matrix + real enrolled Codex account: approved request visibly joins the actual room; one mention yields one reply; native consumption is durable and reflected in bounded stats; central/native crash at each durable boundary produces no second turn/reply; stop and admin/resource revoke stop ongoing work and reject follow-on work. An uncertain Matrix send reconciles with the original transaction ID.
- Validate withdrawal and both receipt arrival orders while reservation/provision/start/send is in flight. Cleanup with current generation works under the same original installation; work with rotated generation fails. Default flags/off and old personal clients remain compatible.

Open activation prerequisites remain source-ledger reconciliation, trusted grant enrollment/publication, managed setup custody, provider configuration sealing, central shared executor transport and the runtime adapter described here. Storage tests and canonical fixtures do not substitute for these integrations.
