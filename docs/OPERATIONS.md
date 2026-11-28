# Hagency Agent ownership and message processing

## Resource-pool Agents (schema 24)

Desktop discovers this workflow through `resource-pool-agents-v1`. New Agents use a selected resource pool directly: no project, intermediate project reservation, or Matrix Space is created. Historical projects and immutable receipts remain available for recovery and cleanup; new management project requests are disabled.

1. The resource owner connects the executor, configures a resource, and obtains resource admission. The owner reviews and publishes a native capacity grant naming the Matrix accounts allowed to request Agents.
2. An applicant opens **Hagency → Resource pools**, browses pools from multiple owners, selects an eligible pool, and requests an Agent with a name, instructions, capability, and token ceiling.
3. Desktop creates an independent private Matrix conversation using the applicant's current account and invites the appservice. Room creation and Agent submission have durable command journals; uncertain responses recover the original operation.
4. Only the selected resource owner approves or rejects. Global administrators cannot approve on the owner's behalf. The original device then verifies and reserves capacity in its native ledger; approval alone does not mean a successful hold.
5. The server verifies the conversation and joins the Agent identity; the executor qualifies its runtime. The applicant can view progress, control automatic replies, and use the named conversation.
6. The applicant may remove the Agent; the resource owner may stop supply. Both stop new work and initiate cleanup. Consumed and unresolved usage remains retained until original evidence establishes its disposition.

Each Agent uses one pool. Published capacity is not a live free-balance claim. The former self-project-approval option is absent from new grants.

Validate with isolated accounts/devices/ledgers: two owners' pool visibility, selected-owner-only decisions, exact retries, insufficient balance, first and subsequent work, applicant removal, owner withdrawal, restart recovery, and populated schema upgrades. Do not publish real resource capacity merely to run a test.


[中文](OPERATIONS.zh-CN.md) · [Documentation](README.md)

The current architecture uses the required integrated Appservice, permanent
personal ownership and local execution. Fleet/Hafleet, Engagement, resource
allocation approvals, old native enrollment, authority import and Miniapp aliases
have been removed. Old business structures are neither accepted nor imported.
See the [cutover audit](../crates/agent-service/legacy-cutover.md).

## Component boundaries

| Component | Responsibility |
| --- | --- |
| Palpo | Existing Matrix users, Rooms, Spaces, events, client/federation APIs, Appservices and generic administration |
| Pasion | Personal accounts, OAuth/OIDC, authorization scopes, sessions, registration and administrator roles |
| `crates/agent-service` | New PostgreSQL identities/devices, permanent owner, creation policy, leases, owner events and reply outbox |
| `crates/backend` | One listener/process, required Appservice registration, trusted Matrix gateway/workers, browser auth/BFF |
| `crates/frontend` | Generic Palpo/Pasion administration plus Project/Agent ownership, bindings and creation policy |
| `chrislearn/hagency-client` | Codex execution, Room/user quotas, requester filtering, risky tool decisions and actual usage |

Palpo/Pasion default functionality and their database schemas/migrations remain
owned by those components. Hagency uses `hagency_agent_v1` in its separate database.
The former `hagency_admin_state` document is no longer loaded or migrated.

## Identity and creation rights

Managed projects use one **project manager** role as of management schema 22.
Approval of a project automatically establishes its manager as its project
authority. That same person sets up and verifies the Matrix workspace, requests
agents, confirms their admission and budget increases, and removes agents.
Confirmation remains explicit; no separate project administrator account or
assignment is required. The resource owner also coordinates capacity. Global server administration
retains its admission responsibilities.

Upgrades merge any separate project authority into the existing manager while
retaining the previous authority and immutable command/decision history.
Changed-authority setups require the manager to update their requirements and
verify membership in the original Matrix rooms. Dependent execution is fenced,
and unresolved capacity holds remain until cleanup completes. Legacy API fields
named `administrator` identify the manager; assigning a different person is
rejected. Session discovery advertises `project-manager-authority-v1`.

