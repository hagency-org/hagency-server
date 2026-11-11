# Shared text work checkpoint (management schema 13)

This checkpoint adds central durable work, usage evidence and Matrix reply delivery to the previously validated schema 12 same-process execution/activity protocol. It is default disabled. No schema 13 backend is deployed by this work. Native actual-provider execution is a separate paired implementation; loopback tests use explicitly synthetic native receipts and do not prove a real model turn.

## Authority and lifetime

The project manager remains the permanent agent owner. Only the separately enrolled resource-owner installation receives a shared execution command. Personal owner/device foreign keys and personal work routes are unchanged. Project agent approval remains the assigned project administrator's decision; no additional coordinator or resource-owner human approval is introduced.

`codex_text_v1` is a first execution milestone, not a replacement for original coding/tool/workspace capabilities. An immutable original spawn command expires within 30 seconds and cannot be reissued from history. A qualified original process receives separate activity epochs, each at most 30 seconds and bounded by current authentication and original resource/project authority. Authenticated renewal preserves the same process and original generation. Expiry/revocation fences work; recovery never grants a replacement process.

Only canonical plaintext Matrix messages from the current project manager, explicitly mentioning the exact agent, enter this initial shared queue. Message edits, encrypted messages, stale events and unrelated senders do not become prompts. Before a prompt is returned or a start is acknowledged, current business authority and actual Matrix room permissions are checked before and after Matrix I/O. Gateway observation age uses the same host clock that stamped the observation; PostgreSQL time independently controls authority and lease deadlines. No clock-skew tolerance turns future or stale observations into current facts.

## Closed transport

All paths below follow `/hagency/api/v1/management/`. The exact closed DTOs are `management_shared_execution_types.rs` and `management_shared_work_types.rs`.

| Operation | Path | Behavior |
| --- | --- | --- |
| Poll | `POST shared-execution/work/poll` | Current activity authority; returns one exact item or null. |
| Acknowledge | `POST shared-execution/work/ack` | Native must first durably retain the original inbox item. |
| Start | `POST shared-execution/work/start` | Returns `newlyStarted` once. False or uncertain response never authorizes a model rerun. |
| Recover/upload ticket | `POST shared-execution/work/{id}/recover` | Exact enrolled installation, including newer generation for cleanup. Finite evidence-only ticket, recorded receipt and current work/reply state; no new prompt or execution authority. |
| Record result | `POST shared-execution/work/{id}/result` | Atomically retains exact native receipt, monotonic per-call usage and one immutable reply intent. |
| History index | `POST agent-work/list` | Current manager/assigned administrator only, requests with recorded work, project filter before pagination; independent of the admission gate. |
| History | `POST agent-requests/{id}/work/query` | Manager/current assigned administrator only; cursor and limit 1–16. No input/output, lease, provider identity or receipt material. |

`shared-work-history-v1` is advertised only when the explicit shared-execution policy is enabled. Discovery describes the read protocol, not runnable authority. History stays readable when issuance dependencies are disabled. With shared execution disabled the route returns the documented `shared_execution_unavailable` error; older servers may lack the route/feature entirely.

Canonical UTF-8 vectors: `tests/fixtures/management-shared-work-v1.json`. Existing version 1 reserve, metadata qualification, agent fence and schema 12 process command bytes remain unchanged.

## Usage and reply ordering

A work receipt is cumulative for one native call, identified by immutable work/execution/call/process bindings and a unique canonical ledger revision. A terminal provider event is not proof that usage is complete. Missing usage remains unknown; no zero usage or refunded capacity is inferred. Incomplete or unknown prior work blocks the next start. The final bounded reply must be committed with the original native result before upload, with serialized receipt at most 12 KiB; truncation cannot discard usage evidence.

Completed work creates one Matrix transaction `hagency_shared_{work_id}`. Every retry uses the same immutable transaction and threaded content. Lost replies remain unknown; a late successful send after revocation is recorded as the observed event. Revocation prevents a fresh send, but cannot unsend an already issued request. Native should retain the same process/activity while an authorized reply is pending; after reply resolution or authority loss, stop the exact process and retain cleanup evidence. Sending or retrying the reply never invokes another model turn.

