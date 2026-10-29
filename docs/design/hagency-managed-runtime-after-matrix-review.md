# Execution gaps after the Matrix membership checkpoint

2026-10-16. Read-only implementation review after paired central schema10/native
schema77 validation. No runtime, schema or deployment changes are made here.
This narrows the existing stage3 design to the next reviewable vertical slice.

## What is established, and what is not

The source has actual `MatrixClient` calls and a serialized membership worker;
its tests use real loopback HTTP, including delayed join/revoke/late membership.
They do **not** prove that the current deployed homeserver accepts that puppet,
that a real enrolled owner registry/admitted contribution matches, or that a
provider can run. The current live fixture remains schema9. Real integration
acceptance must explicitly connect the native transport, current real resource,
server worker and actual homeserver; a synthetic native receipt is insufficient.

The Matrix checkpoint intentionally has `runtimeState="unsupported"` and
`workAvailable=false`. It leaves the identity suspended, assignment
`pending_native`, personal bindings forbidden and native provision custody
unissued. Removing these guards would bypass missing mechanisms below.

| Boundary | Exact current code | Remaining production gap |
| --- | --- | --- |
| Central execution owner | `execution_devices.rs::require_assigned` and `transport.rs::{agent_active,lease,dispatch,claim_reply}` | Personal routes require the permanent manager's device. The resource owner's device is different. Keep those predicates/FKs; add a separate exact managed authority and SQL scope. Never rewrite a Principal to look like the manager. |
| Matrix versus execution | `management_agent_matrix.rs::{matrix_phase,matrix_confirm}` and `management_agent_executor_types.rs::PendingManagedAssignment` | Membership is a bounded observation, not a provider qualification or work lease. No current assignment endpoint grants fresh provision/model permission. |
| Native production proof | Client `owner_resources/capacity/managed_agents/custody.rs::{ManagedAssignmentAuthority,ManagedCallAuthority,ManagedCleanupAuthority}` | Constructors remain `cfg(test)`. A production adapter needs exact authenticated fresh leases, immutable original child/receipt, current enrollment/profile and finite expiry. GET/recovery/old accepted commands must never mint these proofs. |
| One-shot runtime issuance | `custody/provision.rs::{claim_managed_provision,managed_provision_history}` | The store correctly commits `issued` before returning non-cloneable custody. No native host driver consumes it. `History(issued)` after crash cannot authorize a second process. |
| Actual provider identity | `console/owner_provider/identity.rs::verify` | This authenticates a short-lived metadata child and kills it. It is not the identity of a future retained inference process. Qualification must verify the same quota account on the actual retained process against the canonical seat. |
| Process and policy | `hagency-runtime/src/owned/session.rs::{spawn,observation_source,stop,interrupt}`; `hagency-agent-local/src/codex.rs::{Profile,Session}` | Reuse supervised ownership and sealed policy below personal-ledger plumbing. `OwnedSession::qualify_ready_owner` proves process ownership only. Neither a PID nor a generic read-only policy proves no host tools/workspace/web/MCP. Do not use personal `Session::open_context` with a fabricated personal ledger or legacy Registration. |
| Usage provenance | `custody/observations.rs::{ManagedProvisionObservation,ManagedUsageObservation}` and `hagency-execution/src/usage{,/capture}.rs` | Production observation constructors are absent. They must consume retained runtime source/sequence/thread/turn evidence, not JSON counters. Terminal output does not prove complete billing. Personal unreported-zero behavior must not settle shared liabilities. |
| Work and reply delivery | Server `transport_history.rs`, `processing.rs`, `workers.rs::{route,send}` | Every claim/ACK/start/history/reply/processing/send boundary needs the same managed assignment and current authority. Widening only polling or lease acquisition is unsafe. No managed event path is complete today. |
| Stop/revoke/recovery | Native `managed_agents/fence.rs`, `custody/observations.rs::fence_managed_agent`; server `management_agent_executor.rs` | Permanent ledger fencing exists, but no managed process supervisor is attached. An old fence receipt cannot later claim a process stopped or usage settled. Append a distinct late observation with current cleanup authority and keep unknown holds. |

## Smallest next complete slice: qualify and stop the actual retained runtime

Recommend checkpoint2c before shared chat dispatch: an exact central fresh
provision lease → one retained Codex process → account/policy qualification →
durable native observation → central runtime projection → revoke/stop/recovery.
It must cover the entire process lifetime even though it admits no model turn.
It is independently useful: it closes the currently missing production custody
and cleanup boundary without pretending that personal transport can serve shared
agents. Keep a separate default-off host gate and no advertised runnable agent.

### Central contract and persistence

Reserve a new central migration only after review; do not change schema10's
Matrix challenge or schema9's reserve/fence bytes. Add a provision-intent row
referencing the exact existing assignment and native reserve receipt. Freeze a
closed `agent.runtime.provision` command containing:

- command ID, assignment ID/generation, child ID;
- original reserve command/receipt digests, definition digest;
- exact enrollment/device/original generation, current setup/authority/allocation
  bindings already established by the reserve;
- runtime policy version `codex_no_tools_v1` and fixed issue/expiry times.