Management schema 23 automatically makes each resource owner its capacity
coordinator. Resource registration and native capacity review have no separate
coordinator selector. The owner approves project reservations under the existing
grant; the project manager approves agents within that reservation. Owners who
also manage a project remain subject to the grant's explicit self-approval setting.

Upgrades retain original grant receipts, accepted executor commands and usage
history. Current contribution authority and allocation reviewers become the
owner, with previous values retained in an immutable migration audit. Pending
notifications move to that owner. Old delegated receipts remain usable by the
owner without rewriting their digest or resetting capacity. Session discovery
advertises `resource-owner-coordinator-v1`. No separate coordinator account is
needed; legacy wire fields keep their names for saved-command compatibility.

When project reservations are enabled, session discovery advertises
`admission-bound-grants-v2` before the first capacity grant exists. Desktop uses
this capability to review and publish a version 2 native receipt bound to the
contribution's approved `admissionRequestId`. The enrolled owner device must
publish it; a different admission or executor is rejected. A grant cannot be
replaced for the same admission after revocation; exact retries recover the
existing result. Version 1 receipts retain their original serialization and
digests for compatibility. Publishing the first eligible grant creates capacity
approval requests for already-approved projects in the owner's inbox.

Users sign in with their own Matrix account through Pasion Authorization Code +
PKCE. Pasion introspection and real Matrix whoami establish issuer, subject,
client and MXID before granting short server authorization. Device credentials
are bound to a user session, generation, revocation and the real clock. A request
body cannot assert ownership.

An Agent's owner and puppet MXID never change. There is no transfer or ownership
deletion API. Agents are independent of Projects and can bind to multiple
Project/Room pairs, each with separate permission, lifecycle, generation and
routing scope. Local context and budgets must also remain scoped by binding.

Each Project maps to one Matrix Space. Rooms retain independent membership;
Space membership does not imply membership in every child Room. Creation checks
live owner membership in both, the actual Space-child relationship and the
service's invitation capability. Managers register existing Spaces/Rooms;
normal Matrix APIs continue to create discussions and manage their members.

Project policy allows members by default. Deny overrides default and explicit
allow. A Space manager may disable the default and maintain allow/deny lists,
with real Matrix administration checks and revision fencing. Creation denial
does not stop existing Agents. Administrator Project/Room pauses and owner
Agent/binding pauses are separate; the owner cannot bypass an administrator pause.

## Appservice and execution

The integrated Appservice is required. Users and local installations do not
create a separate Fleet or Appservice. A puppet in its reserved namespace needs
a durable permanent owner mapping; querying an unknown reserved name does not
create an ownerless account.

AS transactions are acknowledged after persistence; an offline client does not
block ACK. Trusted workers route canonical plaintext mentions and follow-ups in
registered threads. Fresh owner/requester/Room/Space facts and active
Agent/binding generations are checked for delivery and replies. Conflicting
replays fail and queues are bounded. For encrypted Rooms the executor holds a
Matrix device for the Agent and decrypts messages locally; the server routes
metadata and checks membership and assignment authority. Pooled Agents always
use this executor-device design, so the resource provider can access plaintext
and the Agent device token. Revocation prevents future authorized access but
cannot recover keys or plaintext already received. Compatible execution clients
and the corresponding management feature gates are required; these gates default
to off. Server-side tests do not by themselves qualify a live model run.

Personal Agents use the executor-device design by default. To retain the
server-brokered personal-Agent protocol, set
`personal_encrypted_delivery = "ciphertext_inbox"` in the Hagency host config.
It does not change pooled-Agent routing. Before changing modes, drain existing
personal encrypted work and update all server instances consistently; an event
is routed to only one personal-Agent delivery path.

One Agent has one active device execution lease. Epoch, device generation,
session expiry and revocation fence old authority. ACK records durable client
receipt; start and completion are separate states. Replaying the same execution
start must not run the model again. Lost running authority becomes unknown and
is not automatically executed by a replacement device. The server cannot undo
already executed local tools.

