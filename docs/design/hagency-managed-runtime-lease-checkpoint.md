# Managed runtime qualification lease checkpoint

2026-10-16. Central schema 11; native schema 79 is reserved after the reviewed
schema 78 adoption journal. This is a disabled development protocol. It does not
make shared agents runnable, enable model turns, or settle their token holds.

## Central contract

`management_agent_runtime_types.rs` defines closed version-1 camelCase inputs.
The canonical public fixture is
`crates/agent-service/tests/fixtures/management-agent-runtime-v1.json`; its
unreferenced native mirror is in
`native/hagency-store/src/domain/owner_resources/capacity/managed_agents/custody/runtime/`.
Schema 9 reserve/fence and schema 10 Matrix challenge bytes are unchanged.

| Route below management | Purpose |
| --- | --- |
| POST `agent-runtime/poll` | Fresh exact provision or original stop delivery; `{after?}` is the assignment cursor. |
| POST `agent-runtime/{commandId}/recover` | `{commandDigest, observationId?}`; always `executionAllowed=false`. An exact observation ID can recover older immutable evidence. |
| POST `agent-runtime/{commandId}/result` | `{commandDigest, token, receipt}`; finite upload ticket and exact canonical binding. |
| GET `agent-requests/{requestId}/runtime` | Latest process observation, with `workAvailable=false` and `capacityReleased=false`. |

Fresh provision requires the separate default-off runtime flag and all existing
Matrix, agent, setup and allocation gates, the exact current enrolled owner/device
generation, actual recent Matrix observation, live assigned-admin/project/setup/
allocation/grant authority, and a canonical Codex resource. A policy name is not
accepted as evidence that the selected resource uses Codex. Historical recovery
is available to a fresh generation of the same installation for cleanup only.
No capability is advertised in a normal management session.

`agent.runtime.provision` freezes the original complete reserve command and its
native receipt digest, child, assignment generation, `codex_no_tools_v1`, and
issue/expiry times. Exactly one provision exists per assignment. Its deadline is
at most 30 seconds and no later than the original reservation. Delivery/replay
never moves it. A lost reply or expired proof cannot authorize a replacement.

`agent.runtime.stop` freezes that entire original provision and its digest.
Assignment fencing transactionally enqueues one deterministic stop command.
Expiry also fences and queues cleanup. Queue failure rolls back the revocation,
so a successful revocation cannot omit its durable cleanup obligation. No agent
identity, manager ownership or personal execution-device FK is rewritten.

Receipts append process observations at distinct canonical ledger revisions.
`qualified/live`, `failed/not_started`, `unknown/unknown` apply to provision;
`stopped/stopped`, `not_started/not_started`, `unknown/unknown` apply to cleanup.
They cannot carry usage completeness or token-refund fields. Late qualification
cannot restore a fenced assignment; late older observations remain exact history
without overwriting a newer stop. Revision collisions with project, parent or
agent receipts are rejected. Contradictory not-started/qualified evidence and a
new live observation after a terminal process result are rejected.

## Native implementation boundary

The next native sub-checkpoint will atomically bind the complete command to the
already reserved child and issue the existing provision effect before any spawn.
A non-deserializable fresh delivery proof must verify the original saved native
reserve receipt, profile, installation/generation, current fences, resource
runtime, account identity and fixed deadline. History yields no fresh custody.
Stop before provision creates a permanent semantic tombstone on this same child,
so a delayed provision cannot start a process under another command ID.

The supervised adapter must qualify the actual retained process against its
canonical provider account and effective no-tools policy. A previous metadata
probe, process ID, JSON success, or Matrix membership is insufficient. The
process observer's evidence sink may append stop/unknown evidence after profile
retirement, but cannot issue work, requalify, respawn or release capacity. Runtime
custody must be synchronously retired, then fully drained before the canonical
store is closed. Unknown cleanup is durable and blocks adoption.

Tests of the supervised adapter use isolated fixture children. An actual Codex
provider and actual homeserver end-to-end test remains separate acceptance; this
central checkpoint alone does not claim either.

## Persistent agent boundary after this checkpoint

Thirty seconds is intentionally sufficient only for qualification and stop.
A real persistent agent needs an independently reviewed renewable activity/idle
lease tied to the *same retained process*, separate from its immutable one-shot
spawn command. Renewing activity must never recreate issued spawn custody or
allow respawn after uncertainty. First-turn integration must also bind prompt
issuance, provider usage provenance, cancellation, late usage, tool/reply checks,
and stop recovery to that lifetime. Until then the identity remains suspended,
work unavailable, and all holds conservative.
