-- Lossy downgrade of the Agent service management schema from 29 (pool grant
-- succession) to 28. Prefer setting [management] enable_pool_grant_succession
-- = false instead: that alone stops renewals, amendments and chain-long Agent
-- lifetimes without losing data.
--
-- Run it only with every hagency-server process stopped, on the hagency
-- database, after a pg_dump, and only before starting a schema-28 binary.
--
-- Effects that cannot be undone:
-- * Every active v3 successor (amendment or scheduled renewal) is revoked. A
--   schema-28 server cannot decode v3 receipts (closed DTOs), so its reads of
--   these revoked rows fail closed.
-- * Every superseded grant becomes revoked. Agents carried by an amendment or
--   a renewal are fenced; their managers must request them again.
-- * Chain links are dropped; the receipts keep the predecessor link.
BEGIN;
DO $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_deployment WHERE singleton AND version=29) THEN
  RAISE EXCEPTION 'pool_succession_downgrade_requires_schema_29';
 END IF;
END;
$$;
-- Revoking with the schema-29 trigger still installed fences every Agent that
-- a revoked successor authorized.
UPDATE hagency_agent_v1.management_resource_grants SET state='revoked',revision=revision+1
 WHERE state='active' AND (predecessor_grant_id IS NOT NULL OR receipt::jsonb->>'version'='3');
DROP TRIGGER IF EXISTS retain_resource_grant_chain ON hagency_agent_v1.management_resource_grants;
DROP FUNCTION IF EXISTS hagency_agent_v1.retain_resource_grant_chain();
UPDATE hagency_agent_v1.management_resource_grants SET state='revoked',revision=revision+1 WHERE state='superseded';

-- The schema-28 pool fence (from schema 24), unchanged.
CREATE OR REPLACE FUNCTION hagency_agent_v1.fence_pool_agent_dependency() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_resource_grants' THEN
  IF NEW.state='active' THEN RETURN NEW; END IF;
  UPDATE hagency_agent_v1.management_agent_intents SET state='fenced'
   WHERE state<>'fenced' AND request_id IN (
    SELECT id FROM hagency_agent_v1.management_agent_requests WHERE pool_grant_id=NEW.id);
 ELSE
  IF NEW.state='admitted' AND NEW.executor_device_id=OLD.executor_device_id
   AND NEW.executor_generation=OLD.executor_generation AND NEW.resource_revision=OLD.resource_revision
   AND NEW.request_id=OLD.request_id THEN RETURN NEW; END IF;
  UPDATE hagency_agent_v1.management_agent_intents SET state='fenced'
   WHERE state<>'fenced' AND request_id IN (
    SELECT id FROM hagency_agent_v1.management_agent_requests WHERE pool_contribution_id=NEW.id);
 END IF;
 RETURN NEW;
END;
$$;

DROP INDEX IF EXISTS hagency_agent_v1.management_grant_successor;
DROP INDEX IF EXISTS hagency_agent_v1.management_grant_chain;
ALTER TABLE hagency_agent_v1.management_resource_grants
 DROP CONSTRAINT IF EXISTS management_grant_chain_root,
 DROP CONSTRAINT IF EXISTS management_grant_predecessor,
 DROP CONSTRAINT IF EXISTS management_grant_succession,
 DROP CONSTRAINT IF EXISTS management_resource_grants_state_check;
ALTER TABLE hagency_agent_v1.management_resource_grants
 DROP COLUMN chain_root_id,
 DROP COLUMN predecessor_grant_id,
 DROP COLUMN succession,
 DROP COLUMN period_start_ms,
 ADD CONSTRAINT management_resource_grants_state_check CHECK(state IN ('active','revoked'));

ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=28 WHERE singleton AND version=29;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=28);
COMMIT;