Positive central usage is a floor for new agent fence, project release and parent settlement receipts. A late usage report is still accepted after older zero settlements. Original receipts remain unchanged; project `observedUsage` exposes the current retained floor, and parent status becomes `awaiting_reconciliation` with conservative held tokens until a new aggregate receipt incorporates the evidence. This may conservatively retain overlapping uncertainty; it never claims provider-wide free capacity or replaces the canonical native ledger.

## Persistence and compatibility

Migration 12→13 adds three retained tables: work, work receipts and reply intents. SQL guards bind work to exact manager/assignment/room, evidence to the original process and started work, projections to recorded receipts, and replies to original completed output. Native mutation revisions cannot collide with grant/allocation/parent/agent/process receipts. Cancellation and runtime fencing atomically retain cleanup and reply uncertainty.

`ProjectAllocation.observedUsage` is an additive optional projection. The paired native strict DTO must accept it before upgrading a running client/backend combination, even if the new execution flag stays off. Existing original receipt digests do not change. Do not activate schema 13 against an old strict allocation adapter without its paired update.

## Validation and remaining scope

Focused PostgreSQL/HTTP tests cover exact appservice ingestion, closed role-scoped executor routes, at-most-once start, durable result/reply atomicity, real loopback Matrix sends, same-transaction response-loss retry, in-flight revoke, monotonic late usage, both settlement arrival orders, schema rollback/reopen, generation-only cleanup, bounded history and canonical UTF-8 fixtures.

The frozen schema 13 checkpoint passed 30 focused shared-runtime/work tests and all 221 agent-service PostgreSQL/HTTP tests. Strict agent-service Clippy and the backend build passed. Exact source hashes, binary and logs are retained under `.run/shared-work-schema13-checkpoint/`; none of this activates the running backend. The debug linker reported an oversized unwind-section warning; the build completed.

The first full run retained 218 passes and one HTTP renewal 401 failure. Its original log is preserved. Inspection found that Matrix observations stamped by the application clock were being compared against PostgreSQL time; validation now uses the originating clock, with explicit stale/future regression coverage. The failed run did not capture the actual clock offset, so that individual 401 cannot conclusively be attributed to clock skew. The renewal test passed alone and in the final full run. A separate debug test-stack overflow exposed the growing dispatch future; boxing the existing management dispatch boundary fixed it without changing routes or authority.

## Separate discovery fairness checkpoint

Runtime discovery skips healthy qualified processes because they already have their own activity/work channel. Fresh eligible assignments and actionable original-runtime cleanup share one circular assignment order. The client advances `after` with the last returned runtime command ID; the server resolves it only within that owner and installation. An initial poll favors cleanup, while subsequent cursor advancement prevents an unresolved process from permanently hiding its siblings. Historical recovery and stop observation keep their original semantics; no replacement process or new command is created for a stopped assignment.

A three-agent PostgreSQL/loopback-Matrix regression first reproduced the defect: discovery returned the already-qualified first agent instead of the newly approved second agent. After the fix it also verifies idle discovery, an unknown original process, discovery of a third agent while cleanup remains unresolved, cursor wrap back to that cleanup, exact stop recovery, and exactly one original command per assignment. The follow-up full suite passed all 222 tests. Its separate artifact and logs are retained under `.run/shared-work-schema13-fairness-checkpoint/`; schema and closed transport bytes are unchanged.

## Separate exact-mention intake checkpoint

The intake query now filters the explicitly mentioned puppet Matrix IDs before ordering and applying its candidate limit. This prevents an earlier unmentioned room agent from hiding the intended agent. The production limit remains 32 targeted agents per event; this does not claim unlimited fan-out.

A real two-agent PostgreSQL/loopback-Matrix regression uses the same intake implementation with a test-only one-candidate limit. It first reproduced the missing target, then verified delivery only to the later mentioned agent and exact-event replay without duplicate work. All 32 focused shared-runtime/work tests passed in 137.19 seconds. The two source files and red/green logs are retained under `.run/shared-work-mention-checkpoint/`. This is a source-only follow-up to the frozen fairness artifact; the paired coordinator read API changes require their own combined suite before another backend artifact is frozen.

The native turn adapter, actual provider usage observation, original-process stop, and a real Matrix message→model→reply end-to-end run remain required before enabling this path. Coding/tools/workspace parity, top-ups, persistent agent successor/reassignment and other provider support remain named follow-up work; this text-only checkpoint does not remove them from product scope.
