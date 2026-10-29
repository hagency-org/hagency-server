# Managed agent ledger checkpoint

Status: stages 1 and 2 implemented behind an unwired native proof boundary,
2026-10-16. Stage 2 focused validation passes; full-suite validation follows
reviewed schema 76 audit acceptance. This document does not enable an
agent, add a transport route, migrate a production profile, or authorize a
provider call. It narrows checkpoint 2 of
`hagency-managed-agent-first-execution-next.md` to one independently testable
native ledger boundary.

## Frozen central input

Schema 8 admits a manager's request only through the administrator explicitly
assigned to that project. That administrator may also be the manager. Neither
the server administrator, resource owner nor coordinator obtains this authority
from their other role. Resource consent and project allocation remain separate;
there is no second human resource-owner/coordinator approval of an agent.

The exact input and receipt are `ManagedAgentReserveCommand` and
`ManagedAgentReservationReceipt` in
`crates/agent-service/src/management_agent_types.rs`. The canonical vector is
`crates/agent-service/tests/fixtures/management-agent-admission-v1.json`:

* command digest:
  `b24b300c1be02ae99f0638d0def4d9d0bd5975e2f0c818198814725fae3699f0`;
* receipt digest:
  `babced513abd7a23660207d55dc8176418bb2ff6157685d50c966efd8796ee77`.

The server accepts no native agent result yet and rejects all shared agent
identity inserts. A ledger receipt is a reservation, not a running identity.

## Existing code and the gaps this checkpoint must close

The client owns one canonical OS-owner `OwnerResourceRegistry`, backed by the
existing `DomainRepository` worker and process lock. `capacity.rs` counts only
the parent grant in resource/provider commitments; its project reservations
partition that parent. Preserve this arrangement and the existing personal
owner/device foreign keys. Do not insert legacy `engagements`, fabricate a
`Registration`, or redirect personal `agent-local.sqlite` accounting.

Four concrete gaps prevent simply attaching an agent to a project row:

1. `owner_project_reservations` stores the original command digest and a binding
   digest, but not the original manager or project definition. Its receipt does
   not contain the manager either. Load the actual original reserve command and
   verify its canonical digest against both durable native records before using
   those fields as authority.
2. `retained_tokens <= requested_tokens` on that table cannot represent provider
   overruns. Keep that original partition record immutable; calculate execution
   liabilities separately, without clamping observations to the approved amount.
3. `additional_commitments()` currently reads the original grant or last parent
   settlement. A late usage observation after settlement must increase current
   commitments immediately, before another parent settlement command arrives.
4. `reservation_only_v1` release proves zero usage solely because that mode has
   no execution attachment. Changing its mode without adding a complete
   liability collector would invalidate that proof. Unknown migrated modes must
   remain conservative and unavailable for execution.

## Scope and native proof boundary

Add an unwired native store module, not a mini-app operation. Mirror the closed
central command/receipt with JSON-safe unsigned integers and the exact
canonical fixture. `VerifiedManagedAgentDelivery` is non-deserializable and
contains current `VerifiedManagementEnrollment`, the exact command and digest,
a positive executable-delivery flag, and a finite monotonic lease. Construction
belongs to the future authenticated transport; a JSON approval flag is not
authority. Recheck profile/session, registry shutdown and lease before queueing,
inside the transaction, before commit and after return. A lost post-commit
authority check returns outcome unknown; original receipt recovery is read-only.

The proof also carries the original `AllocationCommand` as supporting evidence.
It must hash to the existing project row's `reserve_digest` and the existing
`owner_capacity_receipts` command digest; recompute its child binding and match
the exact stored grant, project ID/revision, manager, amount and period. Persist
that original command in an additive authority table for new reservations. Old
rows may gain this metadata only by that exact digest check, including evidence
recovered from an authenticated historical command. Hydration grants no new
lease, creates no hold and never upgrades an unknown mode. A missing command or
digest is an explicit blocker, not an invitation to infer the manager.

