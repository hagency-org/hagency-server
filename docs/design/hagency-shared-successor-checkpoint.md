# Restart a stopped shared agent without repeating its work

This checkpoint appends a fresh process attempt to the original approved agent.
It does not register another device, allocate another agent hold, repeat an
approval, move old work, or reinterpret an unknown process as stopped. Native
schema 81 and central management schema 14 are paired; both remain disabled
until their exact protocol and retained runtime adapters pass together.

## Bounded first path

The trusted native worker privately saves one start intent per accepted worker
startup or explicit agent-start action. Polling cannot invent intents. Retrying
that exact intent, including after a lost response or restart, retrieves the
same command. A distinct intent cannot race the same predecessor into a second
attempt. The manager's original request and assigned project administrator's
approval remain the business authority; there is no coordinator/owner approval
step for the successor.

The first successor requires a durable exact native observation that the
original process was actually stopped. Its command, process binding and stopped
receipt must match both ledgers. An issued spawn with no process observation,
missing PID, expired deadline, empty manager map, or crash-unknown custody cannot
qualify. The native ledger has a separate immutable cancellation tombstone
that fences an exact attempt before or after claim. That tombstone never becomes
a stopped-process receipt or refunds a hold. A future closed no-issue receipt
and successor-head transition are still required to retry a provably unissued
cancelled attempt; this checkpoint cannot infer them from absent rows.

A bounded native budget observation covers every old call under the same child:
complete settled usage is charged its observed consumption; incomplete or
unknown usage retains at least max(original reservation, known consumption);
only proven cancellation before issue has zero liability. A late overrun is
never clamped to allocation. Unrepresentable, unsupported or overflowing
liability cannot produce a spendable command. A positive remainder may fund
new work despite an old unknown reply. The native claim and model-start writer
must independently recompute the child, resource and physical-account limits;
the central observation is a comparison value, not spending authority.

## Closed protocol

New routes under `/hagency/api/v1/management/shared-execution/attempts`:

- `POST /start`: exact `startIntentId`, `assignmentId`, predecessor command ID,
  command digest, attempt number, process digest and stopped-receipt digest,
  plus `budgetObservation`. Fresh same original executor device and generation
  only. Before/after actual Matrix observations, recheck original grant,
  resource, assignment, setup, project administrator, manager and reservation.
- `POST /poll`: existing bounded cursor shape, returning only new attempt
  delivery/cleanup. It never creates a start intent or command.
- `POST /{commandId}/recover`: exact digest; historical observation/upload
  ticket only, never fresh execution authority.
- `POST /{commandId}/result`: exact version-2 process receipt and finite original
  delivery ticket. Current activity starts only from an actually qualified
  process under current authority. Stop and unknown observations remain valid
  cleanup evidence without reviving execution.

`SharedAttemptCommand` has version 2, capability
`hagency.managed_agents.execution.v2`, operation `agent.execution.provision`,
new command ID, original assignment/generation/child IDs, monotonically
increasing `processAttempt`, exact `originalRuntimeCommand` and its digest,
exact predecessor, budget observation, start intent ID, and fresh issue/expiry
times. The predecessor number must be `processAttempt - 1`. A separately typed
version-2 receipt adds attempt number and original-runtime digest to the exact
command/process observation. Neither is decoded as a version-1 receipt.

Budget fields are `observedLedgerRevision`, `allocatedTokens`,
`knownConsumedTokens`, `retainedTokens`, `remainingTokens`, `usageComplete` and
`liabilityDigest`. Numbers are JSON-safe nonnegative integers; revision is
positive, retained is at least known consumption, and remaining equals the
saturating difference between allocation and retained. The local global
revision may advance because of unrelated writes when the exact predecessor,
head and all budget fields/digest remain unchanged. No fabricated mutation
revision is allocated to a read observation.

The new spawn window is at most 30 seconds. The original runtime's expired
30-second spawn deadline does not cap it. Current original reserve/allocation,
grant/delegation and accounting-period bounds still apply; their later renewal
is separate work. Activity renewal retains the same live process and never
authorizes another spawn.

## Storage and compatibility

Retain all original payloads, receipt digests, agent ownership and device foreign
keys. Add append-only start-intent/attempt links and unique assignment/predecessor
and assignment/attempt-number constraints. Existing runtime rows become a
versioned process history with one current unfenced head. Keep a partial unique
constraint for the original version-1 runtime. Existing work, activity, usage and
Matrix reply journals remain keyed to their original runtime command. Their
lookup supports both versions; version-1 public wire bytes stay unchanged.

Deduplicate Matrix events across the entire assignment, rather than only one
runtime. A retried original AS event cannot become a new work item after the
successor starts. History pages include all attempts, with the same manager and
current assigned-administrator visibility. Stop/revoke fences every attempt;
unknown holds and old reply states remain visible. A cleanup observation never
claims capacity release.

This first path does not queue messages received while no process is qualified.
An assignment/event-deduplicated pre-runtime inbox remains required parity work;
it can later supply the same durable start intent. It must not recreate or rebind
an existing work item. Text-only execution remains an initial validation slice;
original tools and resource/agent management remain in scope.

## Acceptance

Run real PostgreSQL/HTTP and native SQLite/process tests for exact same-intent
replay, replacement-intent collision, competing issuers, cancelled authority,
old-generation recovery versus fresh execution, changed provider/resource/setup
or administrator, delayed stop evidence and unrelated ledger advancement.
Verify old spawn expiry alone permits a fresh window while expired business
authority refuses it. Missing/unknown predecessor stop must refuse.

Prove partial 40-token liability in a 100-token child leaves at most 60 tokens;
all-headroom unknown work leaves zero; later consumed tokens are conserved across
attempts. An unknown old reply cannot rerun the model or suppress valid positive
headroom. Qualify a new exact process, issue one genuinely new work item through
the existing ack/start/result/reply route, and recover the old reply separately.
Migration rollback must preserve all original command/receipt bytes, holds and
personal-agent behavior. Default policy flags and live fixtures remain unchanged.

## Current validation boundary

The central checkpoint exercises real PostgreSQL transactions and HTTP dispatch,
plus the existing loopback Matrix membership and message-send routes. It tests
concurrent same-intent start, immutable replay, successor 2 to 3, a real wait
beyond the original spawn deadline, revoked authority during Matrix reads,
new-generation cleanup, stale result tickets, atomic intent/receipt rollback,
all-attempt event deduplication and late earlier-process overrun accounting.
These fixtures do not prove the paired native runtime/controller or a real
provider model call; those remain separate activation requirements.

The removal read model selects the current attempt head. An unobserved new head
returns no process observation, rather than displaying its predecessor's stopped
receipt. All earlier work and usage remain in the history and accounting views.
