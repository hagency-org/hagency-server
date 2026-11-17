-- The requester of a hosted Agent: the one non-owner human whose plain
-- messages address the Agent in a private pair Room. Empty = mention required.
ALTER TABLE hagency_agent_v1.bindings ADD COLUMN IF NOT EXISTS room_human_mxid text NOT NULL DEFAULT '';
-- Invites of an Agent's puppet sent by that Agent's recorded Room human. The
-- owner (a concierge) lists them and binds through the normal bind path.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.puppet_invites(
 agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id) ON DELETE CASCADE,
 room_id text NOT NULL,
 inviter_mxid text NOT NULL,
 received_at_ms bigint NOT NULL,
 PRIMARY KEY(agent_id,room_id)
);
