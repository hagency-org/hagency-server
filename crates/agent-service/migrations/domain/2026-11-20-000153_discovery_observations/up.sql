ALTER TABLE hagency_agent_v1.management_shared_executor_protocols
    ADD COLUMN ordinary_poll_at_ms bigint,
    ADD COLUMN budget_poll_at_ms bigint;
