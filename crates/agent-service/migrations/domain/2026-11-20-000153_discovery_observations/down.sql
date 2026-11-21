ALTER TABLE hagency_agent_v1.management_shared_executor_protocols
    DROP COLUMN IF EXISTS budget_poll_at_ms,
    DROP COLUMN IF EXISTS ordinary_poll_at_ms;
