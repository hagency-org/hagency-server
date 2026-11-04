<!-- claude-coordination: start -->
## Coordination notice (2026-11-04, Claude Code)

Claude Code is implementing room-history context budgeting, paged room
summaries and event-id de-duplication for shared Agents, on branch
`fix/room-context-budget` in separate worktrees of hagency-server,
hagency-client and hagency-desktop. Until this notice is removed, please do
not edit these areas; coordinate through this file instead:

- server: `crates/agent-service/src/management_shared_context*.rs`,
  `management_shared_work.rs` (context freeze and paging only)
- client: `native/hagency-store/src/domain/owner_resources/capacity/managed_agents/custody/shared_execution/context*.rs`,
  `native/hagency/src/console/native/shared_execution/context_io.rs`
- desktop: the Agent chat summary trigger (`src/agent_chat/*`, Splash owner page untouched)

Everything else is free. Last updated: 2026-11-04 11:30 local.
<!-- claude-coordination: end -->

