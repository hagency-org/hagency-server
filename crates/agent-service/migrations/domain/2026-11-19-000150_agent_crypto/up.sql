-- Public identity pins only. Authorization epochs do not replace crypto keys.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.agent_crypto_identities (
 agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id),
 execution_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id),
 first_agent_generation bigint NOT NULL CHECK(first_agent_generation>0),
 first_device_generation bigint NOT NULL CHECK(first_device_generation>0),
 matrix_device_id text NOT NULL UNIQUE,
 device_keys_digest text,
 master_key_digest text,
 self_signing_key_digest text,
 user_signing_key_digest text,
 PRIMARY KEY(agent_id,execution_device_id)
);
