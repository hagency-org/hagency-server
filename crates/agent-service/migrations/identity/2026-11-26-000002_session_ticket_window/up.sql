-- How long anything issued to a session's device for pooled work may last:
-- the authorization window the session was granted, once its client says it
-- follows that window. Every other session keeps thirty seconds, which is
-- what an older client checks each such end against.
ALTER TABLE hagency_agent_v1.sessions
 ADD COLUMN IF NOT EXISTS ticket_window_ms bigint NOT NULL DEFAULT 30000
 CONSTRAINT sessions_ticket_window CHECK(ticket_window_ms BETWEEN 30000 AND 120000);
