# ADR 0001: Per-owner room registration

Status: accepted for the demo fork (2026-11-12, Chris); proposed upstream.

## Context

A room was registered with an Agent service under exactly one project
(`rooms.room_id` was the primary key; `register_room_transaction` returned
`room_already_registered` for any other project). A project is one owner's Agent
Space, and a bind requires the owner to be a joined member of that Space with the
room linked as its child. So the first owner on a server to register a room locked
out every other owner on that server: their bind was refused as
`domain_object_not_authorized`, and binding under the first owner's project failed
`room_membership_required` because they are not in that owner's private Space.
Seen live: ben2 (desktop) registered the federated room haws on beta, after which
the concierge could not add webster2's hosted Agent there. Registration also
demanded room power (`im.hagency.agent.policy` / `m.space.child` level), which is
why every guest owner had to be made a moderator first.

## Decision

1. A room is registered once per project: the key is `(room_id, project_id)`.
   The schema migrates the old single-column key in place at startup; every
   foreign key and join already used the pair.
2. Registering a room requires the owner to be a joined member of it (fresh
   observation), plus administration of their own Agent Space and the room
   linked under that Space (both unchanged). Room power is no longer required.
3. Binding is unchanged: owner joined in the room and in their own Space, room
   linked under that Space, service able to invite, project and room policy
   permit it. An owner still cannot bind under someone else's project.
4. Room-admin decisions stay room-wide:
   - room pause (`suspend_room_bindings`, `clear_room_pause`) now acts on every
     owner's bindings in the room (the pause flag was already keyed by room);
   - a room policy set by a room admin is written to every registration of the
     room, and a later registration copies the newest existing one (policy and
     disabled state).

No new table, API or concept. Clients (desktop Add agent, `agent up`, concierge)
are unchanged: each already registers the room under its own first project.

## Rule: Agents go with their owners (Chris, 2026-11-12)

1. An owner can bind an Agent only into a room where the owner is currently
   joined (bind and registration both check a fresh observation).
2. When the owner leaves, is kicked or banned, or the account is deactivated,
   the server retires that owner's bindings in the room and the puppet leaves.
   This is the existing membership controller: a sweep every 2 s moves such a
   binding to `leaving` (then `left`), and delivery re-checks the owner's
   membership for every message, so the Agent stops answering at once.
3. Room admins can still remove any single Agent (Matrix kick/ban of the puppet).
4. No personal Agent stays after its owner leaves. A rejoin does not revive it:
   the owner adds it again. Project-owned bots remain the shared-pool feature.

Concierge-hosted Agents: their server owner is the concierge account, so the
concierge applies the rule for the person each Agent was made for: when that
requester leaves, is kicked or banned from a room, the concierge takes their
hosted Agents out of it (`concierge/rooms.rs` `on_departure`); a later invite of
the concierge counts as a new request.

Tests: `postgres_an_owner_leaving_retires_only_their_agents_and_rejoining_does_not_revive_them`
and the kick / rejoin / re-add steps of `scripts/demo-two-owners-one-room-e2e.py`.

## Security implications

- **Who can bring an Agent into a room:** any joined member, for their own Agents
  only (before: only members with room power). The Agent still needs a Matrix
  invite (room `invite` level) and the server's service account must be joined
  with invite power, so room admins can restrict this with ordinary power levels.
- **Room admins keep control:** kick/ban the puppet, raise the invite level,
  room pause (now covers all owners), room policy `Disabled` or allow-list (now
  covers all owners).
- **Removal on leave or kick:** unchanged and per binding. Delivery re-checks the
  owner's and puppet's membership for every message, so an owner who leaves or is
  kicked stops being able to use their Agent there, and the membership controller
  leaves rooms for bindings whose owner lost membership.
- **Isolation between owners:** an owner sees and manages only rooms in their own
  projects (project listing is filtered by Space membership); one owner's
  per-binding pause or removal does not touch another owner's binding.
- **Project-level admin** (`suspend_project_bindings`) still acts on one
  project only.

## Consequences

- Several owners can each add their Agents to a shared room on the same server,
  matching how Matrix bots work.
- The concierge no longer needs moderator power to add a hosted Agent.
- Upstream PR topic: `room-registration-per-owner` (hagency-server).
- Tests: `domain_room_owners_tests.rs` (two owners bind into one room; room
  pause and policy cover both; another owner's project stays off limits;
  registration needs a joined member) and the e2e
  `scripts/demo-two-owners-one-room-e2e.py`.