Replies enter a durable outbox with a stable Matrix transaction ID and immutable
payload. Unknown sends retain that transaction; no new attempt is invented.
Denied delivery authority prevents automatic sending after rejoining. Evidence
of a completed send must match the actual canonical event, transaction and
payload. Cross-epoch reconciliation of a known result must be explicit; it must
not relax ordinary submit fencing or release unknown usage/side-effect holds.
Availability of that operation depends on the current transport API/tests.

Explicit known-result recovery is implemented at
`/hagency/api/v1/execution/replies/reconcile-known`. Original `dispatchEpoch` stays
fixed; separate `deliveryEpoch` records current send authorization. Sent receipts
retain their original event ID. Pending/unknown retries retain the transaction
and payload; a delivery block cannot be cleared. See the
[transport contract](../crates/agent-service/src/transport-README.md).

## Browser and native APIs

BrowserAuth provides `/hagency/api/login/token`, `/hagency/api/session` and `/hagency/api/logout`, with
live identity/admin rechecks, cookie/CSRF/origin protection and OAuth handover.
It has no old business store.

`/hagency/api/browser/v1` is a closed management BFF. It uses the saved personal
OAuth grant to obtain a separately verified short authorization and revokes it
after the operation. Credentials are never returned to the browser. It allows
Project/Agent/binding operations only, without device execution or an arbitrary
HTTP proxy. Native `/hagency/api/v1` continues rejecting browser Cookie/Origin;
it provides Pasion proof, device and execution protocols. AS keeps Matrix AS APIs.

Browser pages are `/hagency/projects` and `/hagency/agents`. Local quotas, model
settings and tool policy are not submitted for server approval. Fleet/resource
approval/account-approval pages and aliases are gone.

## Validation

```sh
just check-agents
just check-agents-postgres
cargo test --locked -p hagency-server --lib browser_auth::tests
cargo clippy --locked -p hagency-server --all-targets -- -D warnings
cargo check --locked -p hagency-frontend --target wasm32-unknown-unknown
```

The PostgreSQL runner creates/removes a dedicated random database rather than
migrating existing business data. See `scripts/test-agent-integration.py` for
real deployment/PKCE/Matrix delivery checks and its actual arguments. Historical
Fleet validation records do not imply current support or compatibility.


Configure how long a verified sign-in is trusted with `authorization_window_ms`: 30 seconds by default, bounded to thirty seconds through two minutes. A session ends at most this long after its last proof, and every Agent execution lease ends with its session, so an owner computer that makes no request for longer than the window loses its Agents until it proves its sign-in again. Two ends do not follow the window: the activity lease of a process working on a message is thirty seconds whatever the window, so such a process loses its lease after twenty to thirty seconds of silence; and what is offered (a start command, a ticket, a poll hand-out, a challenge) keeps a fixed thirty seconds and is offered again when missed. A longer window is granted only to a client that asks for it with the `x-hagency-authorization-window-v2-ms` request header when it presents its proof; by sending that header a client says that it shares one local session among its calls, so a longer window costs it nothing. The earlier header, `x-hagency-authorization-window-ms`, is still accepted but gives thirty seconds whatever it asks, and when both are sent the second decides; other clients keep thirty seconds too. So a computer that sends only the earlier header runs at thirty seconds even under a longer configured window: Hagency Desktop 0.5.0-rc.19, and also any build that could use a longer window but does not yet send the second header. Such a computer returns to the longer window at its first proof after it sends the second header. The cost of a longer window is that a sign-in disabled or revoked at the identity service stays usable for up to that long; an explicit sign-out or device revocation on this host still takes effect at once. A renewal never shortens a session: after the window is lowered, each session keeps the end it already has and follows the new window from then on, and no Agent is withdrawn by the change. Agents are withdrawn when a session, device or user is revoked, or when what they were reserved under ends. That holds from migration `2026-11-26-000155` on: a host that has not run it yet withdraws every pool Agent of a computer whose session end becomes smaller, so upgrade before lowering the window, and do not run that migration's down step while pool Agents exist. Raise the window above thirty seconds only on a server that grants it through the second header alone: an earlier server grants it through the earlier header too, so to Hagency Desktop 0.5.0-rc.19, which makes one local session per call and then refuses its own calls. For the same reason, while a computer may run that release, lower the window to thirty seconds before moving the server back to a binary from before the second header. The linked document gives the order and the way back. Every time limit and what it is derived from is listed in [Time limits of Agent execution](design/hagency-time-limits.md).

