# Next slice: executor-confirmed project capacity

Date: 2026-10-16. Status: central protocol implemented behind a default-off gate; native integration and full UI acceptance are unfinished.

The current `ProjectPlan` records server-administrator approval and reports
`awaiting_allocation`. The next complete slice ends when the resource executor
has durably reserved the approved amount and the central server has recorded that
exact receipt. It includes rejection, cancellation, revocation and recovery. It
does not claim Matrix project readiness, agent creation or provider execution.

The accepted [management contract](../../../hagency-desktop/docs/design/hagency-management-contract.md)
already decides the roles: the administrator admits a project, the resource
owner delegates capacity, and the delegated coordinator makes the capacity
decision. The executor enforces that decision automatically. Do not ask the
resource owner to approve the same project allocation again. The administrator's
role alone does not grant another person's resource authority.

The user's explicit agent-approval requirement remains controlling: **once
project capacity is allocated, agent creation is approved by the administrator
assigned to that project**. The resource coordinator decides project capacity;
that does not make them the approver of each agent using it. An executor performs
capacity and policy checks automatically, without another human resource-owner
or coordinator approval.

## What existing code does and does not provide

The [native integration review](../../../hagency-client/docs/management-allocation-native-next.md)
lists the exact worker methods and consent boundaries. Relevant source is
`hagency-client/native/hagency-store/src/domain/coordinator.rs`, its child modules,
and pinned `palpo_hagency_contract` at `d1cea837b0e559f7a26091874ec52687d279dbe3`.

| Existing type or method | Reuse | Required adaptation |
| --- | --- | --- |
| `ServerEngagement` | Server/owner/coordinator identity, registration and delegation revisions, expiration, self-approval policy | Current enrollment is a native management connection, not a legacy fleet registration. |
| `DelegationChange`, `DelegationCommand`, `change_coordinator` | Owner-gated delegation, exact expected revision, bounded expiry and revocation | Bind the new enrollment and retain the non-deserializable native gate. A coordinator name in `ContributionInput` is not an owner-approved delegation. |
| `ResourceGrant`, `contribute_resource`, `put_resource_transaction` | Parent resource/account ceiling, eligible managers, period, reservation and publication | Use the current native owner/provider scope; never expose ungated `put_coordinator_resource` to Splash. |
| `ProjectApproval`, `ProjectGrant`, `approve_coordinator_project` | Frozen definition and delegated actor checks | These authorize access to resource allocations; **they contain no per-project token reservation**. They cannot implement `ProjectPlan.requestedTokens` unchanged. |
| `coordinator_commands`, `coordinator_publications`, `coordinator_updates`, `acknowledge_coordinator_updates` | Atomic receipts, canonical digests, exact publication acknowledgements | Add the new reservation operation and management enrollment proof; preserve old receipt handling. |
| `ProjectSetupCommand`, `ProjectReadiness` | Later setup recovery and actual Matrix observations | Existing project approval needs actual project/owner-room IDs. A plan has neither; do not fabricate them to obtain a reservation. |

Two schema constraints make an adapter-only implementation incorrect:

1. `coordinator_engagements.id` references legacy `registrations.fleet_id`.
   `current()` requires a matching registration generation and reception room.
   `Registration::validate` requires an `hf_` plus 32 hexadecimal fleet ID and its
   representative Matrix ID. A `con_*` connection is not that registration.
2. `coordinator_resources` has a unique key on
   `(engagement_id, resource_id, period, period_key)`. Creating a separate
   `ResourceAllocationId` for each project against the same engagement/resource
   cannot work under this schema. Matching a label or using a UI convention would
   not enforce project isolation either.

Extract shared ledger operations behind **two explicit trusted authority types**:
legacy Matrix registration proof and native management enrollment proof. Keep
legacy registration validation and Matrix readiness checks intact. Migrate the
engagement binding to a discriminated authority record with checked foreign keys
to the appropriate enrollment, rather than inserting synthetic legacy records.
Both adapters use the same resource/account ledger and writer transaction.

