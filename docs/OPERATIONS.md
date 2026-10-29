# Hagency Agent ownership and message processing

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
replays fail and queues are bounded. Encrypted Room execution is explicitly
rejected pending client crypto support; storing ciphertext is not decryption.

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


Configure unstarted offline request age with `queue.event_ttl_ms`: 24 hours by default, bounded to one second through thirty days, measured from trusted AS receipt time. Routing retries do not reset it. Expired requests cannot be polled, acknowledged or start new model/tool actions; known original results of started work still require current authority for settlement and delivery. Unknown work and usage holds are not released automatically. Device poll requires `bindingId` for the explicitly started Room. Observed owner membership/link loss or removal of the puppet moves the binding through leaving to confirmed left, advances generation and revokes old dispatches. Suspended bindings are also reconciled. Matrix observation failures retain pending state; confirmed loss triggers durable departure, not automatic rejoin. Restore real authorization and explicitly rebind/re-adopt after confirmed left. Ordinary service pause remains separately resumable. Sending also checks the puppet's current `m.room.message` power level; replies blocked by observed revocation do not automatically send after permission restoration.

## Worker health and shutdown

`/readyz` remains the startup AS-roundtrip latch. Containers use
`/workers-healthz`, which also requires live queues. Three consecutive failed
passes, unexpected queue exit or 60 seconds without activity return 503;
successful idle passes remain healthy. This measures queue operation and observed
failures, not active probes of every external dependency. Aggregate responses
contain queue names, fixed error codes and ages, without identities or content.

SIGTERM stops new work admission, keeps the internal Matrix listener alive for
up to 35 seconds while claimed uploads/sends save their receipts, then stops
HTTP and Pasion. Compose allows 75 seconds. Timed-out work remains unknown and
is never automatically retried. Stop old writers before the chunk-storage and
Board v7 upgrades; mixed old/new writers are unsupported. Large histories need
a maintenance window for the transactional migration.

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
