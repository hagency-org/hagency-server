-- Test fixtures only. Schema 34 has one number space per device, so it cannot
-- describe receipts numbered by a declared ledger.
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_deployment WHERE singleton AND version IN(35,36)) THEN
  IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_executor_ledgers) THEN
   RAISE EXCEPTION 'executor_ledger_downgrade_has_epoch';
  END IF;
  IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_deployment WHERE singleton AND version=36) THEN
   IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes
      WHERE payload::jsonb->>'version'='5' OR payload::jsonb->'originalRuntimeCommand'->>'version'='5') THEN
    RAISE EXCEPTION 'shared_octos_downgrade_has_runtime';
   END IF;
   DROP INDEX hagency_agent_v1.original_shared_runtime;
   CREATE UNIQUE INDEX original_shared_runtime
    ON hagency_agent_v1.management_shared_runtimes(assignment_id)
    WHERE payload::jsonb->>'version' IN ('1','4') AND NOT issuance_void;
  END IF;
  CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_agent_executor() RETURNS trigger LANGUAGE plpgsql AS $fn$
  BEGIN
   IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_agent_executor_history'; END IF;
   IF TG_TABLE_NAME='management_shared_assignments' THEN
    IF (to_jsonb(NEW)-'state') IS DISTINCT FROM (to_jsonb(OLD)-'state') OR OLD.state='fenced' AND NEW.state<>'fenced' THEN RAISE EXCEPTION 'immutable_managed_assignment'; END IF;
   ELSE
    IF (to_jsonb(NEW)-'state'-'lease_hash'-'lease_until_ms'-'receipt'-'receipt_digest') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'lease_hash'-'lease_until_ms'-'receipt'-'receipt_digest')
     OR OLD.state='recorded' AND NEW.state<>'recorded'
     OR OLD.receipt IS NOT NULL AND (NEW.receipt IS DISTINCT FROM OLD.receipt OR NEW.receipt_digest IS DISTINCT FROM OLD.receipt_digest)
     THEN RAISE EXCEPTION 'immutable_agent_executor_command'; END IF;
   END IF;
   RETURN NEW;
  END;
  $fn$;
  DROP TRIGGER retain_replaced_contribution ON hagency_agent_v1.management_contributions;
  DROP INDEX hagency_agent_v1.management_contribution_resource;
  ALTER TABLE hagency_agent_v1.management_contributions DROP COLUMN ledger_replaced_epoch;
  ALTER TABLE hagency_agent_v1.management_contributions ADD UNIQUE(owner_user_id,executor_device_id,resource_id);
  DROP FUNCTION hagency_agent_v1.executor_ledger_replaced_request(text);
  DROP FUNCTION hagency_agent_v1.executor_ledger_replaced_grant(text);
  DROP TABLE hagency_agent_v1.management_executor_ledgers;
  DROP FUNCTION hagency_agent_v1.retain_executor_ledger();
  ALTER TABLE hagency_agent_v1.management_agent_runtime_observations DROP COLUMN ledger_epoch;
  ALTER TABLE hagency_agent_v1.management_shared_process_receipts DROP COLUMN ledger_epoch;
  ALTER TABLE hagency_agent_v1.management_shared_work_receipts DROP COLUMN ledger_epoch;
  ALTER TABLE hagency_agent_v1.management_agent_budget_results DROP COLUMN ledger_epoch;
  ALTER TABLE hagency_agent_v1.management_agent_settlement_results DROP COLUMN ledger_epoch;
  ALTER TABLE hagency_agent_v1.management_resource_grants DROP COLUMN ledger_epoch;
  ALTER TABLE hagency_agent_v1.management_agent_executor_commands DROP COLUMN receipt_ledger_epoch;
  ALTER TABLE hagency_agent_v1.management_executor_commands DROP COLUMN receipt_ledger_epoch;
  ALTER TABLE hagency_agent_v1.management_parent_commands DROP COLUMN receipt_ledger_epoch;
  ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
  UPDATE hagency_agent_v1.management_deployment SET version=34 WHERE singleton AND version IN(35,36);
  ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=34);
 END IF;
END $$;