## Owner grant before coordinator allocation

Add a native “Share capacity” step for the admitted resource. The owner chooses
the coordinator, eligible manager IDs, monthly capacity ceiling, delegation
expiry and explicit self-approval policy. The trusted host supplies the current
account, package, actual resource revision and enrolled device. No script field
can assert provider readiness or act as a resource owner.

The owner command atomically saves the native enrollment/delegation, parent
`ResourceGrant` reservation and a publishable receipt. Publish it to the central
server using the enrolled owner's authenticated native transport. A lost reply
keeps the local reservation and retries the same receipt; it never creates a
second grant. Server admission/verification alone must not fill in these missing
owner decisions. Existing `allow_self_approval` policy remains explicit; an
absent choice grants no self-approval.

The implemented `management_resource_grants` stores: stable grant ID, contribution ID,
native management enrollment ID, owner and executor device/generation, native resource and
configuration revision, delegation revision/coordinator/expiry/self-approval
policy, eligible managers, grant revision, amount, period/key, immutable owner
command ID and receipt digest, current state and observation time. The server
derives owner/device/contribution bindings from authentication and stored records.
It accepts the report only from that contribution's enrolled executor. This is
an authenticated owner report, not hardware attestation or a central ledger.

For this slice, support a fixed **UTC calendar month** only, matching the native
`period()` calculation. Reject daily resources for a monthly project request.
Freeze `period: monthly`, `periodKey: YYYY-MM`, start and end instants in the
command. An expired month is not silently reset or renewed. Renewal needs its own
explicit standing policy or later allocation decision; display this limitation.

## Reserve projects within the existing ledger

Add `coordinator_project_reservations` as children of `coordinator_resources`,
inside the same SQLite database and serialized writer. Required fields:

- Reservation ID; native enrollment/engagement ID; parent resource allocation ID.
- Central plan ID and approved revision; permanent manager ID; allocation request
  and decision IDs; canonical command ID/digest.
- Delegation/resource/grant revisions and executor enrollment generation.
- Monthly period/key; allocated amount; retained amount; state; ledger revision.
- Immutable result receipt and pending publication identity/digest.

Allow one live reservation for `(plan, approved revision, resource grant, period)`.
Different command IDs cannot create duplicate live reservations for that tuple.
A later increase is an explicit revisioned delta, not another initial reserve.

The parent resource grant already reserves its full amount against the physical
resource/account. Reserving a project consumes part of that grant, not another
copy of the physical budget. Enforce, in one transaction:

`sum(project retained holds) + legacy agent holds outside those projects <= parent grant`

An agent allocated inside a reserved project consumes that project's unused hold;
it is not charged again as a separate project-level hold. Extend `held`,
`unused_grant`, `check_agent` and the existing parent commitment calculation so
legacy entry points cannot spend capacity already reserved for a new project.
Keep spent or unknown usage retained through retirement. Existing allocations
count during migration; they never disappear because a new feature is enabled.

No provider process starts while reserving. Workspace access remains a separate
trusted native choice and is required before eventual execution. A zero-agent
reservation can be released only after the ledger proves no child work exists;
future reservations with agents require the existing cleanup/settlement rules.

## Central records and exact transitions

Keep administrator admission in `management_projects`; add separate
`management_allocations` and durable executor commands/results. Add
`management_requests.kind = project_allocation` with a reservation/allocation
reference. Its deciding role is the current delegated coordinator, never the
existing administrator-only project request handler. New clients explicitly
request `includeAllocations: true`; older Inbox clients retain their current
contract. The server's `enable_project_reservations` setting defaults to false. Initial
`project-reservations-v1` discovery requires a live accepted native grant, whose
capability is `hagency.project_reservations.v1`. This proves protocol support,
not allocation or UI authority. Once related allocations exist, discovery remains
available for history/recovery after revocation; every new decision/delivery
still checks current authority. The first grant is bootstrapped by the trusted
native owner-consent flow, not by a feature-gated Splash request.

