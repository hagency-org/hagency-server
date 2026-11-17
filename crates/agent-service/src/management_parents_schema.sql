-- Parent cleanup is a separate protocol. Revocation never implies a refund.
CREATE TABLE hagency_agent_v1.management_parent_settlements (
 grant_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_resource_grants(id),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 state text NOT NULL CHECK(state IN ('closing','retained','closed')),
 close_command_id text NOT NULL UNIQUE,
 latest_command_id text NOT NULL UNIQUE,
 receipt text,
 receipt_digest text,
 CHECK((receipt IS NULL)=(receipt_digest IS NULL))
);
CREATE TABLE hagency_agent_v1.management_parent_commands (
 id text PRIMARY KEY,
 grant_id text NOT NULL REFERENCES hagency_agent_v1.management_parent_settlements(grant_id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 executor_device_id text NOT NULL,
 executor_generation bigint NOT NULL CHECK(executor_generation>0),
 FOREIGN KEY(executor_device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 operation text NOT NULL CHECK(operation IN ('grant.close','grant.settle')),
 command text NOT NULL,
 digest text NOT NULL,
 state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','issued','recorded')),
 lease_hash text,
 lease_until_ms bigint NOT NULL DEFAULT 0,
 receipt text,
 receipt_digest text,
 CHECK((state='recorded')=(receipt IS NOT NULL AND receipt_digest IS NOT NULL))
);
CREATE UNIQUE INDEX management_one_parent_close ON hagency_agent_v1.management_parent_commands(grant_id) WHERE operation='grant.close';
CREATE INDEX management_parent_pending ON hagency_agent_v1.management_parent_commands(owner_user_id,executor_device_id,id) WHERE state<>'recorded';
ALTER TABLE hagency_agent_v1.management_parent_settlements ADD CONSTRAINT management_parent_close_command FOREIGN KEY(close_command_id) REFERENCES hagency_agent_v1.management_parent_commands(id) DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE hagency_agent_v1.management_parent_settlements ADD CONSTRAINT management_parent_latest_command FOREIGN KEY(latest_command_id) REFERENCES hagency_agent_v1.management_parent_commands(id) DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=5;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=5);
