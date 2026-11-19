ALTER TABLE hagency_agent_v1.bindings ADD COLUMN IF NOT EXISTS owner_service_paused boolean NOT NULL DEFAULT false;
ALTER TABLE hagency_agent_v1.bindings ADD COLUMN IF NOT EXISTS thread_auto_reply boolean NOT NULL DEFAULT false;