| Allocation state | Transition and durable evidence |
| --- | --- |
| No allocation (`null`) | Read the project plan first. An approved plan with no applicable accepted grant awaits an owner grant; unapproved/rejected plans are not awaiting capacity. No capacity claim. |
| `pending_coordinator` | Server freezes plan/request revision, current owner grant, exact amount and period; creates one coordinator Inbox request. |
| `approved_waiting_executor` | Coordinator approves exact frozen request. Decision, audit and executor outbox command commit in one PostgreSQL transaction. |
| `reserved` | Exact executor reservation receipt is committed centrally; includes ledger revision, amount, period and observation time. |
| `refused` | Executor commits a terminal refusal such as insufficient capacity or changed authority; no reservation was made. |
| `outcome_unknown` | Delivery may have applied but no matching result is known. Retain the same command and possible hold; recover rather than resubmit. |
| `releasing` | Project/resource/delegation revoked or manager cancelled; server denies new work immediately and queues an idempotent release. |
| `release_blocked` | Executor refuses cleanup (for example unsettled usage). Preserve the last applied balance; retry cleanup with a new command. |
| `released` | Executor confirms safe release/settlement; retain historical receipts and any reported spent/retained amount. Never infer a full refund from the state name. |
| `rejected` / `expired` | Pending coordinator action declined or expired before an executable delivery. An already issued uncertain command still requires reconciliation. |

Administrator approval may automatically create the coordinator request once an
eligible grant exists; the manager need not submit the same request twice.
If policy forbids requester self-approval and the selected coordinator is also
the requester, show the policy conflict. Do not silently route it to an admin or
the resource owner as an alternative approver.

The first allocation decision approves or rejects the exact requested amount.
Changing amount, resource, manager or period requires a new frozen revision; a
coordinator cannot silently alter the manager's request. Unknown attempts must first be resolved by their original receipt. The initial
central slice keeps reservation refusals terminal; an automatic retry operation
under the same frozen human decision is still required before exposing retry in
UI. It must not introduce another human coordinator/owner vote. Release refusals
already support a new idempotent cleanup attempt; known retained holds remain
visible until an applied release receipt proves their new balance.

## Implemented central routes and closed bodies

All paths are under `/hagency/api/v1/management`. Actor, homeserver, device identity,
canonical payload and target executor are supplied by trusted adapters.

| Route | Body / authority / result |
| --- | --- |
| `POST contributions/{id}/grants` | Native owner publication: `commandId`, `expectedContributionRevision`, typed native grant receipt. Exact enrolled owner device only. Returns accepted current grant and receipt identity. |
| `POST grants/{id}/revoke` | `commandId`, `expectedRevision`; resource owner. Denies new allocations immediately and queues cleanup. Owner's local command uses the existing native publication gate. |
| `GET grants/{id}` | Related resource owner, coordinator or administrator; accepted receipt/history. |
| `GET projects/{id}/allocation` | Related manager, owner, coordinator or administrator; latest state and bounded public receipt projection. |
| `POST allocation-requests/{id}/decide` | `commandId`, `expectedRevision`, `decision: approve\|reject`; current delegated coordinator. No body-supplied actor, grant or executor substitution. |
| `POST allocations/{id}/release` | `commandId`, `expectedRevision`; project manager, resource owner or designated server administrator within their scope. Returns `releasing` until native confirmation (or `released` immediately if no reserve command was ever created). Administrator project revocation invokes the same cleanup transition. |
| `POST executor-commands/poll` | Native transport only: bounded cursor/lease request; server selects work for exact owner/device/enrollment generation. No arbitrary target machine parameter. |
| `POST executor-commands/{id}/result` | Private lease token, original command digest, closed reservation/refusal receipt; native executor only. Records result and returns exact publication acknowledgement. |
| `POST executor-commands/{id}/recover` | Original command digest; same enrolled owner/installation under current authentication. Returns recorded result or a bounded **historical-result upload** ticket. It does not lease new execution or renew the frozen command. |
| `POST allocation-commands/{id}/recover` | Exact operation name; same account/installation command identity. Reads latest authorized result, never allocates. |

