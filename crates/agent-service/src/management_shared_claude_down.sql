-- Test fixtures only. A legacy schema cannot describe retained Claude roots,
-- including their immutable roots embedded in successor commands.
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_deployment WHERE singleton AND version=33) THEN
  IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes
    WHERE payload::jsonb->>'version'='4' OR payload::jsonb->'originalRuntimeCommand'->>'version'='4') THEN
   RAISE EXCEPTION 'shared_claude_downgrade_has_runtime';
  END IF;
  DROP INDEX IF EXISTS hagency_agent_v1.original_shared_runtime;
  CREATE UNIQUE INDEX original_shared_runtime
   ON hagency_agent_v1.management_shared_runtimes(assignment_id)
   WHERE payload::jsonb->>'version'='1' AND NOT issuance_void;
  ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
  UPDATE hagency_agent_v1.management_deployment SET version=32 WHERE singleton AND version=33;
  ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=32);
 END IF;
END $$;
