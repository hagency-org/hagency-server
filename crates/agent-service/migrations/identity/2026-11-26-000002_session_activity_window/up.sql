-- How long an activity lease issued to a session's device lasts: the
-- authorization window the session was granted, once its client says it
-- follows that window. Every other session keeps thirty seconds, which is
-- what an older client checks each lease against.
ALTER TABLE hagency_agent_v1.sessions
 ADD COLUMN IF NOT EXISTS activity_window_ms bigint NOT NULL DEFAULT 30000
 CONSTRAINT sessions_activity_window CHECK(activity_window_ms BETWEEN 30000 AND 120000);
