-- Additive migration: the queue never contains provider credentials or execution leases.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.configuration_presence (
 device_id text PRIMARY KEY REFERENCES hagency_agent_v1.devices(id),
 device_generation bigint NOT NULL, last_seen_at_ms bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.configuration_requests (
 id text PRIMARY KEY, owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id), agent_generation bigint NOT NULL,
 execution_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id), device_generation bigint NOT NULL,
 requester_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id), requester_generation bigint NOT NULL,
 idempotency_key text NOT NULL, digest text NOT NULL, operation text NOT NULL,
 input text NOT NULL CHECK(octet_length(input)<=65536), mutation boolean NOT NULL,
 state text NOT NULL CHECK(state IN ('pending','claimed','succeeded','failed','unknown','expired')),
 created_at_ms bigint NOT NULL, expires_at_ms bigint NOT NULL,
 claimed_at_ms bigint, completed_at_ms bigint,
 result text CHECK(octet_length(result)<=65536), error_code text,
 UNIQUE(owner_user_id,requester_device_id,agent_id,idempotency_key)
);
CREATE INDEX IF NOT EXISTS configuration_pending_idx ON hagency_agent_v1.configuration_requests(execution_device_id,state,created_at_ms,id);
CREATE INDEX IF NOT EXISTS configuration_owner_idx ON hagency_agent_v1.configuration_requests(owner_user_id,agent_id);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.configuration_observations (
 agent_id text PRIMARY KEY REFERENCES hagency_agent_v1.agents(id),
 agent_generation bigint NOT NULL, execution_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id),
 device_generation bigint NOT NULL, observed_at_ms bigint NOT NULL,
 environment text NOT NULL CHECK(octet_length(environment)<=65536)
);
