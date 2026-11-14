-- One live status message per running dispatch ("Working…", edited as the
-- device reports steps, then "Done in 41 s · 6 steps" or the failure text).
-- Cosmetic and server-owned: it never authorizes or replays model work.
-- `revision` is the wanted content version, `sent_revision` the last posted one;
-- edits use one Matrix transaction per revision, the first post its own fixed one.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.progress_status (
 id text PRIMARY KEY,
 owner_event_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.owner_events(id) ON DELETE CASCADE,
 execution_id text NOT NULL, binding_id text NOT NULL, binding_generation bigint NOT NULL,
 started_at_ms bigint NOT NULL,
 recent jsonb NOT NULL DEFAULT '[]', step_count bigint NOT NULL DEFAULT 0,
 final_body text,
 revision bigint NOT NULL DEFAULT 1, sent_revision bigint NOT NULL DEFAULT 0,
 matrix_txn_id text NOT NULL UNIQUE, status_event_id text,
 state text NOT NULL CHECK(state IN ('live','final','done','cancelled')),
 worker_token text, worker_until_ms bigint NOT NULL DEFAULT 0, last_sent_ms bigint NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS progress_status_pending_idx ON hagency_agent_v1.progress_status(state,id);
