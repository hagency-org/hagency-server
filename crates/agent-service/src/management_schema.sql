CREATE TABLE hagency_agent_v1.management_deployment (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
 version integer NOT NULL CHECK(version=1)
);
INSERT INTO hagency_agent_v1.management_deployment(singleton,version) VALUES(true,1);
CREATE TABLE hagency_agent_v1.management_contributions (
 id text PRIMARY KEY,
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 executor_device_id text NOT NULL, executor_generation bigint NOT NULL CHECK(executor_generation>0),
 FOREIGN KEY(executor_device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 resource_id text NOT NULL, resource_revision text NOT NULL CHECK(length(resource_revision)=64),
 label text NOT NULL, runtime text NOT NULL CHECK(runtime IN ('codex','claude','octos')),
 capabilities text NOT NULL, coordinator_mxid text NOT NULL,
 state text NOT NULL CHECK(state IN ('pending_admission','admitted','rejected','revoked')),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 request_id text NOT NULL UNIQUE, created_at_ms bigint NOT NULL, updated_at_ms bigint NOT NULL,
 UNIQUE(owner_user_id,executor_device_id,resource_id)
);
CREATE TABLE hagency_agent_v1.management_requests (
 id text PRIMARY KEY, kind text NOT NULL CHECK(kind='resource_admission'),
 contribution_id text NOT NULL REFERENCES hagency_agent_v1.management_contributions(id),
 requester_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 approver_mxid text NOT NULL,
 state text NOT NULL CHECK(state IN ('pending','approved','rejected','revoked')),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 created_at_ms bigint NOT NULL, expires_at_ms bigint NOT NULL CHECK(expires_at_ms>created_at_ms),
 decided_at_ms bigint
);
ALTER TABLE hagency_agent_v1.management_contributions ADD CONSTRAINT management_admission_request
 FOREIGN KEY(request_id) REFERENCES hagency_agent_v1.management_requests(id) DEFERRABLE INITIALLY DEFERRED;
CREATE INDEX management_inbox ON hagency_agent_v1.management_requests(approver_mxid,state,id);
CREATE INDEX management_submitted ON hagency_agent_v1.management_requests(requester_user_id,id);
CREATE TABLE hagency_agent_v1.management_commands (
 actor_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 device_id text NOT NULL, device_generation bigint NOT NULL,
 command_id text NOT NULL, operation text NOT NULL, digest text NOT NULL, result text NOT NULL,
 FOREIGN KEY(device_id,actor_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 PRIMARY KEY(actor_user_id,device_id,device_generation,command_id)
);
-- Transactional intent only. No credentials, resource details, or configuration
-- enter Matrix notices. An old notification resolves the current request.
CREATE TABLE hagency_agent_v1.management_outbox (
 id text PRIMARY KEY, request_id text NOT NULL REFERENCES hagency_agent_v1.management_requests(id),
 request_revision bigint NOT NULL, recipient_mxid text NOT NULL,
 state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','sending','sent','cancelled')),
 transaction_id text NOT NULL UNIQUE, created_at_ms bigint NOT NULL,
 UNIQUE(request_id,request_revision,recipient_mxid)
);