Owner computers read the server's time from the `Date` header of its answers. Hagency Desktop takes it only from an answer that carries exactly one `Date`, `Cache-Control: no-store` (every API answer of this server does) and no `Age` other than 0. A proxy or load balancer in front of the server must therefore pass `Date` and `Cache-Control` through unchanged, must not add a non-zero `Age`, and must not serve API answers from a cache. Without a date a computer allows a sign-in proof no more than 31 seconds on its own clock: it refuses the proof of a session with a longer window (`authorization_time_window_unavailable`), and a computer whose clock is behind by more than a second refuses even a proof of the default window.

A withdrawn (fenced) Agent reservation is not given back, so each one leaves a record of when and why in `hagency_agent_v1.management_agent_fences`: `fenced_at_ms`, a `cause` of `dependency` (a row the reservation depends on changed: a revoked session or device, a closed grant, a withdrawn pool) or `service` (the service decided: a start that ran out of time, a cancelled request, a reservation its executor no longer holds), and `facts`, a JSON object with the state of those rows at that moment (`sessionRevoked`, `sessionEnded`, `deviceRevoked`, `deviceGenerationChanged`, `ownerActive`, `requestState`, `allocationState`, `grantState`, `poolGrantState`, `poolState`, `setupState`, `planState`, `projectActive`, `roomActive`, `startEnded`). The one that is not as it should be is the reason. Reservations fenced before this record existed have none.

An allocation or a grant keeps its state when its time runs out, so the record also says how long before the fence the reserve command, the allocation, the allocation's grant and the pool grant had ended (`reserveEndedMsAgo`, `allocationEndedMsAgo`, `grantEndedMsAgo`, `poolGrantEndedMsAgo`). A negative value means that time was still ahead, so it was not the reason; a pool Agent's later grants can carry its reservation past the end of its reserve command. A fence the service made names in `source` the part of the service that made it:

- `project_sweep`, `pool_sweep`: the sweep that runs on each call of the computer that holds the reservation found that its request, reserve command, allocation, grant, setup or authority had changed or ended, or its device or session had (for a pool Agent: its pool grant, or a successor reservation that was refused).
- `runtime_sweep`: the start of the Agent's process ran out of time (for a reservation that is not renewed each period).
- `runtime_readiness`: the start of the process, or the reservation, was no longer live, and nothing kept the Agent. `runtime_codex`: the contribution behind the Agent no longer offers the runtime it runs.
- `matrix_readiness`: the check before Matrix work found the reservation no longer live, for example because its time was over, and nothing kept the Agent. `matrix_membership`: its Matrix membership had been fenced. `matrix_identity`: the Agent, or its room and space, no longer matched the reservation. `matrix_observation`: its room could no longer be read as the project's or the pool's room. `matrix_pool_invitation`: the pool room's invitation of the service was refused.
- `executor_delivery`, `executor_result`, `executor_acknowledgement`, `assignment_read`: the computer was sent a command, reported a result, had it acknowledged, or read the assignment, for a reservation that nothing kept any more.
- `request_cancelled`: the manager cancelled the request.
- `ledger_replaced`: the computer's ledger was replaced.

A fence by a rule of the database has no `source`, and neither has one made by a server from before migration `2026-11-27-000158`. The down step of that migration keeps every record; while it is down, a new record has neither `source` nor the four times.

