-- Explicit OFFLINE maintenance only: NOT executed by application startup.
-- Back up the database/private deployment first; stop its sole application.
-- Preserves permanent owners, puppets, messages, ledger witnesses and generations.
BEGIN;
SELECT pg_advisory_xact_lock(5210750088328902);
SELECT pg_advisory_xact_lock(5210750088328904);
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.domain_deployment WHERE singleton AND version=2)
 OR to_regclass('hagency_agent_v1.execution_instances') IS NULL THEN
  RAISE EXCEPTION 'expected_offline_domain_version_2';
 END IF;
END $$;
ALTER TABLE hagency_agent_v1.agents ADD COLUMN execution_device_id text;
ALTER TABLE hagency_agent_v1.agents ADD CONSTRAINT agents_execution_device_owner_fk
 FOREIGN KEY(execution_device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id);
UPDATE hagency_agent_v1.agents a SET execution_device_id=i.device_id
 FROM hagency_agent_v1.execution_instances i WHERE i.agent_id=a.id AND i.owner_user_id=a.owner_user_id;
-- No assignment leaves NULL; no generation, identity, completed or sent record reset.
UPDATE hagency_agent_v1.execution_leases SET expires_at_ms=0;
UPDATE hagency_agent_v1.owner_events SET state=CASE WHEN state='running' THEN 'unknown' ELSE 'pending' END
 WHERE state IN ('offered','acknowledged','running');
UPDATE hagency_agent_v1.reply_outbox SET state='cancelled' WHERE state='pending';
DROP TABLE hagency_agent_v1.execution_instances;
DROP FUNCTION hagency_agent_v1.retain_execution_instance();
-- Replace the trigger function with the EXACT retain_agent_scope definition
-- in migrations/domain/2026-11-19-000101_domain/up.sql BEFORE committing this maintenance transaction.
-- Required additionally: reject generation rollback and require an increased
-- generation whenever execution_device_id IS DISTINCT FROM its prior value.
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_agent_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_agent_identity'; END IF;
 IF NEW.id<>OLD.id OR NEW.owner_user_id<>OLD.owner_user_id OR NEW.puppet_mxid<>OLD.puppet_mxid THEN RAISE EXCEPTION 'immutable_agent_identity'; END IF;
 IF NEW.generation<OLD.generation OR (NEW.execution_device_id IS DISTINCT FROM OLD.execution_device_id AND NEW.generation<=OLD.generation) THEN RAISE EXCEPTION 'execution_device_generation_required'; END IF;
 IF OLD.state='retired' AND NEW.state<>'retired' THEN RAISE EXCEPTION 'permanent_retirement'; END IF;
 IF OLD.state='retiring' AND NEW.state NOT IN ('retiring','retired') THEN RAISE EXCEPTION 'permanent_retirement'; END IF;
 RETURN NEW;
END;
$$;
ALTER TABLE hagency_agent_v1.domain_deployment DROP CONSTRAINT domain_deployment_version_check;
UPDATE hagency_agent_v1.domain_deployment SET version=3 WHERE singleton;
ALTER TABLE hagency_agent_v1.domain_deployment ADD CONSTRAINT domain_deployment_version_check CHECK(version=3);
COMMIT;
