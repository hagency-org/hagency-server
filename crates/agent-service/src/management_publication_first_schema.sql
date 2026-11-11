-- A draft is private to its owner until the same transaction that accepts
-- native capacity submits it to the administrator. No approval creates tokens.
ALTER TABLE hagency_agent_v1.management_contributions
 ADD COLUMN IF NOT EXISTS publication_first boolean NOT NULL DEFAULT false;
ALTER TABLE hagency_agent_v1.management_contributions
 DROP CONSTRAINT management_contributions_state_check;
ALTER TABLE hagency_agent_v1.management_contributions
 ADD CONSTRAINT management_contributions_state_check
 CHECK(state IN ('draft','pending_admission','admitted','rejected','revoked'));
ALTER TABLE hagency_agent_v1.management_requests
 DROP CONSTRAINT management_requests_state_check;
ALTER TABLE hagency_agent_v1.management_requests
 ADD CONSTRAINT management_requests_state_check
 CHECK(state IN ('draft','pending','approved','rejected','revoked'));
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=31 WHERE singleton AND version=30;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=31);
