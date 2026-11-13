-- Octos roots have their own wire capability and remain mutually exclusive
-- with retained Codex and Claude roots for the same assignment.
DROP INDEX hagency_agent_v1.original_shared_runtime;
CREATE UNIQUE INDEX original_shared_runtime
 ON hagency_agent_v1.management_shared_runtimes(assignment_id)
 WHERE payload::jsonb->>'version' IN ('1','4','5') AND NOT issuance_void;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=36 WHERE singleton AND version=35;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=36);
