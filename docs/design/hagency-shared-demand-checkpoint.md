# Durable demand checkpoint B (schema17)

This isolated server checkpoint builds on frozen schema16 A. It adds paired native demand polling, an immutable original wrapper for one existing v2 start, and exact one-work promotion. It is default-disabled (`enable_shared_demands=false`) and is not activated or a claim of continuous-chat acceptance. Original reserve, v1/v2 process, work, usage and Matrix-reply contracts remain unchanged. Each successor currently starts a fresh provider thread and receives only the current work body: this is ordered automatic individual-turn execution, not contextual conversation memory. A separately versioned, bounded, same-agent/room/Matrix-thread context contract remains required.

`GET /hagency/api/v1/discovery` advertises `shared-durable-demand-v1` independently of issuance configuration. This describes protocol availability; fresh poll/start requires the existing execution/successor policies plus durable-intake and demand flags. Historical demand/work binding and explicit Stop remain available through authenticated original-device/role checks. Disabling B prevents linked fresh activity, work and Matrix reply sends; it does not erase evidence. Events already retained by intake cannot fall back to the legacy direct-delivery path when intake is disabled.

## Closed routes

All following paths have prefix `/hagency/api/v1/management/shared-execution`. The authenticated response also carries `observedAtMs`.

| Method/path | Exact input | Output |
| --- | --- | --- |
| GET `assignments/{id}/control/readiness` | no query/body | `{readiness:{control,canRun,canStop,requiresLocalStop,runBlockingReason,intakeEnabled,automaticDeliveryEnabled}}` |
| POST `demands/poll` | `{after?: assignmentId}` | `{demand: status|null,nextAfter: assignmentId|null}` |
| POST `demands/{id}/recover` | `{demandDigest}` | `{demand:status}` |
| POST `demands/{id}/start` | `{demandDigest,expectedControlRevision,start:SharedAttemptStartInput,retry?}` | `{delivery:SharedAttemptDelivery}` |
| POST `demands/{id}/retry` | `{demandDigest,expectedControlRevision,retry,attemptId,result:SharedAttemptResultInput}` | `{acknowledgement,demand:status}` |
| POST `demands/{id}/block` | `{demandDigest,expectedControlRevision,reason,budget}` | `{demand:status}` |
| POST `work/{id}/demand` | `{workDigest,runtimeCommandDigest}` | `{demand:null|{offer,demandDigest,binding}}` |

`status` contains the exact offer/digest, state, nullable runtimeCommandId, nullable binding and, once retried, `retry`. States are offered, start_committed, retry_offered, bound, cancelled, expired, head_changed, budget_blocked, usage_unrepresentable and blocked. They describe recorded progress, never fresh native authority. The block request supports budget_blocked with a valid zero-remaining aggregate or usage_unrepresentable with null budget. A current original executor can impose a restriction; this cannot add capacity or manufacture process evidence.

The canonical fixture is `crates/agent-service/tests/fixtures/management-shared-demand-v1.json`. Offer digest is `6ea2ab645922b8aa4c63037cef201c5f6b1c2b105247cada785e4e87d1c4102a`; start wrapper digest is `772e5ef67025b814c0e3d7334194e1e6dc4931f5e41c273dc2eaf85dff71c3dd`; binding digest is `0f4d60d472a05faa2ddab05246f3ce750be437eb6ae6a316235fe99115758e8f`.

## Custody and ordering

One immutable AS event/assignment becomes one intake and at most one demand. The first offer binds the original Run revision and exact predecessor forever. FIFO is receivedAtMs then demandId (equivalent to the original intake ID ordering). Poll examines at most16 current original-device assignments per call, returns a rotating assignment cursor even when selected entries cannot issue, and never creates a replacement intent. A blocked event keeps its own status and notice, and later events in that Run revision continue without it. Expired pending events are retained and do not loop or block the next eligible event.

The native device privately commits the exact demand-to-start mapping before POST. Demand-generated start intents must be excluded from generic manual-start recovery. The server cannot identify an unpublished native intent; after B commit the immutable wrapper, existing v2 attempt and runtime are one atomic transaction. An exact retry returns historical delivery only. Unknown replies, replacement body/ID, stale predecessor and Stop/Run ABA never create another attempt. Current exact owner/device/generation, business assignment, original grant, setup/admin and actual Matrix membership are checked before and after network observation.

An already qualified idle head can receive one queued event directly. Otherwise the demanded successor must qualify first. The existing work poll atomically commits the original event body/thread/receive time, unchanged work bytes, original event pin and separate demand binding. A one-turn runtime receives at most one work item; the next event remains assignment-scoped for a later stopped-head demand. Native validates the descriptive binding before inbox/ACK and recomputes its full canonical ledger hold before issuing the call. Central work/activity/reply checks retain the exact Run revision, including when B is subsequently disabled.

A manager or current assigned project administrator may Run or Stop. The exact original execution-device owner may Stop, not Run; resource-owner consent is general and no second per-agent or per-turn human approval is introduced. Existing migrated stopped assignments remain manual until intent is established. The control command response retains the original commandId/appliedRevision with a separate latest control projection: replay never reapplies Stop after a later Run. Native Stop uses a separate private outbound journal and a monotonic local restriction through the selected revision.

## Limits retained

The live accepted example remains 4,813 consumed/retained against4,096 allocated: remaining0 and overrun717. B retains budget_blocked and creates no successor, new allocation, top-up, refund or replacement agent. This is conservative admission, not a hard provider token cutoff. Unknown bounded usage remains held; unrepresentable usage blocks without a fabricated numeric budget.

