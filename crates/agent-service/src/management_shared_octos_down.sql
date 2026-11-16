-- Test fixtures only. Old executors cannot interpret retained Octos roots,
-- including immutable roots embedded in successor commands.
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_deployment WHERE singleton AND version=36) THEN
  IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes
    WHERE payload::jsonb->>'version'='5' OR payload::jsonb->'originalRuntimeCommand'->>'version'='5') THEN
   RAISE EXCEPTION 'shared_octos_downgrade_has_runtime';
  END IF;
  DROP INDEX hagency_agent_v1.original_shared_runtime;
  CREATE UNIQUE INDEX original_shared_runtime
   ON hagency_agent_v1.management_shared_runtimes(assignment_id)
   WHERE payload::jsonb->>'version' IN ('1','4') AND NOT issuance_void;
  ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
  UPDATE hagency_agent_v1.management_deployment SET version=35 WHERE singleton AND version=36;
  ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=35);
 END IF;
END $$;