Reserve additionally checks the live project state, parent permanent fence,
current ACL/resource revision, exact local allocation receipt digest, original
parent receipt digest, enrollment/device/generation, current UTC period and
definition digest. The agent manager must equal the original project manager.
The agent approver comes from the central assigned-administrator decision;
never apply the coordinator's `allowSelfApproval` policy to this decision.

## Smallest durable schema

Domain version 75 and `077-owner-managed-agents.sql` are reserved with the client
owner. New strict tables:

* `owner_project_authorities(allocation_id PK/FK, reserve_command,
  reserve_digest, manager_mxid, definition_digest)`: immutable, exact original
  project command, never a replacement authority.
* `owner_managed_agents(id PK, profile, agent_id, request_id,
  project_reservation_id FK, original_command_id, original_command_digest,
  immutable_command, definition_digest, allocated_tokens, budget_revision,
  state, permanent_fence)`: unique `(profile, request_id)` and
  `(profile, agent_id)` prevent a different command ID from duplicating the same
  request. Original manager/project/provider bindings never change.
* `owner_managed_effects(id PK, child_id FK, kind, state, fence,
  payload_digest, original_result, original_result_digest)`: one provision intent
  per child. States `pending`, `issued`, `qualified`, `failed`, `unknown`.
* `owner_managed_calls(id PK, child_id FK, execution_id, immutable_digest,
  assignment_generation, state, reserved_tokens, known_consumed_tokens,
  usage_complete, original_result_digest)`: unique `(child_id, execution_id)`;
  states `prepared`, `issued`, `settled`, `unknown`, `cancelled_before_issue`.
  Separate immutable observation/receipt records preserve original reports and
  reject changed replay; a summary row is only a checked projection.

Reuse the existing owner command receipt store and global ledger revision for
new durable commands; retain exact original-command replay revisions. Keep a
separate agent budget revision for reserve/top-up CAS. Do not make transient
usage revisions invalidate an otherwise current budget approval.

Reserve atomically records the child, original command, exact receipt, and
pending provision intent. Change only the affected grant/project to a supported
`managed_agent_v1` accounting mode in this same transaction. No provider effect,
Matrix user, room, runtime or legacy engagement is created. Refusal is also an
exact durable receipt; it never creates a child or changes accounting mode.

Land reserve/refusal, original project authority and the derived accounting
collector as the first tested stage. Add provision/call custody only after that
stage passes. Existing project command validation currently accepts only the
reservation-only mode: extend it deliberately for the supported managed mode.
A managed sibling must not prevent another project's legitimate reserve or
release. Each sibling uses its own derived liability; no managed mode may fall
through the old blanket zero-release branch.

## Provision and call custody, without a runtime driver

Provide store primitives for the eventual native driver and test them directly:

1. `reserve_managed_agent(delivery)` and exact historical recovery.
2. `claim_managed_provision(...)` returns a native non-deserializable custody
   token bound to registry instance, original effect, child, provider account,
   resource revision and live profile. It durably marks the attempt issued
   before any future external initialization. An already issued effect never
   yields another fresh-effect token after restart. The new scope must not
   embed the legacy `OwnedProvisionScope`'s mandatory `Registration`.
3. `record_managed_provision(...)` accepts an observation from the holder of
   that scope. Qualification is separate from a current central assignment or
   permission to run. Missing/uncertain outcome stays unknown.
4. `prepare_managed_call(...)` freezes the exact central execution identity,
   assignment generation, provider/resource identity and estimated token hold.
   `issue_managed_call(...)` commits one-shot issuance and returns custody only
   for the first issuance. Replays return history, never permission to rerun.
5. `record_managed_usage(...)` accepts native runtime-adapter observations with
   exact attempt/custody identity and normalized token semantics. Missing usage
   is not zero. Cache-inclusive input accounting must match the existing codec,
   and repeated cumulative reports must not be summed as independent calls.
6. `fence_managed_agent(...)` permanently blocks new provision/call admission.
   A known never-issued attempt may release its estimate. An issued, unknown or
   missing result keeps liability. A fresh same-installation cleanup proof may
   record late evidence after generation/period change, but cannot issue work.

