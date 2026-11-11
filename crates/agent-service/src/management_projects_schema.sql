-- Project approval records intent only. No capacity, execution assignment or
-- Matrix project is created by this schema or the approval transaction.
CREATE TABLE hagency_agent_v1.management_projects (
 id text PRIMARY KEY,
 manager_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 contribution_id text NOT NULL REFERENCES hagency_agent_v1.management_contributions(id),
 name text NOT NULL, purpose text NOT NULL,
 requested_tokens bigint NOT NULL CHECK(requested_tokens BETWEEN 1 AND 9007199254740991),
 state text NOT NULL CHECK(state IN ('pending_approval','approved','rejected','revoked')),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 request_id text NOT NULL UNIQUE,
 created_at_ms bigint NOT NULL, updated_at_ms bigint NOT NULL
);
ALTER TABLE hagency_agent_v1.management_requests ADD COLUMN project_id text
 REFERENCES hagency_agent_v1.management_projects(id);
ALTER TABLE hagency_agent_v1.management_requests DROP CONSTRAINT management_requests_kind_check;
ALTER TABLE hagency_agent_v1.management_requests ADD CONSTRAINT management_requests_kind_check
 CHECK((kind='resource_admission' AND project_id IS NULL) OR (kind='project_capacity' AND project_id IS NOT NULL));
ALTER TABLE hagency_agent_v1.management_projects ADD CONSTRAINT management_project_request
 FOREIGN KEY(request_id) REFERENCES hagency_agent_v1.management_requests(id) DEFERRABLE INITIALLY DEFERRED;
CREATE INDEX management_projects_manager ON hagency_agent_v1.management_projects(manager_user_id,id);
CREATE INDEX management_projects_resource ON hagency_agent_v1.management_projects(contribution_id,id);
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=3;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=3);
