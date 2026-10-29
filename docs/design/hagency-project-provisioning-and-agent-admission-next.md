# Allocated project readiness and assigned-project-admin agent admission

> Historical checkpoint: schema 22 permanently combines project manager and
> project administrator. Current role and upgrade behavior are documented in
> [Operations](../OPERATIONS.md#identity-and-creation-rights). Separate assignment
> and reviewer handoff below describe the earlier implementation.

Date: 2026-10-16. Status: implementation proposal for review. Server source is
frozen at the validated management schema v5; this document changes no runtime,
configuration or source. The parent settlement counterpart is being implemented
separately in native code and remains unadvertised.

## Decision and first bounded slice

Implement **executor-confirmed allocation → real Matrix project/room → verified
central linkage and explicit project administrator** first. Reuse the existing
owner Matrix creation/recovery journal, not a second service-bot creation flow.
Then implement one complete shared-agent authorization path before exposing
agent creation as runnable. Merely adding an approval button or swapping an
agent's device would violate the existing permanent human-owner model.

The user policy is already settled:

| Step | Human authority |
| --- | --- |
| Admit a project plan | Designated server administrator. |
| Allocate capacity under the owner grant | Exact delegated resource coordinator. |
| Assign the project's agent administrator | Proposed explicit designated-server-admin action; never inferred from room power, manager, owner or coordinator role. |
| Request an agent within that allocated project | Project manager; initially the immutable project manager is also the agent's permanent human owner. |
| Approve/reject the agent or its token increase | Administrator explicitly assigned to this project. |
| Apply reservation, provision, execute and enforce budgets | Enrolled executor automatically validates the accepted decision; no second Hagency/resource-owner/coordinator human approval. |

An account may hold several roles only through their actual assignments. The
historical ADR 0011 coordinator-agent approval text is superseded here by the
user's explicit requirement and the current management contract. Existing native
method names are implementation reuse points, not a reason to restore that role.

## Existing mechanisms and precise gaps

| Existing mechanism | Reuse and limit |
| --- | --- |
| `management_projects.rs` and `management_allocations.rs` | Approved immutable plan, exact native reservation result, revoke/recovery and notification outbox. `approved` is not room readiness; `reserved` is not executable agent capacity. |
| Native `server_login/matrix_creations.rs` (`CreateMatrix`, `Creation`, `Creations`, `ResumeCreation`) | Closed manager-owned creation journal, persisted before `POST createRoom`, exact creator/marker verification, partial setup recovery and no blind retry after unknown outcome. Currently a console adapter; extract its typed service rather than call a local HTTP server from the mini app. |
| Native `AdoptProject`/`AdoptRoom` | Explicit existing-space/room selection under the signed-in owner's Matrix session. Does not prove connection to the approved management plan. |
| `DomainStore::register_project`, `register_room` | Register actual `projects`/`rooms` using fresh trusted `Gateway::admin` observations. They do not create Matrix rooms or know project plans. Extract transactional helpers so domain registration and management linkage commit together. Never forge `AdminFacts` or a manager `Principal`. |
| `Gateway::{admin,room,visible}` | Real server-side Matrix state, membership, child link, invitation power and encryption checks. Incoming JSON cannot supply these facts. Existing freshness limit is 30 seconds and must be rechecked after lock waits. |
| `DomainStore::create_agent` | Global permanent owner identity, durable idempotency and owner's personal execution device. Cannot be called as the admin/coordinator to create a manager-owned shared agent. |
| `bind_room`, `adopt_owner_direct`, identity/membership workers | Durable joining/activation/cleanup, owner DM separation, real puppet state. Binding and provisioning need an additional managed-project admission gate. |
| Native `approve_coordinated_agent` / `receive_coordinator_agent` | Existing reservation, definition validation, durable delivery and provisioning mechanics. Legacy coordinator authority and `Registration` must be replaced by a typed management authority adapter, not fabricated to satisfy checks. |
| Native `begin_project_setup` / `validate_project_setup` / `finish_project_setup` | Useful exact-command recovery/state patterns, but require an existing legacy project grant and real room IDs. They are not room creators or proof for the new owner-registry allocation. |

Source inventory: `hagency-desktop/docs/design/hagency-management-parity.json`,
`OWN-023–028` (adoption/creation/recovery), `OWN-031` (agent identity),
`MGT-067–084` (engagement/project setup), `MGT-006–024` and `MGT-029–044`
(agent lifecycle/approvals), original `JS-069–070` and `JS-125–128`.
Do not mark these entries accepted based on method existence or this design.

## Slice P1: allocated plan to Matrix readiness

Keep `management_projects.revision` and its capacity definition unchanged during
admin assignment or room setup: the native reservation froze that revision.
Use independent authority and setup revisions. Never amend the existing native
reservation receipt merely to attach Matrix identifiers.

Implemented server migration v5→v6 (default off):

- `management_project_authorities(plan_id PK/FK, administrator_user_id FK,
  revision, assigned_by_user_id FK)`. Revision starts at one after an
  explicit assignment; no default inherited administrator. Preserve immutable
  assignment-change history and original mutation receipts.
- `management_project_setups(id PK, plan_id UNIQUE/FK, allocation_id UNIQUE/FK,
  manager_user_id FK, plan_revision, allocation_receipt_digest,
  authority_revision, definition_digest, mode, state, revision,
  space_creation_command_id, room_creation_command_id, space_id?, room_id?,
  domain_project_id? UNIQUE/FK, verified_at_ms?)`.
  Mode is `create` or `adopt`. Candidate IDs remain untrusted until verification.
  A database check requires all final Matrix/domain IDs for stored `ready`.
- A binding admission guard references this managed linkage and later exact
  agent admissions. In P1 there are no valid admissions, so no agent binding to
  the managed project can activate. Do not use `CreationPolicy.default_allow`
  as an approval substitute.

Use existing `management_commands`, audit and reference-only outbox atomically.
Extend receipt recovery only with exact typed operations; do not turn resource
Admission recovery into a generic dispatch endpoint. New closed routes under
`/hagency/api/v1/management`:

| Route | Closed input / projection |
| --- | --- |
| `PUT projects/{planId}/administrator` | `{commandId, expectedAuthorityRevision, administratorMxid}`. Only configured server admin. Resolve the target to an active existing same-server user; no caller-supplied user ID or authority proof. Revision zero means initial assignment. |
| `GET projects/{planId}/setup` | `{readiness}` containing `setup?`, `administrator? {mxid, revision}`, separate `capacityReady`, `matrixReady`, `canAssignAdministrator`, `canBegin`, `canConfirm`, `blockingReason?` and `observedAtMs`. Null does not imply a missing allocation. |
| `POST projects/{planId}/setup` | `{commandId, expectedProjectRevision, expectedAllocationRevision, expectedAuthorityRevision, mode}`. Manager only. Server freezes manager, allocation/result digest, names and deterministic native stage command IDs. No requester-selected executor, token amount or grant. |
| `POST project-setups/{setupId}/confirm` | `{commandId, expectedRevision, spaceId, roomId}`. Manager supplies candidate IDs; server fetches actual state and verifies everything itself. The same binding can be confirmed after loss of the reply. |
| `POST project-setup-commands/{commandId}/recover` | Exact operation `project.setup.begin`, `project.setup.confirm` or `project.admin.assign`; original actor/installation receipt with current visible projection. |

Target native-plus-server P1 state machine (server persists only `awaiting_matrix_setup` / `ready`; the remaining stages are derived native/recovery projections):

`awaiting_administrator → awaiting_matrix_setup → setup_in_progress →
awaiting_membership / outcome_unknown / verification_blocked → ready`.

Capacity loss makes derived readiness `blocked_capacity` immediately; preserve
room IDs and history. These are setup states, not replacements for the original
plan/admission/allocation states. No successful room operation changes balances.

### Creation and recovery ordering

1. Begin checks approved plan, exact live `reserved` receipt/balance, active
   contribution/grant, current native enrollment/generation, unexpired period,
   no parent fence/release and assigned admin. Commit the setup intent first.
2. Trusted native host derives two stage IDs from setup identity. Extract the
   existing Matrix creation service and its private command custody; the script
   cannot provide HTTP paths, access tokens, arbitrary state or room creators.
   Preserve account/issuer/installation binding and verify reply identity after
   each await. Create the private Space and private discussion Room as the
   authenticated manager, invite service and the assigned administrator, and
   retain the normal manager power levels. No force-join or creator impersonation.
3. Bind new creation markers to setup/plan/definition digest in a versioned
   trusted `MatrixOperation` variant. Preserve old journal/marker formats for old
   commands; do not reinterpret them. Set reciprocal space-child/room-parent
   state using the existing exact-state conflict handling.
4. A missing create response stays `outcome_unknown`. Recover the original
   command by exact creator/marker and known/discovered Room ID. Do not mint a
   fresh creation ID or repeat the POST to make a failed test pass. New devices
   may inspect the setup but cannot invent replacement rooms; confirming the
   same original observed rooms is a new authenticated management command.
5. Server confirmation uses fresh trusted state: correct `m.space`, exact child
   link, manager joined with required policy authority, service joined/can
   invite, administrator identity matching the current assignment, and supported
   unencrypted discussion Room. Invite pending is not joined/readiness. In create
   mode require the original manager/creation marker; explicit adopt mode uses
   current real administration proof and rejects conflicting management linkage.
6. Under the shared domain/transport advisory transaction lock, reauthorize,
   recheck allocation/authority/fact freshness, and atomically register/link
   domain project and room, increment setup revision, save receipt/audit/outbox.
   Existing `register_*` public methods take their own transaction, so reuse
   extracted internal helpers instead of nesting independent commits.

The current native creator automatically adopts the new Space into the personal
domain before its child Room exists. Preserve the same journal but allow its
managed variant to stop before registration, or make the final confirmation
adopt that exact pre-existing domain row transactionally. **Never silently adopt
a project with existing agent bindings.** Reject it in P1, including a concurrent
binding attempt. Install the managed binding guard in `bind_room`, common binding
policy/worker activation and database scope validation in the same migration.
Otherwise default-allow personal binding bypasses the project administrator.
Global personal-agent creation and unrelated personal projects retain their rules.

No server Appservice secret is sent to the manager. Do not repurpose
`MatrixClient::create_probe_room` into a privileged manager identity or claim
service-created space ownership. Existing Matrix state APIs remain authoritative.

The project administrator is a business approver, not automatically a Matrix
moderator. Assignment alone does not bypass `Gateway`/Matrix power requirements
for room policy edits. A future admin change increments only authority revision,
reroutes pending agent requests with new request revisions and notifications,
and fences unexecuted approvals from the old assignment. Previously activated
agents retain immutable historical decisions until explicit lifecycle/policy
revocation; changing a reviewer does not silently transfer their ownership.

## Slice P2: one durable agent request and assigned-admin decision

Requires P1 plus the shared execution/capacity boundary below before activation.
Keep a separate feature `managed-agent-admission-v1` unavailable until native
reservation and transport support are both present. An intermediate tested
central request slice must label accepted decisions `awaiting_executor`, not
`active`, and must not emit a runnable identity through old routes.

Proposed next migration adds:

- `management_agent_requests`: immutable manager/human owner, setup/domain
  project, project allocation and period, public definition/canonical digest,
  requested tokens, authority revision, bounded expiry, request/state revision.
- `management_agent_decisions`: exact request/authority revision, assigned
  deciding admin, decision/reason and frozen command digest; one terminal
  decision per revision, with original command receipts.
- `management_agent_executor_commands/results`: separate typed native child
  reserve/provision/cleanup envelopes and durable acknowledgements. Do not store
  fake project reserve results or reinterpret an existing allocation ID.
- `management_shared_assignments` and exact permitted bindings described below.

Closed proposed routes:

```text
POST projects/{planId}/agent-requests
{ commandId, expectedSetupRevision, expectedAuthorityRevision,
  name, role, description, requestedTokens, definitionRef?, definitionRevision? }

POST agent-requests/{requestId}/decide
{ commandId, expectedRevision, expectedAuthorityRevision,
  decision: "approve" | "reject", reason? }

GET agent-requests/{requestId}
POST agent-commands/{commandId}/recover { operation }
POST agent-executor-commands/poll { after? }
POST agent-executor-commands/{id}/recover { commandDigest }
POST agent-executor-commands/{id}/result { commandDigest, token, receipt }
```

The server derives manager/owner, assigned admin, project/resource/enrollment,
permitted Room, period and executor. The script cannot inject provider account,
workspace path, shell command, actor or credentials. Template/model choices must
come from qualified native public offers, preserve actual original capabilities
and bind revisions; names are not security identities. The selected token
request is an amount inside the already-held project allocation, not another
charge against the full parent. Refuse amounts outside actual native headroom.

Assigned administrator is the only business approver. Server admin, resource
owner, coordinator or a room moderator acting solely under that other role is
rejected. Requester/admin equality requires an explicit project policy; do not
reuse the unrelated owner-grant coordinator self-approval flag. Proposed initial
policy is deny self-approval. This is a policy field to review before enabling,
not an assumption that assigning a manager implicitly approves its own agents.

State separates `pending_admin`, `rejected`, `expired`, `approved_waiting_executor`,
`reserved_waiting_provision`, `provisioning`, `active`, `refused`, `outcome_unknown`,
`revoking`, `cleanup_unknown`, and `retired`. Exact native reservation, permanent
human-owned Matrix identity, real Room/DM readiness and active shared assignment
are all prerequisites to `active`. An old card reads the latest state.

Native command freezes request/decision/admin revisions, human owner/requester,
project/setup/binding digests, exact project reserve receipt, native enrollment,
agent definition, tokens and period. A non-deserializable authority adapter
validates the central assignment/decision and native owner resource ACL; it
reuses reservation/receipt/provisioning mechanics without forging legacy
`Registration`, `ProjectGrant` or coordinator actor. Provider/workspace selection
is trusted owner setup, not a new human approval for each requested agent.

Do not create a second ledger: the new agent reservation is a child of the
existing same-ledger project reservation, which is a child of the parent grant.
Today project children explicitly cannot execute. Native must add the execution
mode/attachment proof and real usage/stop/settlement accounting before enabling
agent attachment. Parent zero-retained cleanup must no longer apply to a child
that can execute or has unresolved dispatch. All unknown usage remains held.

## Shared execution cannot weaken the personal path

Current invariant in `domain_schema.sql`:

```text
agents(owner_user_id, execution_device_id)
  FOREIGN KEY(execution_device_id, owner_user_id) → devices(id,user_id)
```

Keep it unchanged. Keep the human owner immutable under `retain_agent_scope`.
Add an explicit immutable `execution_kind` discriminator (`personal` by default,
`shared_project` only for the new path). Shared identities have null personal
`execution_device_id`; the ordinary device setter/lease API must reject them,
including after their grant is revoked. No silent conversion or personal fallback.

`management_shared_assignments` minimally binds:

```text
id, agent_id, human_owner_user_id, plan_id, domain_project_id,
allocation_id, native_agent_reservation_id, native_receipt_digest,
resource_owner_user_id, executor_device_id, executor_generation,
enrollment_id, grant_id, assignment_generation, state, expires_at_ms
```

Composite FK `(agent_id,human_owner_user_id) → agents(id,owner_user_id)` and
`(executor_device_id,resource_owner_user_id) → devices(id,user_id)` enforce the
two different people. Composite/project/allocation consistency must be enforced
by foreign keys or immutable scope triggers, not caller-provided matching strings.
One current assignment per agent. An allowed-binding table pins exact project
Room, binding and generation; owner DM needs an explicit additional exact scope,
not blanket access to every private room owned by that person.

Use a **non-deserializable execution authority** with two verified variants:
`Personal{humanOwner,device}` and `Shared{humanOwner,executorPrincipal,assignment,
binding,allocationReceipt}`. Resolve human owner from storage, never substitute
the resource owner into `Principal` or change `p.user_id` to reuse methods.
Existing personal checks keep their exact semantics. Extract common state-machine
logic below verified authority, with separate typed public shared routes.

Recommended transport storage uses a distinct `shared_execution_leases` table,
keeping the current personal lease FK/triggers intact. Shared leases pin
assignment/binding generation, executor identity/generation, global agent epoch,
expiry and exact history witness. Extend event/reply/processing scopes with
nullable assignment-generation pins; personal rows require null, shared rows
require the exact assignment. A single trusted Rust/SQL scope validator chooses
the verified mode; do not sprinkle permissive `owner OR coordinator` predicates.
Both implementations must enforce mutually exclusive lease mode and generation
fencing, and preserve existing immutable execution/transaction identities.

All of these paths must change together before enabling shared execution:

| Path | Required shared check |
| --- | --- |
| Lease acquire/renew/release and takeover | Current exact assignment/enrollment, allocation and real executor session; original history witness. No automatic takeover of uncertain work. |
| Candidate selection, event read/claim/acknowledge/start | Exact assigned binding plus requester/owner membership, policies, TTL, generation and current native capacity; no access to unrelated owner events. |
| Tool execution | Recheck assignment and lease immediately before the tool; retain native workspace/permission approvals. No replay entitlement from a history receipt. |
| Reply submit/reconcile/worker send and processing reaction | Recheck both human-owner/Room visibility and executor assignment, including SQL worker queries/triggers; retain exact Matrix transaction identity and unknown sends. |
| Execution history/recovery | Executor sees only its exact assigned binding history needed to reconcile; owner retains its history. Preserve permanent started-execution witnesses and fail on missing local accounting. |
| Configuration delivery | Separate manager choices from resource-owner secrets/workspace consent. Existing `VALID_PINS` is owner-device-only and must not be widened to arbitrary provider/filesystem controls. |
| Pause, stop, revoke, retirement and cleanup | Fence new work centrally before enqueueing cleanup; resource owner may withdraw capacity, assigned admin may revoke project agent admission, human owner may retire identity. None transfers identity or assumes an offline process stopped. |

Concrete server call sites: `execution_devices::require_assigned`,
`TransportStore::{agent_active,lease,acquire_lease,dispatch,dispatch_scope,
claim_event,acknowledge,start_execution,authorize_tool_execution,submit_reply,
reconcile_known_reply,claim_reply,invalidate_stale_dispatches}`, `processing.rs`,
`transport_history.rs`, `execution_configuration.rs::VALID_PINS`,
`transport_schema.sql`, `processing_schema.sql`, and membership/cleanup workers.
`DomainStore::suspend_agent`, `leave_binding`, `retire_agent` remain owner controls;
new assigned-admin/resource-owner controls reduce only their explicitly scoped
shared authority, with visible asynchronous stop/settlement status.

Agent identity allocation must be an internal transaction consuming a verified
accepted request, not a call to `create_agent` under the admin/resource owner's
credentials. Reuse immutable identity generation, audit/idempotency and trusted
Matrix provisioning workers. Preserve existing personal IDs, devices, bindings,
history, budgets and messages through the migration with `execution_kind=personal`.

## Acceptance and staging

P1 is implemented on the server behind its disabled gate. It can be integrated without pretending
shared agents run: an actual reserved project gets real manager-owned Matrix
rooms, verified linkage and an assigned administrator, and the UI explicitly
shows agent setup unavailable until the next supported capability arrives.
The existing resource identity reconciliation/native enrollment prerequisites
still apply; synthetic receipts are insufficient for product activation.

Required P1 tests before enabling its UI:

1. Real PostgreSQL v5→v6 migration plus injected failure preserves v5 grants,
   settlement receipts and all personal project/agent rows. Restart is idempotent.
2. Wrong actor/admin, stale allocation/admin/setup revision, revoked contribution,
   expired grant, parent closing, wrong executor generation and false client
   Matrix facts fail without domain rows or outbox side effects.
3. Actual Matrix create/adopt through the manager's native session; server reads
   real Space, child, members and service permissions. Pending invite, encryption,
   foreign marker, wrong creator and room already linked elsewhere stay blocked.
4. Crash/lost response after Space creation, child creation, linking and central
   commit recovers original IDs. Unknown POST is not retried. Account switch
   cannot publish another account's result. Closing the mini app keeps the
   durable setup visible and resumable.
5. Bind-versus-management-link race cannot bypass admission. Existing bindings
   prevent initial adoption; managed binding/worker activation fails until a
   later exact admission. Existing personal tests remain unchanged and pass.
6. Resource/project revoke versus setup confirmation never leaves effective
   readiness; preserve created rooms as visible partial work, not deleted or
   silently recreated. Outbox and receipt failures roll back the central link.

P2/P3 execution acceptance additionally needs different real manager/admin/
resource-owner accounts, real native provider work, project Room and owner DM,
top-up, stop/revoke, exact reply/usage receipts and retained unknown liability.
Test authority changes at every boundary above, including queued replies and
tools, not just request creation. Test direct SQL constraint violations and
cross-profile/cross-device takeover. Verify original-command recovery after
crashes before/after native reservation, Matrix identity/join, provider execution,
reply and settlement. No second human Hagency approval may appear.

UI must retain full theme/i18n/window conventions and show approval, reservation,
Matrix readiness, running process and cleanup as distinct actual states. Update
parity inventory only with named production mini-app and protocol evidence.

The server P1 implementation is validated behind `enable_project_setup=false`,
without a session capability advertisement. It stores `awaiting_matrix_setup` and
`ready`; native creation journal stages stay native rather than being fabricated
centrally. Every confirmation uses one trusted state response per Matrix room.
`matrixReady` expires after 30 seconds and is not an agent execution permission.

Administrator replacement is supported only before setup begins. Once pinned,
expired capacity or an inactive assigned administrator blocks progress; P1 has no
cancel/rebase/reassignment protocol. This is an explicit product activation
blocker alongside native owner-created Matrix transport and resource identity
reconciliation. A future recovery protocol must preserve partial Matrix IDs and
original uncertain POST commands; it must not silently restart setup.

The server tests use actual PostgreSQL and HTTP with controlled trusted Gateway
responses. They do not establish actual native Matrix creation or live service
acceptance. Runtime deployment remains unchanged.

## Server P1 validation checkpoint (2026-10-16)

- Actual isolated PostgreSQL/HTTP suite: **133 passed, 0 failed, 0 ignored**
  (60.28 seconds), `.run/miniapp-setup-postgres-4.log`, via
  `HAGENCY_TEST_POSTGRES_CONTAINER=hagency-latest-test-postgres-1 python3 scripts/test-agent-service-postgres.py`.
- Eleven setup tests cover closed routes, authority pinning, exact recovery and
  generation rotation, one trusted state read per room, invalid Matrix state,
  both generic binding routes, all registered managed child rooms, personal
  compatibility, binding/revocation races, capacity expiry/parent close/project
  revoke, command/outbox rollback and v5→v6 migration rollback/preservation.
- Strict Clippy for server and agent service, all targets with `-D warnings`,
  passed: `.run/miniapp-setup-clippy-2.log`. Formatting and diff checks passed.
- Gate remains false by default and is never advertised as a session feature.
  No live Matrix creation, native setup UI activation, shared-agent execution,
  deployment, runtime replacement, commit or merge is part of this checkpoint.
- `cargo build --locked --offline -p hagency-server` passed with the existing
  macOS debug-link `__eh_frame` size warning. Log:
  `.run/miniapp-setup-server-build-1.log`. SHA256 of `target/debug/hagency-server`:
  `9a612f8b9e99171f9f89eb7edb29f3e881a1e6ff010eb1fc7be8640161806349`.
  This schema6 binary has not been activated; schema migration failure rollback
  was tested, but this does not imply old schema5 binaries can run schema6.

## Bounded setup recovery extension (approved 2026-10-16)

The immutable creation descriptor remains the original setup ID, manager, name,
mode, `definitionDigest`, original allocation/admin fields, and both native
creation command IDs. Any verified Space, Room and domain project IDs are
permanent. Its Matrix markers continue to use that original definition, including
across administrator or capacity changes. Do not recompute markers or rewrite a
native creation request/journal.

An append-only `currentBinding` records the separate current authorization:
`{revision, allocationId, allocationReceiptDigest, projectRevision,
authorityRevision, administratorMxid}`. Fresh confirmation validates the original
creation markers, but requires **this current administrator** to be joined and
the current exact reservation/assignment to remain live. This separates unchanged
room identity from changed business authority. Existing native creation requests
may contain the old invite; invite the new administrator as a separate exact
reconciliation step instead of changing that old request.

New closed routes (same disabled setup gate):

- `POST project-setups/{id}/cancel {commandId, expectedRevision}`: only manager or
  configured server admin. Persist `cancelled`, increment setup revision and
  clear verification. Preserve rooms, markers, bindings and native holds. This
  cancels room setup, not the allocated capacity or the original approved plan.
- `POST project-setups/{id}/rebase {commandId, expectedRevision, allocationId,
  expectedProjectRevision, expectedAllocationRevision, expectedAuthorityRevision}`:
  manager only. Select an already executor-confirmed, live reservation belonging
  to the same approved plan and current active assigned administrator. Append a
  binding revision, set `awaiting_matrix_setup`, invalidate verification and
  advance setup revision. The original creation descriptor and IDs never change.
  No reservation/renewal/refund or Matrix write occurs here.
- Existing server-admin assignment may replace an administrator after begin;
  doing so advances setup revision, clears verification and leaves the previous
  current binding stale until explicit manager rebase. A cancelled setup stays
  cancelled. Coordinator and old/new assigned project admins cannot cancel or
  rebase merely because they have those roles.
- Exact recovery operations `project.setup.cancel` and `project.setup.rebase`
  return the latest projection, not renewed permission to execute Matrix writes.
  `canCancel`/`canRebase` are derived current action permissions, never evidence
  that a native creation command is unissued.

Rebase cannot resurrect a revoked plan, resource or grant. An expired/revoked
allocation requires a distinct existing live reservation for that same approved
plan; pending decisions and unknown native results never qualify. Resuming a
cancelled setup preserves its original native journal, including unknown POST
outcomes. Only the original installation's private creation custody may continue
an actually unissued stage; another installation may observe or confirm exact
existing rooms but cannot invent replacement commands.

Native partial/unknown IDs remain in that original journal. This server slice
cannot prove an unobserved Matrix POST did not happen and cannot clear it. The
capability remains unadvertised until a trusted native managed-journal adapter
supports this recovery rule. No shared-agent admission/execution is added.