These methods require a trusted native assignment/call proof type even while
unwired. Do not provide a production constructor that turns arbitrary script
JSON into runtime authority. Fixture-only constructors belong under `cfg(test)`.
No server/native transport is added in this checkpoint, so an actual model turn
remains impossible through this new API until checkpoint 3 supplies that proof.

## Conserved accounting and late evidence

One shared collector must feed child availability, parent settlement, native
stats and the existing resource/provider commitment engine. Do not maintain
independent counters in a second database.

For an active or unresolved agent, retain at least its allocated child budget
and any larger known consumption plus outstanding reservation lower bound. A
permanently fenced child can shrink to known consumed tokens only after every
issued attempt has complete observed usage and no live custody remains.
Unreported usage keeps `usageComplete=false`; `remainingTokens` is unknown, not
an invented zero-consumption refund. Preserve previously known higher
consumption and allow later positive evidence to increase liability.

An active project holds at least its original reservation and any greater sum
of managed child liabilities. A released project retains the full derived
consumed/unresolved child liability. Original `requested_tokens` and native
reservation receipts never change. The legacy zero-release branch is available
only to the proven reservation-only mode with no managed children.

The existing project-release v1 wire receipt also caps retained tokens at the
original requested amount. If exact managed liability exceeds that amount,
persist the execution fence and refuse release with `unsettled_usage`; do not
clamp the observation or fabricate a successful receipt. The owner ledger and
parent settlement still charge the full amount. A later versioned usage or
release projection is needed to communicate the larger child balance.

At provider/resource level emit **one commitment per parent grant**. Before
close, charge the greater of the original parent hold and current derived
project liabilities. After close, charge the greater of its last exact retained
settlement and current derived liabilities. A later exact settlement may lower
only demonstrably unused capacity. Consumed tokens and late overruns survive
close, revoke, retirement and UTC rollover.

The current generic allocation core treats a null commitment as zero; do not
use that encoding for uncertain managed usage. Add a native collector flag
consumed by `budget_with_grant()` so current resource/seat remaining values are
`None` while provider liability is unresolved, and all new capacity admissions
fail closed. Keep numeric retained amounts as conservative known floors, not a
claim of complete billing. Scope this guard to the affected verified provider
seat/resource; preserve unrelated personal budgets. The same collector must
protect existing native admission paths that share that provider account.

Arithmetic is checked and JSON-safe. An observation exceeding representable
bounds must durably retain a blocking unknown record rather than roll back into
apparently unused capacity. This is admission accounting, not a provider-enforced
hard spending cap.

## Tests and file ownership

Owner: project_backend for this additive store slice after the native release
freeze ends.
New files under `native/hagency-store/src/domain/owner_resources/capacity/managed_agents/`
hold closed types, transactional reserve, effect/call custody, accounting and
focused tests. Minimal integration edits: `owner_resources.rs`, `capacity.rs`,
`capacity/parent.rs`, the next migration, domain schema registration, and the
native budget/usage projection entry points. Coordinate the small `domain.rs`
export/migration edits with native_projects; that agent owns reconciliation
audit modules. No edits to desktop lifecycle, native managed Matrix setup,
personal Ledger, central transport or runtime drivers.

Acceptance before calling this ledger checkpoint complete:

* Byte-identical canonical command/receipt fixtures and closed-body/numeric
  bounds; first reserve, exact replay, changed replay and semantic duplicate ID.
* Wrong manager, grant, local receipt, provider, profile, enrollment, installation,
  resource revision, period, generation and lease fail without a hold.
* Two profiles sharing one provider seat cannot evade its existing ceiling;
  concurrent children cannot exceed project headroom; the parent is counted once.
* Atomic rollback at child/effect/receipt writes and mode changes; migration
  rollback and restart preserve all personal rows and old conservative modes.
* Parent close/project release versus agent reserve races are serialized. No
  new child appears after the permanent fence, including delayed original work.
