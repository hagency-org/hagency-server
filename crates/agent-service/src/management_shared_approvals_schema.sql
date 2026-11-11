CREATE TABLE hagency_agent_v1.management_shared_approvals (
 id text PRIMARY KEY,
 work_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_work(id),
 runtime_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 requester_mxid text NOT NULL,
 room_id text NOT NULL,
 payload text NOT NULL,
 digest text NOT NULL,
 expires_at_ms bigint NOT NULL,
 decision text CHECK (decision IN ('approve_once','approve_session','deny')),
 decision_by_owner boolean,
 decided_by_mxid text,
 decided_at_ms bigint,
 consumed boolean NOT NULL DEFAULT false,
 UNIQUE(runtime_id,id)
);
CREATE INDEX management_shared_approvals_pending ON hagency_agent_v1.management_shared_approvals(owner_user_id,expires_at_ms) WHERE NOT consumed;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=25 WHERE singleton AND version=24;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=25);