Do not overload the current `ManagementRequest` admission decision route,
`Admission` recovery payload or same-owner configuration channel. Add exact
mini-app operations/grants for allocation review and release. Poll/result APIs
are host-private; Splash cannot claim machine authority, select a URL or send an
arbitrary operation. Use the existing 16 KiB management command cap and 96 KiB page budget below the
native transport's 128 KiB ceiling. A serialized `NativeGrantReceipt` is limited
to 12 KiB, leaving space for publication/reserve command envelopes. Native must
apply this limit **before** committing the parent hold. Eligible managers are
sorted and unique (up to 64 when the byte bound fits); all token counts/revisions
are positive JavaScript-safe integers. The shared synthetic interoperability
fixture is `crates/agent-service/tests/fixtures/management-allocation-v1.json`.
`management_allocation_types.rs` is the exact closed wire definition.

The closed `AllocationCommand` freezes: protocol/version; command, decision
and request IDs/revisions; plan ID/revision and canonical definition digest;
server/enrollment, owner, manager, coordinator; delegation revision/expiry;
resource grant/configuration revisions; target device/enrollment generation;
reservation ID, amount, monthly period/key/start/end; issue/expiry instants.
The server constructs this from stored approval and grant records. The native
adapter compares it against locally accepted owner authority before giving a
non-deserializable trusted command to the writer. Reuse `CommandContext` and
validated identifier/number types where their meanings match; do not represent
device generation as an unrelated legacy registration without an explicit mapping.
Bind authority to the actual resource configuration hash, grant/delegation
revisions and executor enrollment generation. The current contribution's generic
revision also changes on connection verification; refreshing that observation
must not invalidate an unchanged owner grant or create a new allocation identity.
Its five-minute freshness display is not a permanent reservation lease.

## Delivery, recovery and revocation invariants

1. The native writer first resolves a matching immutable command receipt.
   Returning it after expiry reports history; it does not execute again. A
   conflicting digest is always rejected.
2. For a new command, check current native enrollment, delegation, eligible
   manager, project/request binding, resource/provider configuration, period,
   capacity and hard local policy inside the reservation transaction. Persist
   reservation, result and pending publication before acknowledging delivery.
3. A delivery lease is not authority or a reservation. Network timeout, session
   rotation or an empty poll is not proof that an earlier command did not run.
4. Central result recording verifies exact command/digest/target, then stores the
   receipt and state transition atomically. A revoked plan may accept historical
   application evidence, but remains `releasing`; that receipt cannot reactivate
   it. Request the corresponding release until confirmed.
5. Acknowledgement removes only the exact publication ID/digest. A stale ack must
   leave a newer native ledger publication queued. Repeated acknowledgements and
   repeated reserve/release deliveries are effect-free. Each newly recorded native
   mutation advances the shared ledger revision strictly; exact-command replay
   returns its historical revision. Central release rejects revisions at or below
   an already observed result/balance. A delayed reserve may supply a known hold
   during cleanup, but cannot replace a newer applied release balance. `result`
   is the latest receipt and `balance` is the last applied balance; refusal zeros
   never erase a known hold.
6. Reauthorization may recover a committed result from the same installation.
   It does not rewrite the original generation or transfer work to another
   machine. Resuming execution authority after generation change requires an
   explicit native owner rebind; unresolved old holds remain visible. A historical
   result-upload ticket can reconcile a receipt after its original delivery lease
   expired, but cannot cause an unrecorded reserve to execute or mark a revoked
   allocation active. Another installation cannot obtain this ticket.
   Cleanup commands intentionally have no policy-expiry cutoff: they can only
   retire/settle an existing hold, never create capacity. Their delivery ticket
   still expires after 30 seconds. Reserve commands retain their original finite
   deadline; recovery never renews it.
