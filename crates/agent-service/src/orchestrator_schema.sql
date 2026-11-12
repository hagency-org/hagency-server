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
-- One GitFab workflow run per task thread. `state` is orchestrator_flow::State in stored form;
-- `revision` guards against two workers advancing the same run.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.orchestrator_runs(
    id text PRIMARY KEY CHECK (id LIKE 'orun_%'),
    project_id text NOT NULL,
    task_id text NOT NULL,
    room_id text NOT NULL CHECK (room_id LIKE '!%'),
    thread_root text NOT NULL CHECK (thread_root LIKE '$%'),
    requester_mxid text NOT NULL CHECK (requester_mxid LIKE '@%'),
    bindings jsonb NOT NULL,
    context jsonb NOT NULL,
    state jsonb NOT NULL,
    revision bigint NOT NULL DEFAULT 1 CHECK (revision >= 1),
    dispatch_id text,
    handoff text,
    pending_events jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(pending_events) = 'array'),
    created_at_ms bigint NOT NULL,
    updated_at_ms bigint NOT NULL,
    UNIQUE (room_id, thread_root)
);
