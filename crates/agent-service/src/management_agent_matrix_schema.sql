-- Matrix membership is independent of native runtime qualification.
CREATE TABLE hagency_agent_v1.management_agent_matrix (
 assignment_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_assignments(id),
 request_id text NOT NULL,
 agent_id text NOT NULL,
 puppet_mxid text NOT NULL,
 room_id text NOT NULL,
 space_id text NOT NULL,
 state text NOT NULL DEFAULT 'waiting_native' CHECK(state IN ('waiting_native','authorized','matrix_verified','fenced')),
 challenge_id text,
 native_until_ms bigint NOT NULL DEFAULT 0,
 effect_started boolean NOT NULL DEFAULT false,
 verified_at_ms bigint,
 cleanup_observed_at_ms bigint,
 blocking_reason text
);
CREATE TABLE hagency_agent_v1.management_agent_matrix_checks (
 id text PRIMARY KEY,
 assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_matrix(assignment_id),
 payload text NOT NULL,
 digest text NOT NULL,
 expires_at_ms bigint NOT NULL,
 lease_hash text,
 lease_until_ms bigint NOT NULL DEFAULT 0,
 observation text,
 observation_digest text,
 CHECK((observation IS NULL)=(observation_digest IS NULL))
);
CREATE INDEX managed_matrix_checks_assignment ON hagency_agent_v1.management_agent_matrix_checks(assignment_id,id);
CREATE FUNCTION hagency_agent_v1.retain_managed_matrix() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_managed_matrix_history'; END IF;
 IF TG_TABLE_NAME='management_agent_matrix' THEN
  IF (to_jsonb(NEW)-'state'-'challenge_id'-'native_until_ms'-'effect_started'-'verified_at_ms'-'cleanup_observed_at_ms'-'blocking_reason') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'challenge_id'-'native_until_ms'-'effect_started'-'verified_at_ms'-'cleanup_observed_at_ms'-'blocking_reason') OR OLD.state='fenced' AND NEW.state<>'fenced'
   OR OLD.effect_started AND NOT NEW.effect_started THEN RAISE EXCEPTION 'immutable_managed_matrix_identity'; END IF;
 ELSE
  IF (to_jsonb(NEW)-'lease_hash'-'lease_until_ms'-'observation'-'observation_digest') IS DISTINCT FROM (to_jsonb(OLD)-'lease_hash'-'lease_until_ms'-'observation'-'observation_digest')
   OR OLD.observation IS NOT NULL AND (NEW.observation IS DISTINCT FROM OLD.observation OR NEW.observation_digest IS DISTINCT FROM OLD.observation_digest)
   THEN RAISE EXCEPTION 'immutable_managed_matrix_check'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_managed_matrix BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_matrix FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_managed_matrix();
CREATE TRIGGER retain_managed_matrix_check BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_matrix_checks FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_managed_matrix();
CREATE FUNCTION hagency_agent_v1.queue_managed_matrix() RETURNS trigger LANGUAGE plpgsql AS $$
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
CREATE TRIGGER queue_managed_matrix AFTER INSERT OR UPDATE OF state ON hagency_agent_v1.management_shared_assignments FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.queue_managed_matrix();
CREATE TRIGGER queue_managed_matrix_identity AFTER INSERT ON hagency_agent_v1.agents FOR EACH ROW WHEN(NEW.execution_kind='shared_project') EXECUTE FUNCTION hagency_agent_v1.queue_managed_matrix();
INSERT INTO hagency_agent_v1.management_agent_matrix(assignment_id,request_id,agent_id,puppet_mxid,room_id,space_id,state)
 SELECT a.id,a.request_id,a.agent_id,p.puppet_mxid,r.room_id,s.space_id,CASE WHEN a.state='fenced' THEN 'fenced' ELSE 'waiting_native' END FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.agents p ON p.id=a.agent_id JOIN hagency_agent_v1.management_agent_requests r ON r.id=a.request_id JOIN hagency_agent_v1.management_project_setups s ON s.id=r.setup_id;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=10;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=10);
