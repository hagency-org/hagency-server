-- The resource owner is the capacity coordinator. Receipts and executor
-- commands retain their original bytes; the merge changes future authority.
CREATE TABLE hagency_agent_v1.management_owner_coordination_merges (
 contribution_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_contributions(id),
 previous_contribution jsonb NOT NULL,
 previous_allocation_requests jsonb NOT NULL,
 merged_at_ms bigint NOT NULL
);
INSERT INTO hagency_agent_v1.management_owner_coordination_merges
 SELECT c.id,to_jsonb(c),COALESCE((SELECT jsonb_agg(to_jsonb(r)) FROM hagency_agent_v1.management_requests r WHERE r.contribution_id=c.id AND r.kind='project_allocation'),'[]'::jsonb),(extract(epoch from clock_timestamp())*1000)::bigint
 FROM hagency_agent_v1.management_contributions c JOIN hagency_agent_v1.users u ON u.id=c.owner_user_id
 WHERE c.coordinator_mxid<>u.mxid;
UPDATE hagency_agent_v1.management_contributions c
 SET coordinator_mxid=u.mxid,revision=c.revision+1,updated_at_ms=m.merged_at_ms
 FROM hagency_agent_v1.users u,hagency_agent_v1.management_owner_coordination_merges m
 WHERE c.id=m.contribution_id AND c.owner_user_id=u.id;
UPDATE hagency_agent_v1.management_requests r
 SET approver_mxid=u.mxid,revision=r.revision+1
 FROM hagency_agent_v1.management_contributions c JOIN hagency_agent_v1.users u ON u.id=c.owner_user_id
 WHERE r.contribution_id=c.id AND r.kind='project_allocation' AND r.approver_mxid<>u.mxid;
UPDATE hagency_agent_v1.management_outbox o SET state='cancelled'
 FROM hagency_agent_v1.management_requests r JOIN hagency_agent_v1.management_owner_coordination_merges m ON m.contribution_id=r.contribution_id
 WHERE o.request_id=r.id AND r.kind='project_allocation' AND o.state='pending';
INSERT INTO hagency_agent_v1.management_outbox(id,request_id,request_revision,recipient_mxid,transaction_id,created_at_ms)
 SELECT 'owner_merge_'||r.id,r.id,r.revision,r.approver_mxid,'hagency-owner-merge-'||r.id,m.merged_at_ms
 FROM hagency_agent_v1.management_requests r JOIN hagency_agent_v1.management_owner_coordination_merges m ON m.contribution_id=r.contribution_id
 WHERE r.kind='project_allocation' AND r.state='pending'
 ON CONFLICT(request_id,request_revision,recipient_mxid) DO NOTHING;
ALTER TABLE hagency_agent_v1.users ADD CONSTRAINT management_owner_mxid_key UNIQUE(id,mxid);
ALTER TABLE hagency_agent_v1.management_contributions ADD CONSTRAINT resource_owner_is_coordinator
 FOREIGN KEY(owner_user_id,coordinator_mxid) REFERENCES hagency_agent_v1.users(id,mxid);
CREATE FUNCTION hagency_agent_v1.require_owner_coordinated_grant() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.receipt::jsonb->>'coordinatorMxid' IS DISTINCT FROM NEW.receipt::jsonb->>'ownerMxid' THEN
  RAISE EXCEPTION 'resource_owner_is_coordinator';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER require_owner_coordinated_grant BEFORE INSERT ON hagency_agent_v1.management_resource_grants
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.require_owner_coordinated_grant();
CREATE TRIGGER retain_owner_coordination_merge BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_owner_coordination_merges
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_project_role_merge();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=23;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=23);
