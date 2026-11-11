# Shared process/activity checkpoint (central schema 12)

Default-disabled, unadvertised and not deployed. The paired native first-turn driver is still being implemented; this checkpoint alone cannot deliver prompts or run a model.

The new `hagency.managed_agents.execution.v1` / `codex_text_v1` command is separate from the original metadata-only runtime v1. A never-issued pending assignment may be claimed by either protocol exactly once; database guards reject attempts to bind both. Original assignment manager ownership, executor device/generation, native child reservation and canonical reserve bytes remain fixed. Personal binding and lease paths retain their original owner/device constraints.

Fresh `POST management/shared-execution/runtime/poll` grants one finite issuance ticket. `runtime/{id}/recover` is observation/upload only. Result receipts bind the original command, native child, native ledger revision and process identity. Only a fresh qualified result may establish activity. Recovered/late/expired/revoked/generation-changed evidence remains retained and cannot create execution permission.

`POST management/shared-execution/activity/renew` requires the current epoch/token and current Matrix/project/admin/allocation/resource/device authority. It renews only the same qualified process before the old lease expires. Every renewal ID is permanently journaled; exact immediate replay returns the same expiry, stale/reused IDs do not extend authority. Native integration must pin each accepted interval to its monotonic clock and never reinterpret a renewal as permission to respawn.

Revocation atomically fences the process. Stop/unknown evidence is durable; it never implies freed capacity. `GET management/agent-requests/{id}/execution` is manager/current assigned-admin scoped and reports `workAvailable:false`, `capacityReleased:false`, and no work history at this checkpoint.

Validation: 14 focused real PostgreSQL/HTTP cases pass, including actual loopback Matrix setup, lost/replayed replies, wrong roles/runtime, expiry, generation rotation, mutation rollback, revoke during Matrix observation, canonical fixture equality, cross-protocol ledger collisions, migration rollback and both directions of direct-SQL legacy rebinding denial. The full isolated PostgreSQL/HTTP suite passes **205/205** in 299.74 seconds. Strict agent-service all-target Clippy passes. Server build passes with the existing macOS large `__eh_frame` linker warning.

Frozen artifact: `.run/shared-execution-schema12-checkpoint/hagency-server`, SHA-256 `dec72e7c554f596e43b1a8c52d3f93ad957983d1495014fb8593f7e411f43f1f`. Canonical fixture: `crates/agent-service/tests/fixtures/management-shared-execution-v1.json`. Evidence/source hashes are in the same checkpoint directory. The staged shared-work module/schema/types are not registered in this artifact.

Next bounded implementation adds AS message intake, native inbox acknowledgement, one durable execution start, late usage/result recovery and one immutable Matrix reply transaction. First acceptance is a text-only actual Codex turn; original coding/tools, workspace, top-up, statistics, restart and multi-provider parity remain required follow-up work, as recorded in the full parity inventory.
