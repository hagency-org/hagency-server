-- Direct pool requests have no project, setup or intermediate project hold.
-- Existing records retain their original identities, receipts and constraints.
ALTER TABLE hagency_agent_v1.management_agent_requests
 ALTER COLUMN plan_id DROP NOT NULL,
 ALTER COLUMN setup_id DROP NOT NULL,
 ALTER COLUMN allocation_id DROP NOT NULL,
 ALTER COLUMN domain_project_id DROP NOT NULL,
 ADD COLUMN pool_contribution_id text REFERENCES hagency_agent_v1.management_contributions(id),
 ADD COLUMN pool_grant_id text REFERENCES hagency_agent_v1.management_resource_grants(id),
 ADD CONSTRAINT managed_agent_capacity_source CHECK (
   (pool_contribution_id IS NULL AND pool_grant_id IS NULL AND plan_id IS NOT NULL
    AND setup_id IS NOT NULL AND allocation_id IS NOT NULL AND domain_project_id IS NOT NULL)
   OR
   (pool_contribution_id IS NOT NULL AND pool_grant_id IS NOT NULL AND plan_id IS NULL
    AND setup_id IS NULL AND allocation_id IS NULL AND domain_project_id IS NULL
    AND setup_binding_revision=0 AND allocation_receipt_digest='')
 );
CREATE INDEX managed_pool_agent_requests ON hagency_agent_v1.management_agent_requests(pool_contribution_id,id)
 WHERE pool_contribution_id IS NOT NULL;

-- The compatibility projection is private to retained legacy readers. Pool
-- wire responses never contain these historic project/setup fields.
CREATE VIEW hagency_agent_v1.management_agent_request_read AS
 SELECT id,COALESCE(plan_id,id) AS plan_id,COALESCE(setup_id,'') AS setup_id,
 setup_binding_revision,COALESCE(allocation_id,id) AS allocation_id,
 allocation_receipt_digest,manager_user_id,COALESCE(domain_project_id,'') AS domain_project_id,
 room_id,proposed_agent_id,definition,definition_digest,requested_tokens,period_key,
 administrator_user_id,authority_revision,state,revision,created_at_ms,expires_at_ms,
 pool_contribution_id,pool_grant_id
 FROM hagency_agent_v1.management_agent_requests;

CREATE FUNCTION hagency_agent_v1.require_pool_agent_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.pool_contribution_id IS NOT NULL AND NOT EXISTS (
  SELECT 1 FROM hagency_agent_v1.management_contributions c
  JOIN hagency_agent_v1.management_resource_grants g ON g.contribution_id=c.id
  WHERE c.id=NEW.pool_contribution_id AND g.id=NEW.pool_grant_id
    AND c.owner_user_id=NEW.administrator_user_id AND g.owner_user_id=c.owner_user_id
 ) THEN RAISE EXCEPTION 'pool_agent_resource_owner_required'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER require_pool_agent_owner BEFORE INSERT OR UPDATE ON hagency_agent_v1.management_agent_requests
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.require_pool_agent_owner();

CREATE FUNCTION hagency_agent_v1.fence_pool_agent_dependency() RETURNS trigger LANGUAGE plpgsql AS $$
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
CREATE TRIGGER pool_agent_grant_fence AFTER UPDATE ON hagency_agent_v1.management_resource_grants
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_pool_agent_dependency();
CREATE TRIGGER pool_agent_contribution_fence AFTER UPDATE ON hagency_agent_v1.management_contributions
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_pool_agent_dependency();

-- The Matrix room belongs directly to the applicant; no Space or project row.
CREATE OR REPLACE FUNCTION hagency_agent_v1.queue_managed_matrix() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE target text;
BEGIN
 IF TG_TABLE_NAME='agents' THEN SELECT id INTO target FROM hagency_agent_v1.management_shared_assignments WHERE agent_id=NEW.id; ELSE target=NEW.id; END IF;
 INSERT INTO hagency_agent_v1.management_agent_matrix(assignment_id,request_id,agent_id,puppet_mxid,room_id,space_id,state)
 SELECT a.id,a.request_id,a.agent_id,p.puppet_mxid,r.room_id,COALESCE(s.space_id,''),CASE WHEN a.state='fenced' THEN 'fenced' ELSE 'waiting_native' END
 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.agents p ON p.id=a.agent_id
 JOIN hagency_agent_v1.management_agent_requests r ON r.id=a.request_id
 LEFT JOIN hagency_agent_v1.management_project_setups s ON s.id=r.setup_id
 WHERE a.id=target AND (s.id IS NOT NULL OR r.pool_contribution_id IS NOT NULL) ON CONFLICT DO NOTHING;
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments WHERE id=target AND state='fenced') THEN UPDATE hagency_agent_v1.management_agent_matrix SET state='fenced',native_until_ms=0,verified_at_ms=NULL,blocking_reason='agent_authority_revoked' WHERE assignment_id=target; END IF;
 RETURN NEW;
END;
$$;
CREATE UNIQUE INDEX pool_agent_room_unique ON hagency_agent_v1.management_agent_requests(room_id) WHERE pool_contribution_id IS NOT NULL;

ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=24;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=24);
