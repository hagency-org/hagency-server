ALTER TABLE hagency_agent_v1.management_shared_executor_protocols
    ADD COLUMN IF NOT EXISTS ordinary_poll_at_ms bigint,
    ADD COLUMN IF NOT EXISTS budget_poll_at_ms bigint;