* First provision/call issuance yields one custody token; process loss and exact
  replay cannot rerun either effect. Session retirement while queued or after
  commit preserves original receipts and unknown outcomes.
* Complete zero usage, missing usage, cumulative duplicates, partial reports,
  late completion, overrun, stop during issuance and unknown cleanup produce
  distinct correct liabilities. Unknown never frees project/provider capacity.
* A settled parent followed by late positive usage immediately increases the
  provider commitment without waiting for another server command. Both receipt
  arrival orders and changed generation cleanup preserve the same liabilities.
* All existing owner reservation/parent settlement tests, personal domain tests
  and strict Clippy remain green. No capability advertisement or provider call.

Checkpoint 3 still owns central result acceptance, manager-owned agent identity,
shared assignment leases, every work/history/reply/tool/stop authority boundary,
actual Matrix joining, native transport/runtime, token-increase approvals and
user-facing stats/removal. Passing ledger tests does not establish this E2E.

## Additive setup-readiness input

The existing `GET projects/{plan}/allocation` selects the greatest opaque
allocation ID. It is not the exact live reservation chosen by setup readiness.
`ProjectReadiness` currently exposes neither that selected revision nor a
replacement allocation when rebase is possible.

Implemented additive optional field, serde-default for old saved receipts:

```text
eligibleAllocation: { id, revision, projectRevision, receiptDigest } | null
```

Populate it from the exact unpinned `setup_capacity(db, plan, None, now)` result
used for begin/rebase. It is null without a current live executor-confirmed
reservation. Existing `capacityReady` remains pinned to `currentBinding` and
can therefore be false while a replacement is eligible. Keep original setup
descriptor, stage IDs and room identities immutable. This projection supplies
real CAS inputs only; it grants no Matrix-create permission and is not a native
receipt or enrollment proof.

New begin commands must include `allocationId` from that projection as well as
`expectedAllocationRevision`. The server selects that exact row; a newly
confirmed replacement with the same numeric revision cannot silently replace
the user's selection. For historical recorded commands only, the input field
is optional and omitted during serialization when absent, preserving their
original command digest. Exact replay precedes the new-input requirement;
unrecorded inputs without an ID return `project_setup_allocation_required`.
The matching native journal preserves old omitted fields and recovers an old
recorded begin, but never posts an unpinned historical input or invents a current
selection for it. Original setup descriptors and creation stage IDs are intact.

The setup protocol is discoverable as `project-setup-v1` only when the explicit
server `project_setup` policy is enabled. Reservation issuance can be disabled
independently: readiness then disallows dependent effects, while setup history,
cancellation and command recovery remain discoverable. All flags remain false
in the default and running configurations.

This is a **paired client/server checkpoint**, despite retaining database schema
8. The earlier unwired native `ProjectReadiness` DTO denies unknown fields and
cannot parse `eligibleAllocation`. Its optional-field mirror must ship before
that client is used against these responses. Old stored server command receipts
still decode through serde defaults; that fact does not make an old strict
native DTO wire-compatible. Keep the original schema 8 artifact for old-adapter
tests. No runtime fixture upgrade is part of this change.


## Stage 1 validation checkpoint

Native domain schema 75/file077 is implemented in the canonical owner store.
The central/native command and receipt fixture match exactly. Native reserves
verify the actual original project command and receipt, persist semantic request
uniqueness, and partition the existing parent hold. Derived liability is shared
by project admission, resource/seat remaining, old native headroom and parent
settlement. Unresolved usage blocks new capacity on the same physical provider
account. Late observations raise the current hold above an old settlement, and
an overrun cannot be clamped into a successful v1 project release.

Validation (client `.run`):

* `managed-agent-ledger-tests-3.log`: 10 focused tests passed, including exact
  replay and receipt recovery, concurrent capacity, wrong authority, rollback,
  historical-command hydration, cross-profile unknown usage, immutable history,
  and a lease expiring after worker commit but before caller return.
* `managed-agent-ledger-all-lib-tests-2.log`: all 147 store library tests passed
  before the added post-return regression; that regression subsequently passed
  independently and in the focused 10-test run.
