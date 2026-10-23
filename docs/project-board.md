# Shared native Project board

The Server is authoritative for human Project work items. Client's Project
**Board** tab starts with five columns and supports 2–12 custom columns,
WIP limits, one-level subtasks and versioned checklists. This is separate from the historical local operator/runner task projection.
It does not execute an Agent, import local tasks or read private Room messages.
Cloud product code is unchanged; self-hosted servers work independently.

## Availability and identity

`project-board-v2` appears in native discovery only after the domain (including
board storage) is initialized. The optional host TOML `project_board_enabled =
false` removes that capability and closes board endpoints while retaining data.
It defaults to true, so existing hosting configurations need no new input. This
is a deliberate implementation choice instead of the report's suggested default
closed rollout; deployments can explicitly set false before their pilot.

All operations use the existing finite **user session bearer**, matching public
Host and native Origin/Cookie restrictions. Fresh trusted Space membership is
obtained on every operation, including counts, search, comments and changes.
Project activation and current user/session/device facts are checked inside the
transaction. Membership observations expire after 30 seconds and are checked
again after lock waits and before writes commit. The authorization point is the
trusted Matrix observation; Matrix and PostgreSQL do not share a transaction.
Leaving a Room and leaving a Project Space remain independent operations.

Current joined members can create, edit, move, comment, archive and restore. Only
joined humans may be assigned; known Hagency puppets are refused. Cloud teams,
Agent ownership/creation permissions and knowing a task ID confer no access.

## Storage and concurrency

Additive schema `hagency_project_v1` uses the same Hagency database, an independent
four-connection pool and its own transaction-protected version-1 and version-2 migrations. It
never changes the existing Agent domain version or Palpo/Pasion migrations.
Opening an already migrated board store verifies the migration and avoids DDL.
Pool checkout is bounded by eight seconds, SQL lock waits by five seconds and
each statement by eight seconds. Cancelled transactions are discarded by the
Diesel pool's broken-transaction recycling check.

There is one logical board per Project; its physical board row is created on the
first successful board mutation. Every mutation locks that row, checks the task
version and commits task changes, activity, sequence and original idempotency
receipt together. Different boards can proceed independently. Receipts are scoped
by Project, actor and intent key and remain retained. Same canonical input
replays its original result, including its original task version; another input
using that key returns `board_idempotency_conflict`. Re-read after replay for the
current board. Comments advance the board sequence without changing task fields.

Server-issued `tsk_` IDs and Project-local numbers remain stable. Tasks have
explicit Project foreign keys. Ranks use gapped positive integers with a target
lane rebalance when gaps run out. A `between` move names the immediate previous
(`after`) and next (`before`) task and requires the current board sequence.
Append/prepend operate on the true server lane boundary. The moved task's version
still must match. Filtered or partially loaded Client lanes use append, while a
fully loaded unfiltered lane supports precise insertion.

## HTTP contract

See the reviewed `crates/agent-service/openapi/hagency-v1.openapi.json` for the
13 board operations and strict input schemas. All use the existing prefix
`/hagency/api/v1/projects/{projectId}`:

| Method and suffix | Result |
| --- | --- |
| GET `board` | Five lanes, exact filtered counts, first page per lane, sequence and joined member IDs |
| GET `tasks` | One filtered page with a revision-bound cursor |
| POST `tasks` | Create a task with an original intent key |
| GET / PATCH `tasks/{taskId}` | Full detail / versioned field update |
| POST `tasks/{taskId}/move` | Atomic state/order change |
| POST `tasks/{taskId}/archive` / `restore` | Retain history and hide / restore at a valid rank |
| GET / POST `tasks/{taskId}/comments` | Page comments / append a comment |
| GET `tasks/{taskId}/activity` | Page that task's activity |
| GET `board/changes` | Page Project change metadata after a committed sequence |

Task filters: `state`, exact `assignee`, exact `label`, literal case-insensitive
substring `query`, `overdue`, `archived`, `limit`. Defaults are 50, maximum 100;
Client snapshots use 50 per lane and omit descriptions. `overdue` compares the
calendar date to today's **UTC** date and excludes Done tasks. Due dates remain
calendar strings `YYYY-MM-DD`; they are never converted to timestamps. A task
page cursor binds Project, filters and sequence; a concurrent mutation yields
`board_snapshot_changed` instead of silently skipping cards. Both history
endpoints use `after` (default 0) and `limit`; returned `next` is the committed
cursor. A board cursor beyond the current sequence yields `board_cursor_reset`.
The Client reloads the authoritative snapshot when the sequence changes.

Task title: 1–240 Unicode characters; description: 32 KiB; at most 12 labels of
1–32 characters; comment: 8 KiB; JSON mutation body: 64 KiB. Priority is
`low/normal/high/urgent`; one optional joined human assignee. There can be at most
10,000 active tasks per board. These bounds are product limits, not measured
throughput claims. There is no permanent task deletion endpoint.

