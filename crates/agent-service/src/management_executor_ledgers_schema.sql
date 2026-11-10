-- An owner ledger database numbers its own mutations from 1. A device that
-- replaces its database declares the new one, which opens the next epoch.
-- Receipt numbers collide only inside one epoch. A device that never declares
-- stays in epoch 0, where every existing row already is.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.management_executor_ledgers (
 owner_user_id text NOT NULL,
 executor_device_id text NOT NULL,
 ledger_id text NOT NULL CHECK(ledger_id ~ '^[0-9a-f]{64}$'),
 epoch bigint NOT NULL CHECK(epoch>=1),
 declared_at_ms bigint NOT NULL CHECK(declared_at_ms>0),
 PRIMARY KEY(owner_user_id,executor_device_id,epoch),
 UNIQUE(owner_user_id,executor_device_id,ledger_id),
 FOREIGN KEY(executor_device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id)
);
ALTER TABLE hagency_agent_v1.management_agent_runtime_observations ADD COLUMN IF NOT EXISTS ledger_epoch bigint NOT NULL DEFAULT 0;
ALTER TABLE hagency_agent_v1.management_shared_process_receipts ADD COLUMN IF NOT EXISTS ledger_epoch bigint NOT NULL DEFAULT 0;
ALTER TABLE hagency_agent_v1.management_shared_work_receipts ADD COLUMN IF NOT EXISTS ledger_epoch bigint NOT NULL DEFAULT 0;
ALTER TABLE hagency_agent_v1.management_agent_budget_results ADD COLUMN IF NOT EXISTS ledger_epoch bigint NOT NULL DEFAULT 0;
ALTER TABLE hagency_agent_v1.management_agent_settlement_results ADD COLUMN IF NOT EXISTS ledger_epoch bigint NOT NULL DEFAULT 0;
-- Grants are published with their receipt. Commands receive theirs later, so
-- the epoch is recorded together with that receipt.
ALTER TABLE hagency_agent_v1.management_resource_grants ADD COLUMN IF NOT EXISTS ledger_epoch bigint NOT NULL DEFAULT 0;
ALTER TABLE hagency_agent_v1.management_agent_executor_commands ADD COLUMN IF NOT EXISTS receipt_ledger_epoch bigint NOT NULL DEFAULT 0;
ALTER TABLE hagency_agent_v1.management_executor_commands ADD COLUMN IF NOT EXISTS receipt_ledger_epoch bigint NOT NULL DEFAULT 0;
ALTER TABLE hagency_agent_v1.management_parent_commands ADD COLUMN IF NOT EXISTS receipt_ledger_epoch bigint NOT NULL DEFAULT 0;
-- Same rules as before, plus: the epoch may be set only by the update that
-- records the receipt, and is immutable afterwards.
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_agent_executor() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_agent_executor_history'; END IF;
 IF TG_TABLE_NAME='management_shared_assignments' THEN
  IF (to_jsonb(NEW)-'state') IS DISTINCT FROM (to_jsonb(OLD)-'state') OR OLD.state='fenced' AND NEW.state<>'fenced' THEN RAISE EXCEPTION 'immutable_managed_assignment'; END IF;
 ELSE
  IF (to_jsonb(NEW)-'state'-'lease_hash'-'lease_until_ms'-'receipt'-'receipt_digest'-'receipt_ledger_epoch') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'lease_hash'-'lease_until_ms'-'receipt'-'receipt_digest'-'receipt_ledger_epoch')
   OR OLD.state='recorded' AND NEW.state<>'recorded'
   OR OLD.receipt IS NOT NULL AND (NEW.receipt IS DISTINCT FROM OLD.receipt OR NEW.receipt_digest IS DISTINCT FROM OLD.receipt_digest)
   OR NEW.receipt_ledger_epoch IS DISTINCT FROM OLD.receipt_ledger_epoch AND (OLD.receipt IS NOT NULL OR NEW.receipt IS NULL)
   THEN RAISE EXCEPTION 'immutable_agent_executor_command'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=35 WHERE singleton AND version=34;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=35);