A changed immutable offer head or unresolved native custody can require explicit resolution. Uncertain process issuance, start or stop is time-bounded (below). No automatic rebase or invented stopped receipt exists. Stop cancels pending events with recorded reason; old work and reply outcomes remain auditable/nonreplayable. Schema16 queue limits remain100 pending,24h expiry,10,000 retained rows per assignment. A reviewed archive/retention policy is still required before production; this checkpoint never discards audit to make room. Native paired transport/worker and actual multi-turn acceptance are separate qualification steps.

## Mini-app control display

The readiness read is scoped to the same manager/current assigned administrator/exact execution-device owner as control. No implicit server-admin or coordinator visibility is added. `canRun` reflects known current business authority, role, intake policy and a mode change; reasons are role_required, intake_disabled, assignment_authority_changed, already_running, or null. `canStop` remains available to the authorized roles when issuance is disabled. `automaticDeliveryEnabled` describes all required server policies, not device reachability, process state, provider health, remaining budget or context memory. The UI must display the central Run/paused/manual mode separately from the latest process observation and separately observed usage/device freshness. Actual Run always rechecks real Matrix membership. The original GET control and POST control shapes are unchanged.

Authenticated management session features include `shared-run-control-v1` independently of issuance policy. This advertises the scoped control/readiness/recovery/Stop protocol; it is not authority to Run. Older servers lack that feature. General discovery independently advertises the versioned native transport capability `shared-durable-demand-v1`.

`requiresLocalStop` identifies the exact original executor owner/device cleanup identity independently of role priority or issuance policy, including a later authorized generation. It remains true when that same actor is also manager/assigned administrator. Native must durably apply its owner Stop journal/fence first and must not fall back to a generic central POST if its local selection is missing. For an unrelated manager/administrator it is false; `canRun=false` alone must never infer owner identity.

## Uncertainty time bound (2026-11-06)

Owner policy: uncertain shared execution never blocks an Agent's queue indefinitely. This covers an attempt issued but never claimed, a claim whose process state is unknown, and an unconfirmed start or stop.

- Bound: 3 minutes (`UNCERTAIN_GRACE_MS`) after the later of `expires_at_ms` (which also bounds every execute ticket), the first activity lease (at most 30 s after the window) and later process evidence the server holds (activity renewals, work acknowledge/start/complete). Observe and cleanup tickets never extend it.
- Within the bound nothing changes: the attempt stays fenced and no second process can start for its work.
- After it, the server stops a v2/v3 attempt still `pending`, `issued`, `qualified` or `unknown` by itself, on every executor poll and in the background cleanup worker. It fences the attempt and sets `stopped`. It ends the demand that started it (`head_changed` for v2, a `head_changed` terminal for v3) only if the attempt reported a process; a v2 attempt that never did waits for the executor's evidence (see below). It writes no receipt and no usage.
- The executor then supplies explicit evidence on a cleanup ticket: `not_started` when nothing was claimed, or `stopped` with `reason: "uncertain_timeout"` and the claim's own process digest. Before the bound the server refuses the latter with `shared_attempt_timeout_pending`. A stopped attempt without such evidence stays discoverable with a cleanup ticket.
- The next attempt names that receipt as its predecessor stop (an empty `processDigest` after `not_started`). The interrupted event is not replayed; a rare duplicate or missing reply is accepted.
- v2 now accepts `not_started` for an attempt issued but never claimed, with the same fence proof as v3, and only on a cleanup ticket after `expires_at_ms`. The `validate_shared_runtime()` trigger, re-created at every start, accepts that v2 receipt and a v2 or v3 successor that names it. There is no schema version change.

## Retry of a never-started event (2026-11-06)

ADR-193 amendment, v2 demands only: an event whose attempt provably never started runs again in place, at most 3 times (`MAX_DEMAND_RETRIES`).

- Only the executor's explicit request re-opens an event: `POST demands/{id}/retry` carries the `not_started` evidence for the demand's latest attempt on that attempt's cleanup ticket. In one transaction the server records the receipt exactly as the attempt result route does and, if the event is still current, adds `management_shared_demand_retries` row `n`. The time bound never retries anything.
- The demand keeps its id, offer, event and FIFO place. Its status becomes `retry_offered` with `retry: n`. Retry `n`'s start (`demands/{id}/start` with `retry: n`) names the never-started attempt (empty `processDigest`) and is kept in `management_shared_demand_retry_starts`. A replay returns the same attempt.
- Bindings, `current()`, `allowed()` and the sweep treat a retry start like the original start. The binding validator is replaced to accept the latest retry start.
- A fourth `not_started` ends the event: `head_changed`, block payload `"retry_exhausted"`. A claim, a qualified process or any work evidence makes an event ineligible.
- A `not_started` receipt on the plain result route (older executors) ends the event as before. A never-qualified attempt stopped by the time bound keeps its demand open for the executor's evidence.
- Discovery advertises `shared-demand-retry-v1`. Wire additions are optional (`retry` on the start input and the status), and the new state and fields appear only after a retry request.
- Schema: two additive tables, their triggers and the replaced validator, applied by `management_shared_demand_retry_schema.sql` at every start. There is no version change; the previous binary ignores them.
- Matrix 429 responses map to `matrix_rate_limited` (retryable, 503); the delay and endpoint name are logged, never ids or bodies.