7. Server and local owner revocation fence new work independently. Reclaiming
   capacity waits for confirmed absence/termination/settlement; offline is not
   a successful release. Neither transport errors nor stale snapshots free tokens.

## Matrix setup and personal-agent boundary

After `reserved`, show “Capacity reserved — project setup pending.” Matrix
Space/Room provisioning and membership verification are the next independent
durable operation; preserve the actual manager identity and recover unknown room
creation outcomes before retrying. The legacy `ProjectDefinition` needs real room
IDs and cannot stand in for this step. Failed setup retains the allocation until
explicit safe cancellation or successful recovery.

Do not edit `agents.execution_device_id` to point at a contributor's device.
Its existing `(execution_device_id, owner_user_id)` foreign key and
`require_assigned` check enforce the personal-agent path. Future shared execution
needs a separate assignment binding agent owner, verified project/Room,
reservation, resource owner/executor, revisions, budget and constraints.
Claims, execution leases, tool authorization, replies, stop and cleanup must all
accept that explicit proof before shared execution is enabled. Updating only the
claim route would leave existing lease/reply SQL unsafe or unusable.

Agent approval must use a server-stored, revisioned **assigned project
administrator** role. Neither ordinary project-manager status, resource
coordination nor inferred Matrix room power can substitute for that assignment.
The same person may hold both roles, but the recorded decision must identify the
project-admin authority actually used. Within an already allocated project,
agent requests go to that administrator; the executor then checks the remaining
project reservation, owner grant and machine restrictions without a second human
vote. Increasing the project's total reserved capacity remains a separate
resource-coordinator decision. Tool-execution approval remains distinct from both.

The restored `approve_coordinated_agent` / legacy `AgentApproval` path currently
checks coordinator authority. Reuse its ledger, receipt and verification
mechanics through an explicit new project-admin authority adapter; do not invoke
it with a forged coordinator actor or retain its legacy coordinator requirement
as an additional human approval. Extend the shared contract and native checks to
bind the assigned project-admin revision and current project reservation before
exposing agent creation in the new management flow.

## Implementation order and acceptance

1. Extract native enrollment authority adapter and add project reservation children
   to the existing ledger; preserve legacy/account tests. Add owner grant commands
   with native custody and canonical receipt publication.
2. Add central grant/decision/outbox/result tables and routes; freeze actor and
   all revisions server-side. Implement private executor polling and reconciliation.
3. Add exact mini-app capabilities and role-aware owner/coordinator/manager screens;
   show requested, decided, actually reserved and observed quantities separately.
4. Validate on the existing isolated real server and native engine before exposing
   allocation as implemented. Required cases:

   - Two distinct server engagements on one account/resource compete for the last
     available capacity, including multiple executor processes and SQLite waits.
   - Two projects and legacy agents share one grant; project/agent holds are neither
     double-counted nor spendable by another project. Unknown/late usage stays held.
   - Kill after native commit but before reply, rotate authorization, restart and
     recover the exact reservation once. Corrupting a Splash draft cannot lose it.
   - Revoke/change delegation, manager eligibility, resource revision, device or
     period while work is queued; the old command cannot obtain new authority.
   - Revoke concurrently with reserve/result/ack; retain historical results and
     cleanup until confirmed. Old acknowledgements preserve newer publications.
   - Fault outbox/receipt writes: PostgreSQL decisions and SQLite reservations each
     roll back atomically with their own receipt/outbox, with no distributed success
     claim between commits.
   - Distinct owner/coordinator/manager/admin accounts: only delegated coordinator
     decides capacity, self-approval policy is enforced, and no executor human
     prompt repeats that decision. Personal-agent tests remain unchanged and pass.
   - For the subsequent agent slice, a separately assigned project administrator
     can approve an agent within reserved capacity; a coordinator without that
     assignment cannot. The executor applies or refuses automatically, with no
     second human approval. Role revocation fences queued agent approvals.
   - Actual Makepad-driven owner grant → admin admission → coordinator decision →
     executor receipt → reserved → release/recovery, including English/Chinese,
     light/dark, account switch, lost replies and closed mini-app windows.

