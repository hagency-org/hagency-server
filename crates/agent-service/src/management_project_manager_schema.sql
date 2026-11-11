-- Project management and project administration are one authority. Keep the
-- legacy wire/storage names so saved commands and immutable history still read.
CREATE TABLE hagency_agent_v1.management_project_role_merges (
 plan_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_projects(id),
 manager_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 previous_authority jsonb,
 merged_at_ms bigint NOT NULL
);
INSERT INTO hagency_agent_v1.management_project_role_merges
 SELECT p.id,p.manager_user_id,to_jsonb(a),(extract(epoch from clock_timestamp())*1000)::bigint
 FROM hagency_agent_v1.management_projects p
 LEFT JOIN hagency_agent_v1.management_project_authorities a ON a.plan_id=p.id
 WHERE a.administrator_user_id IS DISTINCT FROM p.manager_user_id
 AND (a.plan_id IS NOT NULL OR p.state='approved');

UPDATE hagency_agent_v1.management_project_authorities a
 SET administrator_user_id=m.manager_user_id,revision=a.revision+1,assigned_by_user_id=m.manager_user_id
 FROM hagency_agent_v1.management_project_role_merges m WHERE m.plan_id=a.plan_id;
INSERT INTO hagency_agent_v1.management_project_authorities(plan_id,administrator_user_id,revision,assigned_by_user_id)
 SELECT plan_id,manager_user_id,1,manager_user_id FROM hagency_agent_v1.management_project_role_merges
 ON CONFLICT(plan_id) DO NOTHING;

-- A changed authority must rebase and verify its Matrix binding. Existing
-- dependency triggers fence execution and retain unresolved reservation holds.
UPDATE hagency_agent_v1.management_project_setups s
 SET state=CASE WHEN s.state='cancelled' THEN s.state ELSE 'awaiting_matrix_setup' END,
 verified_at_ms=NULL,revision=s.revision+1
 FROM hagency_agent_v1.management_project_role_merges m WHERE m.plan_id=s.plan_id;
UPDATE hagency_agent_v1.management_agent_requests r
 SET state=CASE WHEN r.state='approved_waiting_executor' THEN 'revoked' ELSE r.state END,
 revision=r.revision+1,administrator_user_id=a.administrator_user_id,authority_revision=a.revision
 FROM hagency_agent_v1.management_project_authorities a
 JOIN hagency_agent_v1.management_project_role_merges m ON m.plan_id=a.plan_id
 WHERE r.plan_id=a.plan_id AND r.state IN ('pending_approval','approved_waiting_executor');
UPDATE hagency_agent_v1.management_agent_outbox o SET state='cancelled'
 FROM hagency_agent_v1.management_agent_requests r
 JOIN hagency_agent_v1.management_project_role_merges m ON m.plan_id=r.plan_id
 WHERE o.request_id=r.id AND o.state='pending';
INSERT INTO hagency_agent_v1.management_agent_outbox(id,request_id,request_revision,recipient_mxid,transaction_id,created_at_ms)
 SELECT 'manager_merge_'||r.id,r.id,r.revision,u.mxid,'hagency-manager-merge-'||r.id,m.merged_at_ms
 FROM hagency_agent_v1.management_agent_requests r
 JOIN hagency_agent_v1.management_project_role_merges m ON m.plan_id=r.plan_id
 JOIN hagency_agent_v1.users u ON u.id=m.manager_user_id
 WHERE r.state IN ('pending_approval','revoked')
 ON CONFLICT(request_id,request_revision,recipient_mxid) DO NOTHING;

ALTER TABLE hagency_agent_v1.management_projects ADD CONSTRAINT management_project_manager_key UNIQUE(id,manager_user_id);
ALTER TABLE hagency_agent_v1.management_project_authorities ADD CONSTRAINT project_manager_is_authority
 FOREIGN KEY(plan_id,administrator_user_id) REFERENCES hagency_agent_v1.management_projects(id,manager_user_id);
CREATE FUNCTION hagency_agent_v1.establish_project_manager_authority() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.state='approved' THEN
  INSERT INTO hagency_agent_v1.management_project_authorities(plan_id,administrator_user_id,revision,assigned_by_user_id)
  VALUES(NEW.id,NEW.manager_user_id,1,NEW.manager_user_id) ON CONFLICT(plan_id) DO NOTHING;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER establish_project_manager_authority AFTER INSERT OR UPDATE OF state ON hagency_agent_v1.management_projects
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.establish_project_manager_authority();
CREATE FUNCTION hagency_agent_v1.retain_project_role_merge() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 RAISE EXCEPTION 'permanent_project_role_merge_history';
END;
$$;
CREATE TRIGGER retain_project_role_merge BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_project_role_merges
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_project_role_merge();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=22;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=22);
