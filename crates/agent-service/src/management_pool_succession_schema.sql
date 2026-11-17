-- Schema 29: a pool's capacity is a chain of immutable grant receipts on one
-- admission. A successor renews the next UTC month or amends the current one;
-- central rows stay receipts and never reserve capacity. Re-running this file
-- after a test downgrade is a no-op on rows that already have chain columns.
ALTER TABLE hagency_agent_v1.management_resource_grants
 ADD COLUMN IF NOT EXISTS chain_root_id text,
 ADD COLUMN IF NOT EXISTS predecessor_grant_id text,
 ADD COLUMN IF NOT EXISTS succession text,
 ADD COLUMN IF NOT EXISTS period_start_ms bigint;
-- Backfill without firing grant triggers: no grant state changes here.
ALTER TABLE hagency_agent_v1.management_resource_grants DISABLE TRIGGER USER;
UPDATE hagency_agent_v1.management_resource_grants
 SET chain_root_id=id,period_start_ms=(receipt::jsonb->>'periodStartMs')::bigint
 WHERE chain_root_id IS NULL OR period_start_ms IS NULL;
ALTER TABLE hagency_agent_v1.management_resource_grants ENABLE TRIGGER USER;
ALTER TABLE hagency_agent_v1.management_resource_grants
 ALTER COLUMN chain_root_id SET NOT NULL,
 ALTER COLUMN period_start_ms SET NOT NULL,
 DROP CONSTRAINT IF EXISTS management_grant_chain_root,
 DROP CONSTRAINT IF EXISTS management_grant_predecessor,
 DROP CONSTRAINT IF EXISTS management_grant_succession,
 DROP CONSTRAINT IF EXISTS management_resource_grants_state_check;
ALTER TABLE hagency_agent_v1.management_resource_grants
 ADD CONSTRAINT management_grant_chain_root FOREIGN KEY(chain_root_id) REFERENCES hagency_agent_v1.management_resource_grants(id),
 ADD CONSTRAINT management_grant_predecessor FOREIGN KEY(predecessor_grant_id) REFERENCES hagency_agent_v1.management_resource_grants(id),
 ADD CONSTRAINT management_grant_succession CHECK(
  (predecessor_grant_id IS NULL)=(succession IS NULL)
  AND (predecessor_grant_id IS NULL)=(chain_root_id=id)
  AND (succession IS NULL OR succession IN ('renewal','amendment'))
  AND (predecessor_grant_id IS NULL OR predecessor_grant_id<>id)
  AND period_start_ms=(receipt::jsonb->>'periodStartMs')::bigint),
 ADD CONSTRAINT management_resource_grants_state_check CHECK(state IN ('active','revoked','superseded'));
-- One live successor per grant. Revoking a scheduled renewal frees the slot.
CREATE UNIQUE INDEX IF NOT EXISTS management_grant_successor ON hagency_agent_v1.management_resource_grants(predecessor_grant_id)
 WHERE predecessor_grant_id IS NOT NULL AND state<>'revoked';
CREATE INDEX IF NOT EXISTS management_grant_chain ON hagency_agent_v1.management_resource_grants(chain_root_id,period_start_ms);

-- History is append-only: a receipt and its place in its chain never change,
-- and a revoked or superseded grant never becomes active again.
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_resource_grant_chain() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id IS DISTINCT FROM OLD.id OR NEW.contribution_id IS DISTINCT FROM OLD.contribution_id
  OR NEW.owner_user_id IS DISTINCT FROM OLD.owner_user_id OR NEW.executor_device_id IS DISTINCT FROM OLD.executor_device_id
  OR NEW.executor_generation IS DISTINCT FROM OLD.executor_generation
  OR NEW.receipt IS DISTINCT FROM OLD.receipt OR NEW.receipt_digest IS DISTINCT FROM OLD.receipt_digest
  OR NEW.period_key IS DISTINCT FROM OLD.period_key
  OR NEW.chain_root_id IS DISTINCT FROM OLD.chain_root_id OR NEW.predecessor_grant_id IS DISTINCT FROM OLD.predecessor_grant_id
  OR NEW.succession IS DISTINCT FROM OLD.succession OR NEW.period_start_ms IS DISTINCT FROM OLD.period_start_ms
 THEN RAISE EXCEPTION 'immutable_resource_grant_chain'; END IF;
 IF OLD.state<>'active' AND NEW.state<>OLD.state THEN RAISE EXCEPTION 'terminal_resource_grant'; END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_resource_grant_chain ON hagency_agent_v1.management_resource_grants;
CREATE TRIGGER retain_resource_grant_chain BEFORE UPDATE ON hagency_agent_v1.management_resource_grants
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_resource_grant_chain();

-- Pool Agent fencing follows the chain. A superseded grant fences only intents
-- that never reserved: reserved Agents continue under its successor. Revoking a
-- renewal that never started fences only its own intents; revoking any other
-- grant fences every intent authorized through it or one of its predecessors.
CREATE OR REPLACE FUNCTION hagency_agent_v1.fence_pool_agent_dependency() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_resource_grants' THEN
  IF NEW.state='active' OR NEW.state=OLD.state THEN RETURN NEW; END IF;
  IF NEW.state='superseded' THEN
   UPDATE hagency_agent_v1.management_agent_intents i SET state='fenced'
    WHERE i.state<>'fenced'
    AND i.request_id IN (SELECT id FROM hagency_agent_v1.management_agent_requests WHERE pool_contribution_id=NEW.contribution_id)
    AND i.payload::jsonb->'poolGrant'->>'grantId'=NEW.id
    AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_executor_commands w
     WHERE w.intent_id=i.id AND w.operation='agent.reserve' AND w.receipt::jsonb->>'state'='reserved');
  ELSIF NEW.period_start_ms>(extract(epoch from clock_timestamp())*1000)::bigint THEN
   UPDATE hagency_agent_v1.management_agent_intents SET state='fenced'
    WHERE state<>'fenced'
    AND request_id IN (SELECT id FROM hagency_agent_v1.management_agent_requests WHERE pool_contribution_id=NEW.contribution_id)
    AND (payload::jsonb->'poolGrant'->>'grantId'=NEW.id
     OR request_id IN (SELECT id FROM hagency_agent_v1.management_agent_requests WHERE pool_grant_id=NEW.id));
  ELSE
   UPDATE hagency_agent_v1.management_agent_intents SET state='fenced'
    WHERE state<>'fenced'
    AND request_id IN (SELECT id FROM hagency_agent_v1.management_agent_requests WHERE pool_contribution_id=NEW.contribution_id)
    AND (payload::jsonb->'poolGrant'->>'grantId' IN (
      WITH RECURSIVE up(id,predecessor,depth) AS (
       SELECT g.id,g.predecessor_grant_id,0 FROM hagency_agent_v1.management_resource_grants g WHERE g.id=NEW.id
       UNION ALL
       SELECT g.id,g.predecessor_grant_id,up.depth+1 FROM hagency_agent_v1.management_resource_grants g JOIN up ON g.id=up.predecessor WHERE up.depth<4096)
      SELECT id FROM up)
     OR request_id IN (SELECT id FROM hagency_agent_v1.management_agent_requests WHERE pool_grant_id=NEW.id));
  END IF;
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

ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=29 WHERE singleton AND version=28;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=29);
