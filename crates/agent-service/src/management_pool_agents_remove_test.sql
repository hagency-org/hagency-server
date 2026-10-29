-- Schema 29 objects that an older schema never had.
DROP TRIGGER IF EXISTS retain_resource_grant_chain ON hagency_agent_v1.management_resource_grants;
DROP FUNCTION IF EXISTS hagency_agent_v1.retain_resource_grant_chain();
DROP FUNCTION IF EXISTS hagency_agent_v1.canonical_shared_block_json(jsonb);
DROP TABLE IF EXISTS hagency_agent_v1.management_shared_approvals;
-- Test-only removal of schema 24, with legacy constraints restored.
DROP VIEW IF EXISTS hagency_agent_v1.management_agent_request_read;
DROP TRIGGER IF EXISTS require_pool_agent_owner ON hagency_agent_v1.management_agent_requests;
DROP TRIGGER IF EXISTS pool_agent_grant_fence ON hagency_agent_v1.management_resource_grants;
DROP TRIGGER IF EXISTS pool_agent_contribution_fence ON hagency_agent_v1.management_contributions;
DROP FUNCTION IF EXISTS hagency_agent_v1.require_pool_agent_owner(),hagency_agent_v1.fence_pool_agent_dependency();
DROP INDEX IF EXISTS hagency_agent_v1.pool_agent_room_unique;
ALTER TABLE hagency_agent_v1.management_agent_requests DROP CONSTRAINT IF EXISTS managed_agent_capacity_source,
 DROP COLUMN IF EXISTS pool_contribution_id,DROP COLUMN IF EXISTS pool_grant_id,
 ALTER COLUMN plan_id SET NOT NULL,ALTER COLUMN setup_id SET NOT NULL,
 ALTER COLUMN allocation_id SET NOT NULL,ALTER COLUMN domain_project_id SET NOT NULL;
CREATE OR REPLACE FUNCTION hagency_agent_v1.queue_managed_matrix() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE target text;
BEGIN
 IF TG_TABLE_NAME='agents' THEN SELECT id INTO target FROM hagency_agent_v1.management_shared_assignments WHERE agent_id=NEW.id; ELSE target=NEW.id; END IF;
 INSERT INTO hagency_agent_v1.management_agent_matrix(assignment_id,request_id,agent_id,puppet_mxid,room_id,space_id,state)
 SELECT a.id,a.request_id,a.agent_id,p.puppet_mxid,r.room_id,s.space_id,CASE WHEN a.state='fenced' THEN 'fenced' ELSE 'waiting_native' END
 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.agents p ON p.id=a.agent_id JOIN hagency_agent_v1.management_agent_requests r ON r.id=a.request_id JOIN hagency_agent_v1.management_project_setups s ON s.id=r.setup_id WHERE a.id=target ON CONFLICT DO NOTHING;
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments WHERE id=target AND state='fenced') THEN UPDATE hagency_agent_v1.management_agent_matrix SET state='fenced',native_until_ms=0,verified_at_ms=NULL,blocking_reason='agent_authority_revoked' WHERE assignment_id=target; END IF;
 RETURN NEW;
END;
$$;