A fenced process of a pooled Agent leaves the same kind of record in `hagency_agent_v1.management_shared_runtime_fences`: `fenced_at_ms`, a `cause` of `dependency` (a rule of the database did it: the Agent was stopped, or its reservation was withdrawn) or `service` (the service did it: it found the lease over, or the time bound gave the process up), and `facts`. Where a reservation's record names in `source` the part of the service that fenced it, a process's record names in `stopSource` who ended the process: `computer` when its computer reported the stop, `time_bound` when the server's time bound gave it up, `server` for any other decision of the service or a rule of the database. Beside it, `released` counts the process's messages that were given back to the queue, and `releaseFailed` those that met the conditions for that and could not be given back (these three from migration `2026-11-27-000159` on). Its other facts are `stateBefore`, `stopped`, `leaseEndedMsAgo` (how long before the fence the activity lease had ended; negative when the lease still had time, so the lease was not the reason), `startWindowEndedMsAgo`, `controlMode`, `assignmentState`, `reservationFenced`, and how many messages the process had been given that never began (`workNeverStarted`) or whose outcome is unknown (`workUnknown`). Read it to tell which limit ended a run. The down step of migration `2026-11-26-000156` stops new records of both kinds and keeps the existing ones.

A message whose process ended before its turn began is given back to the queue and offered again (rule 3 of [Time limits of Agent execution](design/hagency-time-limits.md)) only when `enable_shared_demand_reoffers` is set under `[management]`. It is off by default and takes effect only together with the demand retry switches, which keep working when it is off. Switch it on only after a release that reads the computers' `reoffers` field has been in service. A message is given back only while its Agent can serve it, by the same rule by which the chat status offers its sender to send a message again (the reservation stands and its time is not over, the executor's ledger was not replaced, the Agent runs and is a verified member of the room; `shared_agent_serves()`), and only while no budget was applied to the Agent; a Stop gives nothing back. While it is on, discovery lists `shared-demand-retry-v2`, and the service logs, by ids and reasons only, each message given back (`message given back to the queue at its process's fence`) and each one that could not be: a warning, `message could not be given back to the queue; it ends as before`, with the message's `demand` and `work` ids, its process (`runtime`) and the SQLSTATE (`kind` `release_failed`; when the search for such messages failed, `release_search_failed` with the process only). Such a message then ends as before, and the fence record of its process counts it in `releaseFailed`. On or off, it also logs each message given back that the queue later ends without its turn (`message that was given back to the queue ended`, with the reason from its block: `retry_lapsed`, `retry_exhausted`, `reoffer_unsupported`, `retry_unsupported` or `head_changed`); one that is stopped or expires ends as any waiting message does, without this line. To go back: switch it off and restart the service; wait until no message given back is still waiting, that is until this query returns 0 (each one ends within minutes):

```sql
SELECT count(*) FROM hagency_agent_v1.management_shared_intake q
WHERE q.state = 'pending' AND EXISTS (
  SELECT 1 FROM hagency_agent_v1.management_shared_demands d
  JOIN hagency_agent_v1.management_shared_demand_released_bindings x ON x.demand_id = d.id
  WHERE d.intake_id = q.id);
```

Then move the binary back, to the release that reads `reoffers` or a later one; going further back needs every demand start that carried `reoffers` finished, because a computer sends a saved start again with its original bytes and older releases refuse the field. Run the down step of `2026-11-27-000159` only after the binary went back, if at all, because the release applies that migration again when it starts. The down step removes the trigger that gives messages back, restores the guards it relaxed and the earlier fence record, and keeps `management_shared_demand_released_bindings` with its guards and functions. Without it an older binary gives nothing back, because only this release sets the session setting the trigger needs (`hagency.shared_demand_reoffers`).

To find a message that could not be given back later, for example once the log has rotated, read the fence records that counted one in `releaseFailed`. Each row is a message that the ended process still holds and that never began, by ids only:

