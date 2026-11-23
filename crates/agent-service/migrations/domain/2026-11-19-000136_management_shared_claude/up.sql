-- Adopt pre-Diesel installations at their existing schema version.
DO $hagency_migration$
BEGIN
IF (SELECT version FROM hagency_agent_v1.management_deployment WHERE singleton) = 32 THEN
-- V4 introduces Claude roots; v1 Codex and v2/v3 successors keep their bytes.
DROP INDEX hagency_agent_v1.original_shared_runtime;
CREATE UNIQUE INDEX original_shared_runtime
 ON hagency_agent_v1.management_shared_runtimes(assignment_id)
 WHERE payload::jsonb->>'version' IN ('1','4') AND NOT issuance_void;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=33 WHERE singleton AND version=32;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=33);
END IF;
END $hagency_migration$;
