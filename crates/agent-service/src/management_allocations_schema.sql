-- Owner-native grant receipt is evidence of a reservation; central rows do not
-- reserve provider capacity. Agent execution still has its original owner FK.
CREATE TABLE hagency_agent_v1.management_resource_grants (
 id text PRIMARY KEY,
 contribution_id text NOT NULL REFERENCES hagency_agent_v1.management_contributions(id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 executor_device_id text NOT NULL,
 executor_generation bigint NOT NULL CHECK(executor_generation>0),
 FOREIGN KEY(executor_device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 receipt text NOT NULL, receipt_digest text NOT NULL,
 state text NOT NULL CHECK(state IN ('active','revoked')),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 period_key text NOT NULL, expires_at_ms bigint NOT NULL,
 observed_at_ms bigint NOT NULL
);
CREATE UNIQUE INDEX management_current_resource_grant ON hagency_agent_v1.management_resource_grants(contribution_id,period_key) WHERE state='active';
CREATE TABLE hagency_agent_v1.management_allocations (
 id text PRIMARY KEY,
 project_id text NOT NULL REFERENCES hagency_agent_v1.management_projects(id),
 project_revision bigint NOT NULL,
 grant_id text NOT NULL REFERENCES hagency_agent_v1.management_resource_grants(id),
 request_id text NOT NULL UNIQUE,
 state text NOT NULL CHECK(state IN ('pending_coordinator','approved_waiting_executor','reserved','refused','outcome_unknown','rejected','expired','releasing','release_blocked','released')),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 expires_at_ms bigint NOT NULL,
 reserve_command_id text,
 release_command_id text,
 result text,
 balance text,
 UNIQUE(project_id,grant_id)
);
ALTER TABLE hagency_agent_v1.management_requests ADD COLUMN allocation_id text REFERENCES hagency_agent_v1.management_allocations(id);
ALTER TABLE hagency_agent_v1.management_requests DROP CONSTRAINT management_requests_kind_check;
ALTER TABLE hagency_agent_v1.management_requests ADD CONSTRAINT management_requests_kind_check CHECK(
 (kind='resource_admission' AND project_id IS NULL AND allocation_id IS NULL) OR
 (kind='project_capacity' AND project_id IS NOT NULL AND allocation_id IS NULL) OR
 (kind='project_allocation' AND project_id IS NOT NULL AND allocation_id IS NOT NULL));
ALTER TABLE hagency_agent_v1.management_allocations ADD CONSTRAINT management_allocation_request FOREIGN KEY(request_id) REFERENCES hagency_agent_v1.management_requests(id) DEFERRABLE INITIALLY DEFERRED;
CREATE TABLE hagency_agent_v1.management_executor_commands (
 id text PRIMARY KEY,
 allocation_id text NOT NULL REFERENCES hagency_agent_v1.management_allocations(id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 executor_device_id text NOT NULL,
 FOREIGN KEY(executor_device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 executor_generation bigint NOT NULL,
 operation text NOT NULL CHECK(operation IN ('project.reserve','project.release')),
 command text NOT NULL, digest text NOT NULL,
 state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','issued','recorded')),
 lease_hash text, lease_until_ms bigint NOT NULL DEFAULT 0,
 receipt text, receipt_digest text,
 CHECK((state='recorded')=(receipt IS NOT NULL AND receipt_digest IS NOT NULL))
);
CREATE INDEX management_executor_pending ON hagency_agent_v1.management_executor_commands(owner_user_id,executor_device_id,id) WHERE state<>'recorded';
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=4;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=4);