```sql
SELECT f.runtime_id, f.fenced_at_ms, x.demand_id, x.work_id, q.event_id
FROM hagency_agent_v1.management_shared_runtime_fences f
JOIN hagency_agent_v1.management_shared_demand_bindings x ON x.runtime_id = f.runtime_id
JOIN hagency_agent_v1.management_shared_work w ON w.id = x.work_id
JOIN hagency_agent_v1.management_shared_demands d ON d.id = x.demand_id
JOIN hagency_agent_v1.management_shared_intake q ON q.id = d.intake_id
WHERE (f.facts::jsonb ->> 'releaseFailed')::bigint > 0
  AND w.started_at_ms IS NULL;
```

Configure unstarted offline request age with `queue.event_ttl_ms`: 24 hours by default, bounded to one second through thirty days, measured from trusted AS receipt time. Routing retries do not reset it. It does not apply to a message to a pool Agent, which waits a fixed 24 hours (see [Time limits of Agent execution](design/hagency-time-limits.md)). Expired requests cannot be polled, acknowledged or start new model/tool actions; known original results of started work still require current authority for settlement and delivery. Unknown work and usage holds are not released automatically. Device poll requires `bindingId` for the explicitly started Room. Observed owner membership/link loss or removal of the puppet moves the binding through leaving to confirmed left, advances generation and revokes old dispatches. Suspended bindings are also reconciled. Matrix observation failures retain pending state; confirmed loss triggers durable departure, not automatic rejoin. Restore real authorization and explicitly rebind/re-adopt after confirmed left. Ordinary service pause remains separately resumable. Sending also checks the puppet's current `m.room.message` power level; replies blocked by observed revocation do not automatically send after permission restoration.

## Worker health and shutdown

`/readyz` remains the startup AS-roundtrip latch. Containers use
`/workers-healthz`, which also requires live queues. Three consecutive failed
passes, unexpected queue exit or 60 seconds without activity return 503;
successful idle passes remain healthy. This measures queue operation and observed
failures, not active probes of every external dependency. Aggregate responses
contain queue names, fixed error codes and ages, without identities or content.
The `membership` queue's pass also gives up pooled processes that stayed
silent past their grace. One it cannot give up does not hold back the others: it
is logged (`Shared attempt time bound failed`, with its runtime id and error
code), the pass counts as failed (`shared_attempt_timeouts_failed`), and the
next pass tries it again. A computer's attempt polls first give up that
computer's silent processes the same way, inside their own transaction: one
they cannot give up is logged with the same line and rolled back alone, and
the poll goes on.

SIGTERM stops new work admission, keeps the internal Matrix listener alive for
up to 35 seconds while claimed uploads/sends save their receipts, then stops
HTTP and Pasion. Compose allows 75 seconds. Timed-out work remains unknown and
is never automatically retried. Stop old writers before the chunk-storage and
Board v7 upgrades; mixed old/new writers are unsupported. Large histories need
a maintenance window for the transactional migration.
Final send-claim admission is checked after database and identity row-lock waits.
An admission minted before shutdown belongs to the drain even if its transaction
commits later; a waiter that has not passed this point cannot acquire send custody.

File capacity counts at most 4096 unsettled deliveries and 128 MiB per owner;
sent/cancelled records retain permanent witnesses without consuming active quota.
Unknown records still consume both quotas. Removing a Board attachment retains
its identity, original size, SHA-256 and receipt, clears its payload and frees
the active 20-file/40-MiB allowance. Physical reclamation follows normal VACUUM.

Browser login budgets use the direct socket IP, ignoring untrusted forwarding
headers. Users behind one reverse proxy share that peer's budget; configure
appropriate proxy-side admission for the deployment. Do not trust arbitrary
client-supplied IP headers. See [dependency security](DEPENDENCY_SECURITY.md)
for the remaining upstream risks and the expiring CI review policy.

BFF reads and writes both require the original CSRF token; `/session` remains
the Cookie restoration endpoint. Hagency logout revokes the original session
and token, without issuing an unbound Pasion Cookie SSO logout. SSO can be ended
separately on the Pasion account page. A revoked HttpOnly session ID may remain
until expiry or the next binding, but the server immediately refuses it; a late
logout response does not overwrite a newer login Cookie.
