-- The authorization window a session follows for the activity leases issued
-- to its device: set once its client says it takes a lease as long as that
-- window. NULL is a session that follows none, which is every older client:
-- it checks each lease against thirty seconds and gets thirty seconds.
ALTER TABLE hagency_agent_v1.sessions
 ADD COLUMN IF NOT EXISTS activity_window_ms bigint
 CONSTRAINT sessions_activity_window CHECK(activity_window_ms BETWEEN 30000 AND 120000);