* `managed-agent-ledger-clippy-3.log`: all-target store Clippy passed with
  warnings denied. Native audit owner independently reviewed schema 75 and
  passed its 11 audit tests; managed histories remain adoption blockers.

Unknown/late usage collector tests seed native evidence deliberately. They do
not prove a provider runtime or transport integration. No native assignment or
call proof constructor, capability advertisement, server result acceptance or
actual shared-agent execution is activated by stage 1.


## Stage 2 implementation boundary

Domain schema 76/file078 adds immutable assignment and call pins plus append-only
observations. Every observation retains its exact canonical numerical evidence,
content digest and original result/revision. It does not retain prompts, tool
bodies, credentials or paths. `recover_managed_observation` returns the original
receipt even when newer usage changes the current balance.

The native-only `ManagedAssignmentAuthority`, `ManagedCallAuthority`,
`ManagedCleanupAuthority`, `ManagedProvisionObservation` and
`ManagedUsageObservation` deliberately have **no production constructor**.
Their fixture constructors exist only under `cfg(test)`. The future authenticated
shared transport/runtime adapter must supply real assignment and source proofs;
script JSON, an historical receipt and an approval flag cannot create them.

Provision and call issuance persist before returning their one-shot custody
handle. An issued/unknown row after restart yields history, never another handle.
A handle is bound to the actual registry instance and current native profile,
original effect/call, assignment digest, issuance revision and finite deadline.
Every queue boundary rechecks current authority; the outer return also checks
its shorter enrollment/assignment deadline. Fresh same-installation cleanup can
record late original evidence after resource-access or executor-generation
retirement, but it cannot issue work or transfer an old handle to another store.
A single assignment generation is pinned in this checkpoint; reassignment is a
future explicit protocol, not an automatic upgrade of old native records.

Usage uses the existing Codex normalization and ceiling convention: cache-inclusive
input is split once, and ceiling consumption is fresh input + cache write + output.
Cumulative reports advance a high-water value rather than being added together.
A complete observed zero is distinct from missing usage. A higher late report
raises liability immediately; a valid older partial report cannot erase an
already complete observation. Unrepresentable individual or aggregate amounts
are durably marked unknown, and a permanent agent fence remains recordable even
when no exact JSON-safe aggregate can be returned. Resource queries then remain
unavailable instead of reporting free capacity.

The first slice conservatively blocks further admission on the same physical
provider while an execution is issued or unresolved. Parallel provider scheduling,
actual runtime invocation, shared transport, server result acceptance and UI
activation remain outside this checkpoint. Custody is evidence ownership, not a
standalone authorization to call a provider; the future driver must perform its
live assignment/fence checks at the actual effect boundary as specified in P3.

Focused stage 2 validation: `managed-custody-tests-2.log` passes 13 tests, covering
one-shot and concurrent issue, restart history, exact original receipts, queued
retirement, parent-close races, numerical/cumulative/zero/missing/late usage,
cache normalization, aggregate overflow, foreign-store denial, revoked-access
cleanup, observation rollback and schema75→76 rollback. The added exact historical
observation-recovery assertion passes in `managed-custody-recovery-test.log`.
`managed-custody-owner-tests-1.log` passes the then-current 52 owner-resource tests;
`managed-custody-clippy-1.log` passes strict all-target store Clippy before the final
recovery addition. These results do not establish an actual model or Matrix turn.


Schema 76 final combined verification: `CARGO_BUILD_JOBS=2 cargo test -p hagency-store --lib` passed 181/181, zero failed/ignored, in 65.52 seconds after the source-census/archive owner accepted all three retained custody tables. Log: client `.run/managed-custody-all-lib-final.log`. Final strict all-target store Clippy passed after the latest custody changes (`.run/managed-custody-clippy-final.log`). The selected personal/coordinator integration suites passed 121 tests with zero failures and five unchanged documented ignores (`.run/managed-custody-personal-regressions-2.log`). No production runtime proof constructor or model path is enabled.