No current project-intent test or restored legacy unit test proves this new
allocation protocol. Agent execution, monthly renewal, full workspace/provider
onboarding, Matrix setup and notification delivery remain separate acceptance
work after this slice.

## Activation dependencies and current limits

The running isolated project-intent server has not been replaced by this work.
The new central schema migrates v3 to v4 atomically, including on startup; the
allocation setting stays disabled until the native integration is verified.
Existing project/admission routes and old Inbox clients keep their contract.

Existing admitted contributions were created with per-profile native resource
IDs/revisions. The new canonical owner registry can assign different resource
identities or hashes. A real native grant cannot bind to those old contributions
until an explicit reconciliation/mapping (or a separately designed revisioned
resource amendment) proves they represent the same owner-approved resource.
The central publication route deliberately rejects mismatched resource IDs,
configuration hashes, enrolled devices/generations and coordinators. Do not
weaken that check, manufacture a legacy registration, or use labels as identity.

The server trusts the enrolled owner's authenticated native report; PostgreSQL
unit/HTTP tests use synthetic receipts and do not establish that provider capacity
was reserved. The native owner registry, quota account verification, profile ACL,
local owner consent and durable parent/child hold transaction must all complete
before enabling the feature. Grant changes, delegation changes and monthly
renewal require explicit later operations; an immutable active grant cannot be
silently edited. Central grant revocation fences new decisions and queues child
cleanup; local parent-grant settlement still belongs to the native ledger.

No allocation UI/native transport capabilities are exposed by this server change.
Matrix project setup, assigned-project-admin agent requests, shared execution,
provider/workspace onboarding, capacity retry under the existing decision and
real Matrix notification delivery remain separate acceptance work. Admission
approval is still intent-only. The existing `ProjectPlan.allocationStatus` remains
its compatibility intent projection; new clients must read the dedicated
`GET projects/{id}/allocation` response for actual reservation state and balance.
A `reserved` result reports a known native hold;
it does not establish room readiness or current execution authority.

## Central validation evidence

On 2026-10-16, the PostgreSQL-backed service suite passed **110 tests, zero
failures and zero ignored tests**, including 12 new allocation tests. The runner
created and dropped a dedicated database in `hagency-latest-test-postgres-1`;
no running application database was used. Command:

```sh
HAGENCY_TEST_POSTGRES_CONTAINER=hagency-latest-test-postgres-1 \
  python3 scripts/test-agent-service-postgres.py
```

The new cases cover owner/device/resource binding, closed HTTP bodies, coordinator
versus administrator authority, self-approval, expired or replaced authority,
exact command/result replay, lost results after device reauthorization, capacity
release with retained usage, contradictory/late ledger revisions, revoke races,
transactional outbox failure at publication/decision/result, and v3 migration
failure rollback plus existing project preservation. Shared canonical vectors
were independently compared with the native implementation.

Strict `cargo clippy --locked -p hagency-agent-service --all-targets -- -D warnings`,
`cargo fmt --all --check`, `git diff --check`, and
`cargo build --locked -p hagency-server` passed. The debug server link emits its
existing large `__eh_frame` warning. Local evidence:
`.run/miniapp-allocation-postgres-5.log`,
`.run/miniapp-allocation-clippy-3.log`, and
`.run/miniapp-allocation-server-build-2.log`.

These results validate central durability/authority and synthetic receipt
handling. They do not replace native resource enrollment, provider/account
conservation, actual mini-app end-to-end allocation or production acceptance.
