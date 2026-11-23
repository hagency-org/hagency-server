CREATE TABLE IF NOT EXISTS hagency_agent_v1.inbound_transactions (id text PRIMARY KEY,digest text NOT NULL,body jsonb NOT NULL,received_at_ms bigint NOT NULL);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.routing_jobs (transaction_id text PRIMARY KEY REFERENCES hagency_agent_v1.inbound_transactions(id),state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','routed')),received_at_ms bigint NOT NULL);
CREATE INDEX IF NOT EXISTS routing_jobs_pending_time ON hagency_agent_v1.routing_jobs(received_at_ms,transaction_id) WHERE state='pending';
CREATE INDEX IF NOT EXISTS routing_jobs_pending_id ON hagency_agent_v1.routing_jobs(transaction_id) WHERE state='pending';
CREATE TABLE IF NOT EXISTS hagency_agent_v1.routing_rejections (transaction_id text NOT NULL REFERENCES hagency_agent_v1.inbound_transactions(id) ON DELETE CASCADE,event_id text NOT NULL,binding_id text NOT NULL,reason text NOT NULL,PRIMARY KEY(transaction_id,event_id,binding_id));
CREATE TABLE IF NOT EXISTS hagency_agent_v1.readiness_room (singleton boolean PRIMARY KEY CHECK(singleton),room_id text NOT NULL);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.readiness_receipts (event_id text PRIMARY KEY,received_at_ms bigint NOT NULL);
