-- GitFab orchestrator stage messages: the only events ingest_orchestrated admits without a mention.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.orchestrator_stage_events(
    binding_id text NOT NULL,
    event_id text NOT NULL CHECK (event_id LIKE '$%'),
    run_id text NOT NULL,
    stage text NOT NULL CHECK (stage IN ('spec','plan','execute','test','review','release')),
    round integer NOT NULL CHECK (round >= 0),
    attempt integer NOT NULL CHECK (attempt >= 1),
    created_at_ms bigint NOT NULL,
    PRIMARY KEY (binding_id, event_id)
);
