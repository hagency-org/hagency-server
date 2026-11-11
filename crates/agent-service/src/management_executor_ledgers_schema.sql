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
 -- Whether opening this epoch ended what the earlier database left behind,
 -- and how many objects of each kind that was.
 swept boolean NOT NULL DEFAULT false,
 sweep text,
 CHECK((sweep IS NOT NULL)=swept),
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
-- A declaration that replaces a ledger database ends everything the earlier
-- database left behind. Every such object hangs under a contribution of that
-- owner device, so the contribution carries the mark: the epoch whose
-- declaration replaced it. A marked contribution stays closed, and nothing
-- under it is delivered to the device or accepts a receipt again. No recorded
-- row is rewritten.
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_executor_ledger() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_executor_ledger_history'; END IF;
 IF TG_TABLE_NAME='management_executor_ledgers' THEN
  -- The sweep record is written once, by the declaration that opened the epoch.
  IF (to_jsonb(NEW)-'swept'-'sweep') IS DISTINCT FROM (to_jsonb(OLD)-'swept'-'sweep') OR OLD.swept OR NOT NEW.swept THEN RAISE EXCEPTION 'immutable_executor_ledger'; END IF;
 ELSIF OLD.ledger_replaced_epoch IS NOT NULL AND (NEW.ledger_replaced_epoch IS DISTINCT FROM OLD.ledger_replaced_epoch OR NEW.state<>OLD.state) THEN
  RAISE EXCEPTION 'executor_ledger_replaced';
 END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_executor_ledger ON hagency_agent_v1.management_executor_ledgers;
CREATE TRIGGER retain_executor_ledger BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_executor_ledgers FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_executor_ledger();
ALTER TABLE hagency_agent_v1.management_contributions ADD COLUMN IF NOT EXISTS ledger_replaced_epoch bigint;
ALTER TABLE hagency_agent_v1.management_contributions DROP CONSTRAINT IF EXISTS management_contribution_replaced_ledger;
ALTER TABLE hagency_agent_v1.management_contributions ADD CONSTRAINT management_contribution_replaced_ledger FOREIGN KEY(owner_user_id,executor_device_id,ledger_replaced_epoch) REFERENCES hagency_agent_v1.management_executor_ledgers(owner_user_id,executor_device_id,epoch);
ALTER TABLE hagency_agent_v1.management_contributions DROP CONSTRAINT IF EXISTS management_contribution_replaced_closed;
ALTER TABLE hagency_agent_v1.management_contributions ADD CONSTRAINT management_contribution_replaced_closed CHECK(ledger_replaced_epoch IS NULL OR state IN ('rejected','revoked'));
DROP TRIGGER IF EXISTS retain_replaced_contribution ON hagency_agent_v1.management_contributions;
CREATE TRIGGER retain_replaced_contribution BEFORE UPDATE ON hagency_agent_v1.management_contributions FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_executor_ledger();
-- The new database may offer a resource under the id the earlier one used:
-- a resource id is unique among the contributions that were not replaced.
DO $$ DECLARE existing text; BEGIN
 SELECT conname INTO existing FROM pg_constraint WHERE conrelid='hagency_agent_v1.management_contributions'::regclass AND contype='u' AND pg_get_constraintdef(oid)='UNIQUE (owner_user_id, executor_device_id, resource_id)';
 IF existing IS NOT NULL THEN EXECUTE format('ALTER TABLE hagency_agent_v1.management_contributions DROP CONSTRAINT %I',existing); END IF;
END $$;
CREATE UNIQUE INDEX IF NOT EXISTS management_contribution_resource ON hagency_agent_v1.management_contributions(owner_user_id,executor_device_id,resource_id) WHERE ledger_replaced_epoch IS NULL;
-- A grant, with its allocations and parent settlement, of a replaced database.
CREATE OR REPLACE FUNCTION hagency_agent_v1.executor_ledger_replaced_grant(grant_id text) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_resource_grants g JOIN hagency_agent_v1.management_contributions c ON c.id=g.contribution_id WHERE g.id=$1 AND c.ledger_replaced_epoch IS NOT NULL);
$$;
-- A shared Agent request, with its intent, assignment, processes and work,
-- whose pool or project capacity belonged to a replaced database.
CREATE OR REPLACE FUNCTION hagency_agent_v1.executor_ledger_replaced_request(request_id text) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_requests r LEFT JOIN hagency_agent_v1.management_projects p ON p.id=r.plan_id JOIN hagency_agent_v1.management_contributions c ON c.id=COALESCE(r.pool_contribution_id,p.contribution_id) WHERE r.id=$1 AND c.ledger_replaced_epoch IS NOT NULL);
$$;
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
  -- A command of a replaced ledger database never receives its receipt.
  IF OLD.receipt IS NULL AND NEW.receipt IS NOT NULL
   AND hagency_agent_v1.executor_ledger_replaced_request((SELECT i.request_id FROM hagency_agent_v1.management_agent_intents i WHERE i.id=NEW.intent_id))
   THEN RAISE EXCEPTION 'executor_ledger_replaced'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=35 WHERE singleton AND version=34;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=35);
