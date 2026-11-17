-- Roll back only before publication-first authority has been created.
-- Never erase a submitted owner grant or reinterpret its admission request.
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_deployment WHERE version=31) THEN RAISE EXCEPTION 'publication_first_downgrade_requires_schema_31'; END IF;
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_contributions WHERE publication_first) THEN RAISE EXCEPTION 'publication_first_downgrade_has_authority'; END IF;
END $$;
ALTER TABLE hagency_agent_v1.management_contributions
 DROP CONSTRAINT management_contributions_state_check,
 ADD CONSTRAINT management_contributions_state_check CHECK(state IN ('pending_admission','admitted','rejected','revoked')),
 DROP COLUMN publication_first;
ALTER TABLE hagency_agent_v1.management_requests
 DROP CONSTRAINT management_requests_state_check,
 ADD CONSTRAINT management_requests_state_check CHECK(state IN ('pending','approved','rejected','revoked'));
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=30 WHERE singleton AND version=31;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=30);