Use a separate finite delivery ticket and append-only native observation records
with immutable canonical digests. Observation is an exact target/command/child/
assignment binding, native ledger revision, state, optional runtime-policy
commitment, process state and retained/unknown accounting. Raw account identity,
provider locator, credentials, executable path and prompt/body never enter this
contract. A policy commitment is not accepted without the native qualified
observation; Matrix verification alone is insufficient.

Fresh poll requires current Matrix observation, assigned-project-admin decision,
manager/project/setup/allocation/resource authority and exact enrolled device.
Recover is observation-only, always non-executable. Result replay never extends
a lease. Current device generation may recover/stop the original installation's
process history, but cannot provision under an older generation. Revoke must
atomically fence fresh provision and enqueue cleanup before any acknowledgement.
An accepted qualification arriving after revoke remains history and cannot
restore readiness. Identity stays suspended and work remains unavailable in 2c.

### Native driver and proof construction

New `console/native/managed_runtime` owns this separate lifecycle. It consumes
the actual host/canonical-registry/enrollment capability, not a mini-app path.
Before every await and effect, pin SDK profile, exact device, native owner access,
resource account, permanent child/parent fence and finite central lease.
Use the installation lifecycle guard and synchronous retirement cancellation.

Only a fresh executable provision delivery may create production
`ManagedAssignmentAuthority`. After `claim_managed_provision` returns **Issued**,
commitment already exists; start exactly one retained supervised process. Select
only trusted owner-consented native configuration, with no arbitrary command/
path/env from central or UI. Verify actual effective no-tools settings and the
same account on that process. Produce a non-deserializable runtime observation
from retained process evidence and write it to the canonical ledger before any
central upload. A metadata probe or a fabricated successful observation cannot
qualify it. Errors after spawn preserve uncertain state and retained process
cleanup; errors before spawn must be distinguishable by durable evidence.

Attach the retained process to cancellation for logout/profile replacement,
owner access retirement, period/resource/grant loss, central revoke, expiry and
host shutdown. Stop is bounded and records the observed process-tree outcome.
An expired execution proof may only be replaced with cleanup evidence authority;
it cannot obtain fresh provision custody. Restart sees history, never respawns
an `issued`/unknown effect. Initial single-generation scope explicitly has no
resume/reassignment after stop; a successor protocol is a later feature.

### Source ownership and acceptance

Server scope: new `management_agent_runtime{,_types,_schema,_tests}` plus narrow
policy/API/cleanup hooks. Preserve Matrix controller, personal routes and SQL
owner-device constraints. Native scope: new `console/native/managed_runtime`,
small reviewed production-proof constructors in `custody`, and a sealed runtime
adapter below existing personal/legacy scope factories. Coordinate existing
`NativeOwner`/global lifecycle files with their owner; do not splice in a fake
legacy Registration, RunnerCapability, owner room or personal Ledger.

Required tests for this slice:

1. Real SQLite + HTTP + supervised fixture child: exactly one spawn across lost
   reply, cancellation, crash/restart and command replay; history never reissues.
2. Wrong actual quota account, swapped native configuration, unknown effective
   policy, tool/server requests, wrong profile/device/generation or expired lease
   fail closed. No central JSON can provide authority or usage provenance.
3. Revocation while spawn/qualification/result is in flight stops/retains the
   original process and never restores ready. Late stop observation can refine
   evidence without freeing uncertain consumption or overwriting original bytes.
4. Shutdown/retirement returns only after bounded quiescence or explicit unknown;
   retain the process identity and liability if the guardian cannot prove stop.
5. Existing personal and managed ledger/controller tests remain green. Paired
   real enrollment → actual homeserver membership → actual retained native
   provider qualification is a separate explicit integration acceptance, run
   only by the activation owner. Fixture tests alone do not fulfill it.

## Complete first-turn slice that still follows 2c

Do not claim the full ADR after qualification. A complete first turn requires
all managed transport boundaries together: history witness, finite shared lease,
explicit manager mention, durable native event custody, ACK, fresh start,
per-call reservation/issue, actual turn, canonical usage/result save, exact
Matrix transaction reply, revoke/stop and recovery. Keep manager ownership and
resource-owner execution separate throughout. Use a new closed shared authority;
retain personal owner-device FKs unchanged.

Split call preparation authority from **newly started** execution authority.
The current `ManagedCallAuthority` can drive both fixture prepare and issue;
its future production constructor must not turn an old `newlyStarted=false`
response into a new provider call. Issued local history never recreates custody.
One tools-disabled Codex turn, one plaintext project room, one executor and one
assignment generation is the initial supported scope. Missing usage remains
unknown/retained even if a textual reply or terminal event exists. Actual
complete-capture evidence is a provider adapter requirement, not a boolean the
server or mini-app can submit. Top-up/stats/removal must use the same native
budget/usage records; coding tools, workspace consent, renewal and reassignment
remain explicit parity items rather than silently enabled fallbacks.

The administrator assigned to the project is the only human agent approver,
including when also the manager. No coordinator/resource-owner second approval
is introduced by runtime qualification or cleanup.