## Native behavior

The desktop uses closed typed commands through NativeOwner and its existing
session lifecycle, and checks the Server capability before calling board APIs.
It never creates a new local management listener or falls back to local tasks.
Unsupported Servers show an explicit message. Mutations retain their exact
intent/payload after an unconfirmed result and offer an explicit original retry.
No write is automatically retried. Conflicts preserve the draft; loading the
latest task requires an explicit discard choice. Account/Project generations
reject late replies. Polls do not overlap, run about every five seconds while
visible/foreground, stop while hidden and back off to 60 seconds after failures.
The board cache and drafts are in memory; offline editing and crash recovery of
an unconfirmed client draft are not promised.

## Verification and release

Run the repository's full Agent suite with a fresh disposable database and
`--include-ignored --test-threads=1` (the existing script uses this isolation and
ordering). Board tests cover live HTTP authentication/membership, shared durable
storage, version races, idempotency, pagination, order and reopen.

For cross-repository testing, launch the example `project_board_fixture` with
`HAGENCY_AGENT_TEST_DATABASE_URL` pointing to a disposable loopback database
whose name starts with `kanban_`. `HAGENCY_BOARD_FIXTURE_INFO` names the output
JSON file. It runs the **real Server API and PostgreSQL**, with explicit fixture
Pasion introspection and Matrix membership on loopback port 55474. In Client run:

```sh
HAGENCY_BOARD_FIXTURE_INFO=/tmp/hagency-kanban-fixture-info.json \
  cargo +1.99.0 test --locked -p hagency --test project_board_http -- --ignored
```

The test uses two real NativeOwner instances and validates create/replay, shared
read/move, stale edit rejection, local reopen and member/outsider refusal. It
launches no model and is not real Matrix-account or two-desktop qualification.
Never expose the fixture identity endpoints or include its router in production.

Release Server first, then Client with capability detection. Verify the actual
running Server image; changing a global image default or an ordinary restart
need not update an existing deployment's saved image reference. Backup/recovery
must include the complete Hagency database (including the new schema), Palpo,
Pasion and existing persistent volume under the current operator procedure.
A successful backup job alone does not establish restore qualification. No
Cloud self-service recovery implementation is required by this feature.


## Custom columns, WIP, subtasks and checklists (v2)

Upgrade Server and Client together. Discovery advertises only `project-board-v2`:
a fixed-five-column v1 Client must explicitly refuse the incompatible board.
The additive `schema_v2.sql` seeds every existing board's five columns, replaces
the fixed-state constraint with a project/column foreign key, and retains task
IDs, versions and immutable original receipts. No Cloud migration is needed.

`PATCH /board/columns` accepts `ConfigureColumns` (original idempotency key,
expected board sequence, ordered column definitions). Each definition has a
stable `id`, trimmed unique `title` (1–48 characters), nullable `wipLimit`
(1–10000), and `isDone`. Keep 2–12 columns and at least one completed column.
Deleting a column containing any active or archived task returns
`board_column_not_empty`; move those tasks first. Configuration changes generate
a `columns_updated` board event with nullable `taskId` and a normal receipt.
The mutation response has `task: null`. A stale board sequence returns 409.

Snapshots return ordered lanes with title, WIP limit, completion flag and
`taskCount` for **all unarchived tasks**, independently of filters or pagination.
Create, cross-column move and restore enforce WIP while holding the same board
row lock; a full column returns `board_wip_limit_reached` (409), without a partial
mutation. Same-column reorder and field edits remain available. Lowering a WIP
limit below existing occupancy is allowed and visibly warns; it deletes nothing.
Overdue filters respect every configured completed column.

Task Fields add nullable `parentId` and a checklist of up to 100 stable items
(`id`, trimmed 1–160-character `title`, `checked`). They use the normal versioned
update and retained original intent. Empty added Fields are omitted when
serializing canonical mutations, preserving pre-upgrade receipt digests.
New parent links require an active root task in the same Project, no self-link,
no nesting/cycles, and at most 100 total children. Existing links survive parent
archive; children keep independent state, assignment and archive lifecycle.
Details read the task, bounded `subtasks` and optional full `parent` in one
repeatable-read database snapshot. Card summaries omit
the checklist body and expose checklist/subtask progress counts instead.

Native Client provides a dedicated columns panel with reorder controls, draft
validation, original retry and explicit adoption of a newer board revision after
review. Task inspector offers editable/ordered checklist rows, parent navigation
and unlinking, child navigation and creation. Checklist edits become shared only
when **Save task** succeeds; conflicts preserve the complete draft. Input bodies
are bounded at 128 KiB; native board responses at 4 MiB.
