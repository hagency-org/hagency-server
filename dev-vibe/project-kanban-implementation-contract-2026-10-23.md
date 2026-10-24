# Shared native project kanban — implementation contract, 2026-10-23

Scope: Server and native Client only. One board per registered active Project;
current joined Space members collaborate. No Cloud product changes, local-task
import, Agent auto-execution, Web board, attachments, offline editing or E2EE.
Historical REQ-PROJECT-BOARD/ADR-009 remain read-only operations projections;
this contract defines a separate shared human work-item model.

Server owns tasks, comments, activity, order and versions in PostgreSQL. Fixed
states: todo, in_progress, blocked, in_review, done. Fields: title, plain-text
body, low/normal/high/urgent priority, labels, optional joined human Matrix
assignee and calendar due date. Archive and restore retain history.

Native identity/session/device revocation and fresh Space membership are checked
for every operation. Fresh membership is obtained before the transaction and has
a bounded age rechecked after queue/lock waits. Active Project and identity are
rechecked while holding rows in the transaction. Cloud membership and Agent
creation rights grant no board rights. Private Room content is never imported.

Use existing /hagency/api/v1/projects/{project} routes: board, tasks,
tasks/{task}, tasks/{task}/move, archive, restore, comments, and board/changes.
Discovery capability project-board-v1; unsupported server is an explicit error.
Bound all pages (default 50, maximum 100), snapshots (50 per lane), payloads and
queries. Use stable task IDs and project-local numbers. Client checks captured
account/project/request identity before applying every async response.

Board-row serialization provides atomic ordering, strictly committed change
sequences, version conflict detection and idempotency receipts. The same intent
key plus same payload replays the original result; different payload conflicts.
Client retains the original key/payload after unknown network outcomes. Never
mark a save successful until Server confirms. Conflicts refresh server state
and preserve the user's draft. A move uses server-checked neighbours/append,
never client-generated rank. Filtered moves append; exact reorder only in an
unfiltered complete lane. Consistent snapshots and revision-bound pagination
avoid silently skipping cards during concurrent moves.

Native design: project Board tab; five neutral surface columns with restrained
status accents and counts; cards have title, priority, labels, assignee and date;
right-side details/editor with comments/activity; horizontal board scrolling and
virtualized independent columns. Visible loading, empty, saving, offline,
permission, unsupported and conflict states. Keyboard/menu move alternative to
drag. English/Chinese and existing light/dark theme. Foreground five-second
non-overlapping change polling, stopped while hidden; account switch clears all
content. No arbitrary path/URL proxy and no revived Web listener.

Validation: actual disposable PostgreSQL migrations/reopen, two authenticated
identities, cross-project/left-member refusal, concurrent edits/moves, receipt
replay, comments, archive/restore, filters/pagination and sequence recovery;
HTTP auth and native command serialization; UI script/layout/interaction tests,
i18n checks, affected strict Clippy and native visual inspection where available.
Product acceptance and any unqualified real-device conditions must be recorded
separately; mock UI does not prove shared operation. No production deployment.
