-- Ciphertext intake precedes every model/budget operation. No room keys, tokens
-- or decrypted content are stored here.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.management_shared_e2ee_candidates (
 assignment_id text NOT NULL,
 event_id text NOT NULL,
 payload text NOT NULL,
 received_at_ms bigint NOT NULL,
 state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','confirmed','ignored')),
 PRIMARY KEY (assignment_id,event_id)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.executor_matrix_devices (
 scope_id text NOT NULL,
 puppet_mxid text NOT NULL,
 device_id text NOT NULL,
 executor_device_id text NOT NULL,
 executor_generation bigint NOT NULL,
 kind text NOT NULL CHECK(kind IN ('personal','shared')),
 revoked boolean NOT NULL DEFAULT false,
 PRIMARY KEY (scope_id,device_id)
);

CREATE TABLE IF NOT EXISTS hagency_agent_v1.personal_e2ee_candidates (
 binding_id text NOT NULL,
 event_id text NOT NULL,
 payload text NOT NULL,
 received_at_ms bigint NOT NULL,
 done boolean NOT NULL DEFAULT false,
 PRIMARY KEY(binding_id,event_id)
);

ALTER TABLE hagency_agent_v1.management_shared_e2ee_candidates ADD COLUMN IF NOT EXISTS last_polled_ms bigint NOT NULL DEFAULT 0;
ALTER TABLE hagency_agent_v1.personal_e2ee_candidates ADD COLUMN IF NOT EXISTS last_polled_ms bigint NOT NULL DEFAULT 0;
ALTER TABLE hagency_agent_v1.personal_e2ee_candidates ADD COLUMN IF NOT EXISTS confirmation text;
-- Retire identity before attempting logout: a lost response may already have
-- deleted its published keys. Never reuse that Olm account on a later grant.
ALTER TABLE hagency_agent_v1.executor_matrix_devices ADD COLUMN IF NOT EXISTS retiring boolean NOT NULL DEFAULT false;
CREATE INDEX IF NOT EXISTS shared_e2ee_poll ON hagency_agent_v1.management_shared_e2ee_candidates(assignment_id,last_polled_ms,received_at_ms,event_id);
CREATE INDEX IF NOT EXISTS personal_e2ee_poll ON hagency_agent_v1.personal_e2ee_candidates(binding_id,last_polled_ms,received_at_ms,event_id);
