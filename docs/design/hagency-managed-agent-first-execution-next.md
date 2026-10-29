# Assigned project administrator to the first managed agent execution

> Historical checkpoint: schema 22 permanently combines project manager and
> project administrator. Current role and upgrade behavior are documented in
> [Operations](../OPERATIONS.md#identity-and-creation-rights). Separate assignment
> and reviewer handoff below describe the earlier implementation.

Date: 2026-10-16. Status: implementation proposal, following validated central management schema 7. No implementation, deployment, advertised capability or live execution is authorized by this document. It narrows P2/P3 in `hagency-project-provisioning-and-agent-admission-next.md`; schema 7 already supplies setup cancel/rebase/admin replacement, contrary to that document's historical schema 6 limitation paragraph.

## Bounded outcome and existing policy

The first complete vertical slice is: the immutable project manager requests a text-reply agent in the verified project discussion room; the administrator assigned to that project approves; the exact enrolled executor reserves a child budget, qualifies its existing owner-configured runtime, and automatically starts when central Matrix provisioning is ready. A manager's mention produces one durable response with observed usage. Token increase, statistics, stop and removal use the same assignment and child accounting. The resource coordinator does not review agent requests.

The first execution capability explicitly excludes tools, arbitrary workspace access, owner DMs, extra rooms, requester delegation, provider switching and automatic period renewal. Only offers that actually support this text-only mode may be selected; do not label a tool-dependent coding agent fully supported. Keep the existing richer original features in the parity backlog rather than deleting them. This is a bounded execution acceptance target, not completion of all agent management.

The retained parity inventory (`hagency-client/docs/parity/console-2026-09-24.md` and `docs/design/native-execution-parity.md`) still names the following required follow-on capabilities: provider/account preparation and retirement; first-resource creation and enrollment; model/reasoning/preset editing; monthly ceilings and daily rate caps; offer publication/withdrawal and per-engagement limits; project-side enrollment/verification and credential rotation; agent start/restart, stop, retirement and cleanup retry; task inspection/continue/accept-completed/keep-blocked; tools, reusable execution grants and their revocation; encrypted file send/receive, private owner DMs, extra rooms and requester/delegation policy; per-agent/project usage and runtime observations; alert notes/triage; and Claude/Octos runtime qualification. Their old coordinator approval entrypoints must be adapted to the assigned-project-admin policy rather than restoring an extra human agent-creation vote. Existing native tool/workspace consent remains a separate runtime boundary.

Roles remain distinct:

| Role | Authority in this slice |
| --- | --- |
| Server administrator | Admit project and explicitly assign its project administrator. No implicit permission to approve that project's agents. |
| Project manager | Permanent human agent owner; request creation/top-up, inspect owned stats, stop and retire own identity. |
| Assigned project administrator | Approve/reject new agent and token requests; revoke this project's agent admission. |
| Resource coordinator | Allocate project capacity under the existing owner grant. No second agent approval. |
| Resource owner/executor | Establish provider/resource consent once, automatically enforce allocation and runtime policy; withdraw resource authority and stop unsafe work. No per-agent approval dialog. |

Approval follows the explicit project-administrator assignment even when that account is also the manager. Neither a second role nor the coordinator grant's `allowSelfApproval` flag changes this policy. Server-administrator, resource-owner and coordinator roles alone grant no agent-approval permission.

## Reuse boundaries verified in source

| Mechanism | Reuse boundary |
| --- | --- |
| Central permanent `agents` identity and Matrix provisioning/membership workers | Retain manager ownership, puppet namespace, real identity checks, real joins, exact transaction/recovery and cleanup. Add managed authority guards before selecting or activating work. |
| Central transport state machine | Extract common transitions below a verified authority enum. Keep personal endpoints/checks intact; add closed shared routes. Preserve immutable started execution, history snapshots, exact reply transaction IDs and send-unknown recovery. |
| Native `approve_coordinated_agent`, `approve_coordinator_top_up` | Useful examples of atomic child hold, receipt and quota transition. Their public authority entrypoints are **not reusable**: pinned `palpo-hagency-contract` authorizes the coordinator actor and legacy `VerifiedRequest`/`Registration`/`ProjectGrant`. |
| Native `approve_allocating_inner`, `OwnedProvisionScope`, existing `engagements/projects/effects` | These require real legacy registration/project FKs and registration generations. Do not fabricate a Registration or legacy Matrix owner room. A new typed managed scope and separate managed records are required. Extract only registration-independent runtime/config/receipt primitives. |
| Native owner-registry project hold | This is the actual parent of the new agent child. It currently uses `reservation_only_v1` and cannot run an agent. Attach execution/accounting atomically before any effect. |
| Native `coordinator_agent_usage`, `quota_holds`, coordinator settlement | Retain distinctions between observed lower bound, incomplete usage, quota pause and final settlement. Existing resource-owner reconciliation is accounting evidence, not another business approval. |
| Personal `agent-local.sqlite` runtime | Do not use it as the shared-account ledger or silently change it. It is profile-local, holds overlapping policy accounts, and intentionally supports `unreported-zero-v1`. Shared unknown usage must remain charged/blocking in the canonical owner ledger. |

Source anchors include server `domain.rs::create_agent`, `execution_devices.rs::require_assigned`, `transport.rs`, `transport_history.rs`, `processing_schema.sql`; client `domain/coordinator.rs`, `domain/coordinator/settlements.rs`, `domain/provision_runtime.rs`, `domain.sql`, and `domain/owner_resources/capacity{.rs,/parent.rs}`. The native legacy FKs are a real integration boundary, not just different naming.

## Central schema: preserve the two owners explicitly

Use the next atomic management migration after schema 7, without changing personal defaults:

1. `agents.execution_kind text NOT NULL DEFAULT 'personal' CHECK IN ('personal','shared_project')`. It is immutable. Shared rows require `execution_device_id IS NULL`. Keep the existing `(execution_device_id,owner_user_id) → devices(id,user_id)` FK and permanent owner/puppet trigger unchanged in meaning. Personal assignment/lease APIs explicitly reject shared rows even after revoke; no fallback or conversion.
2. `management_agent_requests`: `id`, immutable `plan_id`, `setup_id`, original `setup_binding_revision`, `allocation_id`, `allocation_receipt_digest`, `manager_user_id`, `domain_project_id`, `room_id`, bounded canonical `definition`, `definition_digest`, exact monthly `requested_tokens`, `period_key`, `authority_revision`, `state`, `revision`, `expires_at_ms`, and unique `proposed_agent_id`. This last ID is reserved once in the request, not yet a runnable `agents` row. Resolve owner/executor/room from storage, not request JSON.
3. `management_agent_decisions`: immutable request ID/revision, deciding administrator user, authority revision, approve/reject, decision command ID/digest and decision time. One accepted verdict per request revision. Use existing actor/device command receipts, audit and reference-only outbox in the same transaction.
4. `management_agent_executor_commands`: immutable exact reserve/top-up/stop/release commands, native owner/device/generation scope, digest, lease token hash/deadline and original result/digest. Separate table/routes from project reservation and parent settlement; those v1 receipts retain their meanings.
5. `management_shared_assignments`: `id`, `agent_id`, `human_owner_user_id`, plan/setup/domain project, project allocation and receipt digest, native agent child ID/receipt digest, resource owner user, exact executor device/generation/enrollment, resource/grant IDs, `assignment_generation`, state and period expiry. Composite FKs enforce agent/human-owner and executor/resource-owner pairs. Scope triggers enforce project/allocation consistency. One current assignment per agent; old generations/history remain permanent.
6. `management_shared_bindings`: exact assignment, binding ID/generation, room and scope. The first slice has one verified project discussion binding. An extra owner-direct room or registered child room cannot inherit admission automatically.
7. `management_agent_token_requests`: immutable agent/assignment, requested additional amount and expected native budget revision, current administrator assignment revision, state/revision/expiry and exact command receipts. Reuse the same decision policy, not the legacy coordinator top-up policy.
8. Append-only bounded native usage snapshots: assignment + native ledger revision, canonical receipt/digest, observation time and last current projection. Older arrivals remain recoverable but cannot replace a newer balance or reduce known consumption.

Do not weaken the existing managed-project generic-bind guard. The typed admission transaction inserts a binding only when an exact validated assignment/admission row exists; SQL validates the same relationship on insert/update. Existing generic `bind_room`, `owner_direct`, room adoption and personal execution-device setters keep failing for managed scopes. Requests cannot attach an already-existing personal agent as a shortcut.

## Closed management contracts

All counters use JSON-safe integers, IDs/hashes use existing bounds, bodies stay below 16 KiB and pages below native response limits. Canonical encoding uses `hagency_core::canonical` and cross-repo fixtures. No requester-controlled actor, executor, native account, provider reference, filesystem path, shell command or raw Matrix state.

```text
POST projects/{plan}/agent-requests
{ commandId, expectedSetupRevision, expectedSetupBindingRevision,
  expectedAuthorityRevision, expectedAllocationRevision,
  name, capability, description, requestedTokens }

POST agent-requests/{request}/decide
{ commandId, expectedRevision, expectedAuthorityRevision,
  decision: "approve" | "reject", reason? }

GET agent-requests/{request}
POST agent-commands/{command}/recover
{ operation: "agent.request" | "agent.decide" | "agent.tokens.request"
           | "agent.tokens.decide" | "agent.stop" | "agent.revoke"
           | "agent.retire" }

POST agents/{agent}/token-requests
{ commandId, expectedAssignmentGeneration, expectedBudgetRevision,
  requestedAdditionalTokens, reason }
POST agent-token-requests/{request}/decide
{ commandId, expectedRevision, expectedAuthorityRevision,
  decision: "approve" | "reject", reason? }

GET agents/{agent}/management
POST agents/{agent}/stop
{ commandId, expectedAssignmentGeneration }
POST agents/{agent}/revoke
{ commandId, expectedAssignmentGeneration, reason? }
POST agents/{agent}/retire
{ commandId, expectedAssignmentGeneration }
```

The central `capability` selects only the exact admitted resource's published capability; native verifies actual current resource qualification and the unchanged resource revision. The initial resource fixes runtime/model/template through its trusted owner configuration. Adding manager-selectable models/templates later requires a versioned native public offer, not a freeform provider argument.

Creation/top-up requests use assigned-admin `canDecide` plus explicit blocking reason. Request approval is `approved_waiting_executor`, never `active`. Existing admin-assignment changes reroute pending requests and advance their revisions. Old-admin decisions that have not become an active assignment are fenced; an issued or uncertain reserve is cleaned up with its original command/child identity before any replacement. Already active agents retain their historical approval until an explicit lifecycle revocation; changing reviewers alone does not transfer ownership or revoke working agents. New top-ups always require the current assigned administrator.

Current Matrix readiness is verified through trusted Gateway state, not by extending the old setup's 30-second timestamp. Preflight network reads followed by final transactional CAS mirror setup confirmation. Check current setup binding, live exact allocation, current manager/admin membership and authority before issuing new executor work. Keep original room markers/IDs through setup rebase.

## Executor reserve and activation sequence

Use the existing envelope pattern on separate `agent-executor-commands/{poll,recover,result}` routes: current authenticated owner/device, exact immutable command digest, finite lease beginning at request send, `executionAllowed`, historical `recorded?`, durable exact acknowledgement. Recovery is upload-only and never a new-effect lease. Generation rotation may clean up/recover the original same installation; it cannot execute an old reserve or silently adopt another installation's ledger.

The first closed reserve command freezes:

```text
ManagedAgentReserveCommand {
  version:1, operation:"agent.reserve", commandId,
  requestId, requestRevision, proposedAgentId,
  managerMxid, projectId, projectRevision,
  setupId, setupBindingRevision, domainProjectId, roomId,
  authorityRevision, decisionActorMxid, decisionDigest,
  allocationId, allocationReceiptDigest,
  grantReceiptDigest, enrollmentId, executorDeviceId, executorGeneration,
  resourceId, resourceRevision,
  definition:{name,capability,description,toolMode:"none"}, definitionDigest,
  requestedTokens, periodKey, issuedAtMs, expiresAtMs
}
ManagedAgentReservationReceipt {
  version:1, commandId, commandDigest, requestId, agentId,
  allocationId, agentReservationId, definitionDigest,
  state:"reserved"|"refused", budgetRevision, ledgerRevision,
  allocatedTokens, retainedTokens, periodKey, reason?
}
```

The native command adapter loads the original locally durable project reservation, parent grant, profile/resource ACL and authenticated current enrollment. Hashes identify records; they do not replace the actual local proof. It verifies the actual resource/provider account and current trusted runtime policy. Unknown or unsupported provider identity cannot qualify an executable offer. Today the verified provider identity path is Codex-specific; do not guess Claude/Octos identity from a path or email.

1. Central approval commits decision, immutable reserve command and notification outbox atomically. No native hold or Matrix user is claimed yet.
2. Native transaction reserves an agent child against unused project capacity and writes its exact receipt plus managed provision intent. New IDs cannot duplicate an existing semantic request. Change the project and parent accounting mode to a supported executable mode in this transaction, before any provider/model effect. A lost response is recovered from the original ledger receipt.
3. Central records that receipt under the exact delivery ticket and rechecks live request/assignment authority. Only then create the permanently manager-owned `agents` row with `execution_kind=shared_project`, assignment and exact joining binding atomically. If authority changed, retain the receipt as history and enqueue cleanup; do not provision a runnable identity.
4. Existing trusted server Matrix workers register the exact puppet and join the real project room. Their selectors and completion transitions also require the live managed admission. Never expose Appservice secrets or create manager credentials on the executor. Revoke during provisioning leaves visible cleanup rather than an active binding.
5. Native managed provision intent qualifies the existing owner-configured runtime without issuing a model turn. A non-deserializable `ManagedProvisionScope` binds original effect/receipt, provider-account proof, resource revision, profile, assignment and local runtime custody. It does not embed a fabricated legacy Registration. Record exact qualification outcome and retain uncertainty on process failure.
6. Only an actual native process with that provision proof and a current central shared lease may report running. Public projection distinguishes `reserved`, `matrixProvisioning`, `runtimeReady`, `running`, `quotaPaused`, `stopping`, `cleanupUnknown` and `retired`; puppet identity `active` alone means none of these.

## Shared execution: one complete boundary

Keep existing personal leases unchanged. Add `management_shared_execution_leases` keyed by assignment, with agent epoch, assignment/binding generation, human owner, executor owner/device/generation, expiry and exact execution history witness. Add immutable nullable `shared_assignment_id/generation` pins to owner events, replies and processing intents. SQL requires null for personal scope and exact current managed scope for shared scope. A shared agent cannot simultaneously acquire a personal lease.

An internal, non-deserializable `ExecutionAuthority::Personal` / `::Shared` selects common transport transitions. Shared authority contains **both** human owner and authenticated executor principal. Never mutate `Principal.user_id` or forge a manager principal to satisfy old owner filters. Owner projections remain manager-scoped; the executor gets only the exact assignment's event/history/configuration data.

Closed shared transport endpoints under `shared-execution/assignments/{id}` mirror the existing operations: history, acquire/renew/release lease, candidates, claim/ack/start, finish-without-reply, submit/reconcile-known-reply. Each input pins assignment generation and the existing exact lease/event/execution identity. Tools have an explicit denial response in the first capability; there is no tool-authorize bypass.

The complete first message order is:

1. Existing Matrix ingest routes an explicit manager mention for the exact admitted project binding, preserving immutable event identity. Other room/member traffic is not an implicit requester grant in this slice.
2. Native durably saves the exact event and ACKs only afterward. Server validates current binding/owner/requester membership, grant/resource/assignment state, period, device generation and lease at claim, ACK and start.
3. Native atomically records one per-call reservation and issued attempt under the agent child; central `start` preserves immutable execution identity before a model call. Replay of `newlyStarted:false` authorizes historical recovery only.
4. Runtime uses the pinned provider/account/configuration and rejects all tool, host-file and external action requests. Recheck central lease and native account/resource authority before model admission. Cancellation cannot be represented as successful completion merely because the process was killed.
5. Record actual reported usage and known result durably before submitting the response. Central reply retains the original Matrix transaction ID and uses actual trusted delivery checks at worker claim/send. Assignment loss blocks queued sends/processing markers; an uncertain send retains identity for exact reconciliation.
6. Missing result/usage remains an unknown attempt with retained liability. Restart cannot rerun the model on a historical command or transform missing usage into zero. Known reply recovery does not establish that tools/model usage settled.

Change every shared boundary together: lease, candidate visibility, event claim/ACK/start, history, finished/reconciled replies, processing indicators, worker send, stale-dispatch invalidation, lifecycle stop and SQL triggers. Partial widening of one `owner_user_id` predicate is not sufficient. Existing personal tests must remain unchanged and pass.

## Native accounting, top-up, stats and removal

Use new managed records in the **same canonical owner-registry database**, not a new accounting database and not legacy registration rows:

```text
owner_managed_agents(
 id PK, profile, agent_id UNIQUE, request_id UNIQUE,
 project_reservation_id FK, original_command_id/digest,
 definition_digest, allocated_tokens, budget_revision,
 retained_tokens, state, accounting_mode)
owner_managed_effects(id PK, agent_child FK, kind, state, fence,
 immutable_payload_digest, original_result/digest)
owner_managed_calls(id PK, agent_child FK, assignment_generation,
 central_event_id, execution_id UNIQUE-in-child, immutable_digest,
 state, reserved_tokens, known_consumed_tokens, unknown_liability,
 usage_encoding, original_result_digest)
```

Add a versioned `managed_agent_v1` liability collector to the same existing commitment engine. Count the parent hold once at provider/resource level; agent children partition the project hold. Do not also add a legacy engagement to resource commitments. Project available capacity subtracts retained children, including consumed current-period tokens and unresolved work. Observed overruns remain charged and can exceed the original allocation; never clamp them into falsely available space.

The current parent cleanup intentionally treats any mode other than `reservation_only_v1` conservatively. An executable mode must add its verified consumed/unresolved collector and child release logic before any refund is enabled. Until that implementation is complete, parent/project release retains the full known hold and reports blocked; simply setting the old mode back is forbidden. Per-call estimates are admission accounting, not a guaranteed provider billing cap. Pause new work on exhausted/uncertain capacity and preserve late actual usage.

Top-up follows the same assigned-admin request/decision flow. The frozen `agent.top_up` command pins child/assignment, project receipt, current period, expected **budget revision** and expected allocated amount plus exact additional amount. Native checks current project headroom and increments only the child's allocation/budget revision atomically with its receipt; it does not enlarge the parent/project grant. Live usage may advance ledger revision independently. Lost replies recover the original top-up; a different command ID cannot repeat the semantic request. No period rollover or renewal is implied.

Stats come from bounded, canonical native snapshots authenticated as the exact enrolled executor: `agentId, assignmentGeneration, ledgerRevision, budgetRevision, periodKey, allocatedTokens, knownConsumedTokens, reservedInFlightTokens, unresolvedTokens?, remainingTokens?, usageComplete, runtimeState, observedAtMs, previousReceiptDigest`. Unknown is nullable/explicit, never zero. Native deduplicates per-call reported usage with the existing cache-inclusive input/output semantics; central cannot synthesize usage from message counts. Manager and assigned admin see scoped agent stats; resource owner/coordinator receive authorized capacity aggregates without message bodies or private provider identity. Old snapshots cannot overwrite newer balances.

Stopping/revoking fences new central work first by advancing assignment/agent/binding authority, expiring leases and cancelling definitely unsent replies. It then enqueues exact native stop/cleanup. Manager stop is reversible only through a future explicit current resume command; manager retire is permanent human identity retirement. Assigned-admin revoke removes project admission; resource withdrawal removes capacity. Neither role becomes the agent's owner. Initial removal preserves permanent identity/history and awaits actual native/Matrix cleanup.

Cleanup survives same-installation device generation rotation and original grant expiry, while never reviving execution. A native stop receipt proves what stopped, not that unknown usage is free. Release only proven unused child capacity after durable no-running/unknown accounting checks; retained consumed/unresolved amounts continue charging the current period. Resource-owner reconciliation of missing external billing is a separate accounting action, never another human approval to create an agent.

## Implementation order and activation acceptance

Build three independently reviewable checkpoints, with one default-off/unadvertised final capability:

1. **Central admission + personal isolation:** closed manager/admin requests, exact decision/outbox/recovery, shared identity discriminator/SQL guards and typed executor contract. No agent identity or active execution before native receipt. Real PostgreSQL rollback/migration tests.
2. **Native managed child + provision/usage custody:** exact command fixtures, atomic child accounting/effects, supported execution mode, pinned runtime/provider and per-call unknown/stop/usage receipts in the canonical ledger. No legacy registration fabrication or personal ledger redirection.
3. **Complete shared transport + first real execution:** current authority enum at every path, Matrix provision/room visibility, native driver, stats/top-up/removal and crash/recovery. Only after this checkpoint may the capability be advertised to the mini app.

Required acceptance includes:

- Four distinct identities: manager, assigned project admin, resource coordinator and resource owner/executor; server admin is separate when feasible. Only the assigned project admin can approve agents/top-ups. Old admin, unrelated project admin, coordinator, resource owner and server admin acting only in those roles are denied. No second owner/coordinator prompt appears.
- Wrong actor/device/profile, generation rotation, stale setup/admin/child revisions, wrong room, pending/revoked native allocation, expired period and withdrawn resource fail without side effects. Active assignment approvals survive reviewer replacement as specified; new work is fenced by actual revocation.
- Concurrent reserve/top-up cannot exceed project headroom. Parent is counted exactly once; two Matrix profiles sharing one verified provider account still share one ceiling. Unknown calls, consumed retired children and late overruns remain charged.
- Fault injection before/after decision, native reserve, central receipt, identity registration, Matrix join, runtime qualification, event ACK/start, provider call, usage/result save, Matrix send, stop and release. Original command/attempt/Matrix IDs survive every lost response; no duplicate model call, puppet, budget increase or refund.
- Actual manager mention → one model response in the verified project room, with durable usage and monotonic stats. Agent token increase succeeds inside project headroom and refuses above it. Stop/removal during a running call fences new work, retains unknown accounting, and eventually reports the actual cleanup result.
- Denied tool/host-file/web actions remain denied even if requested by model output or a room message. Missing native provider proof/workspace consent fails before any effect. No credential/provider locator/private file content appears in server or mini-app projections.
- Every personal unrelated project/agent/device/lease/owner DM/history/reply test remains unchanged and passes, including direct SQL FK/trigger violations and generic managed-bind bypass tests. Migration failure rolls back, restart is idempotent, and no existing agent silently changes execution kind.

Existing activation dependencies remain: reconciled real owner resources, current trusted enrollment/provider proof, durable native Matrix setup custody and complete local accounting. Synthetic receipts and method existence do not satisfy these gates.

## Checkpoint 1 implementation boundary

The schema 8 admission checkpoint implements manager requests, assigned-administrator approve/reject, manager cancellation, administrator replacement, and exact actor/device recovery. `POST projects/{plan}/agent-requests` accepts the revisions above plus `definition:{name,capability,description,toolMode:"none"}` and `requestedTokens`. `POST agent-requests/list` provides the separate bounded Inbox (`needsMyAction`, `after`); `GET agent-requests/{id}`, `POST agent-requests/{id}/decide`, `POST agent-requests/{id}/cancel` and `POST agent-commands/{command}/recover` return `agentRequest`. Decisions accept only `commandId,expectedRevision,expectedAuthorityRevision,decision`; cancellation uses `commandId,expectedRevision`. Closed bodies reject actor/executor injection. `canApprove` and `canReject` distinguish unavailable capacity from an administrator's remaining right to reject; `canDecide` covers either action.

The current shared-identity SQL fence rejects all `shared_project` agent inserts, and makes `execution_kind` immutable. Approval writes an immutable `management_agent_intents` record in `unavailable`, never a native delivery or agent identity. No `agent-executor-commands` routes exist yet. Exact native command and future reservation receipt shapes live in `management_agent_types.rs` with `management-agent-admission-v1.json` canonical vectors. The new reference-only notification outbox is transactional intent; no Matrix notification delivery is claimed. Original project and resource Inbox shapes remain unchanged.

The host configuration `enable_agent_admission` defaults false and depends on the existing reservation/setup gates; the capability is never advertised even when an isolated test enables these flags. Pending reviews follow a newly assigned administrator with a new request revision. An old accepted but unexecuted intent is fenced and the request revoked. Original request room/allocation/definition remain immutable; no personal owner/device FK is relaxed. Native result acceptance, executable assignments, shared transport, top-ups, usage, stop/removal and richer original agent capabilities remain the following checkpoints.

Validation on 2026-10-16: the isolated PostgreSQL runner passed all 151 tests with none ignored, including 10 new managed-agent contract/admission/authority/race/rollback/migration tests. Existing personal execution/DM/history/reply regressions remained green. Four older expiry fixture updates now use PostgreSQL time rather than a 1 ms Mac-clock boundary; production timeout rules are unchanged. Server compilation, strict all-target Clippy and formatting checks pass. This is server checkpoint validation, not native execution or whole-workflow E2E qualification; no deployed server was replaced.
