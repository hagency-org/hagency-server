-- Delivery/admission bridge only. No managed work lease or room binding exists.
CREATE TABLE hagency_agent_v1.management_agent_executor_commands (
 id text PRIMARY KEY,
 intent_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_intents(id),
 operation text NOT NULL CHECK(operation IN ('agent.reserve','agent.fence')),
 created_at_ms bigint NOT NULL CHECK(created_at_ms>0),
 state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','issued','recorded')),
 lease_hash text, lease_until_ms bigint NOT NULL DEFAULT 0,
 receipt text, receipt_digest text,
 CHECK((receipt IS NULL)=(receipt_digest IS NULL)),
 CHECK((state='recorded')=(receipt IS NOT NULL)),
 UNIQUE(intent_id,operation)
);
CREATE TABLE hagency_agent_v1.management_shared_assignments (
 id text PRIMARY KEY,
 request_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_agent_requests(id),
 agent_id text NOT NULL UNIQUE,
 manager_user_id text NOT NULL,
 FOREIGN KEY(agent_id,manager_user_id) REFERENCES hagency_agent_v1.agents(id,owner_user_id) DEFERRABLE INITIALLY DEFERRED,
 executor_owner_user_id text NOT NULL,
 executor_device_id text NOT NULL,
 FOREIGN KEY(executor_device_id,executor_owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 executor_generation bigint NOT NULL CHECK(executor_generation>0),
 agent_reservation_id text NOT NULL,
 reserve_receipt_digest text NOT NULL,
 state text NOT NULL CHECK(state IN ('pending_native','fenced')),
 generation bigint NOT NULL DEFAULT 1 CHECK(generation=1),
 UNIQUE(executor_owner_user_id,executor_device_id,agent_reservation_id)
);
CREATE INDEX managed_agent_executor_scope ON hagency_agent_v1.management_agent_intents(executor_owner_user_id,executor_device_id,id);
CREATE INDEX managed_agent_executor_pending ON hagency_agent_v1.management_agent_executor_commands(intent_id,lease_until_ms,id) WHERE receipt IS NULL;
CREATE FUNCTION hagency_agent_v1.queue_agent_executor() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 INSERT INTO hagency_agent_v1.management_agent_executor_commands(id,intent_id,operation,created_at_ms)
 VALUES(NEW.id,NEW.id,'agent.reserve',(NEW.payload::jsonb->>'issuedAtMs')::bigint) ON CONFLICT(intent_id,operation) DO NOTHING;
 IF NEW.state='fenced' THEN
  INSERT INTO hagency_agent_v1.management_agent_executor_commands(id,intent_id,operation,created_at_ms)
  VALUES('fence_'||NEW.id,NEW.id,'agent.fence',(extract(epoch from clock_timestamp())*1000)::bigint) ON CONFLICT(intent_id,operation) DO NOTHING;
  UPDATE hagency_agent_v1.management_shared_assignments SET state='fenced' WHERE request_id=NEW.request_id;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER queue_agent_executor AFTER INSERT OR UPDATE OF state ON hagency_agent_v1.management_agent_intents FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.queue_agent_executor();
INSERT INTO hagency_agent_v1.management_agent_executor_commands(id,intent_id,operation,created_at_ms)
 SELECT id,id,'agent.reserve',(payload::jsonb->>'issuedAtMs')::bigint FROM hagency_agent_v1.management_agent_intents;
INSERT INTO hagency_agent_v1.management_agent_executor_commands(id,intent_id,operation,created_at_ms)
 SELECT 'fence_'||id,id,'agent.fence',(extract(epoch from clock_timestamp())*1000)::bigint FROM hagency_agent_v1.management_agent_intents WHERE state='fenced';

CREATE FUNCTION hagency_agent_v1.retain_agent_executor() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_agent_executor_history'; END IF;
 IF TG_TABLE_NAME='management_shared_assignments' THEN
  IF (to_jsonb(NEW)-'state') IS DISTINCT FROM (to_jsonb(OLD)-'state') OR OLD.state='fenced' AND NEW.state<>'fenced' THEN RAISE EXCEPTION 'immutable_managed_assignment'; END IF;
 ELSE
  IF (to_jsonb(NEW)-'state'-'lease_hash'-'lease_until_ms'-'receipt'-'receipt_digest') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'lease_hash'-'lease_until_ms'-'receipt'-'receipt_digest')
   OR OLD.state='recorded' AND NEW.state<>'recorded'
   OR OLD.receipt IS NOT NULL AND (NEW.receipt IS DISTINCT FROM OLD.receipt OR NEW.receipt_digest IS DISTINCT FROM OLD.receipt_digest)
   THEN RAISE EXCEPTION 'immutable_agent_executor_command'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_agent_executor BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_executor_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_executor();
CREATE TRIGGER retain_managed_assignment BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_assignments FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_executor();
CREATE FUNCTION hagency_agent_v1.validate_managed_assignment() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_requests r
 JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=r.id
 JOIN hagency_agent_v1.management_agent_executor_commands w ON w.intent_id=i.id AND w.operation='agent.reserve'
 WHERE r.id=NEW.request_id AND r.proposed_agent_id=NEW.agent_id AND r.manager_user_id=NEW.manager_user_id
 AND i.executor_owner_user_id=NEW.executor_owner_user_id AND i.executor_device_id=NEW.executor_device_id AND i.executor_generation=NEW.executor_generation
 AND w.receipt_digest=NEW.reserve_receipt_digest AND w.receipt::jsonb->>'state'='reserved'
 AND w.receipt::jsonb->>'agentReservationId'=NEW.agent_reservation_id
 AND w.receipt::jsonb->>'agentId'=NEW.agent_id
 AND (NEW.state='fenced' OR r.state='approved_waiting_executor' AND i.state<>'fenced'))
 THEN RAISE EXCEPTION 'managed_assignment_receipt_required'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER validate_managed_assignment BEFORE INSERT OR UPDATE ON hagency_agent_v1.management_shared_assignments FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_managed_assignment();

-- Keep the historic trigger name so previous migration removal remains explicit.
CREATE OR REPLACE FUNCTION hagency_agent_v1.fence_unimplemented_shared_agent() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='UPDATE' AND NEW.execution_kind<>OLD.execution_kind THEN RAISE EXCEPTION 'immutable_agent_execution_kind'; END IF;
 IF NEW.execution_kind='shared_project' THEN
  IF NEW.execution_device_id IS NOT NULL OR NEW.owner_direct_room_id IS NOT NULL
   OR NEW.state NOT IN ('suspended','retiring','retired')
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a WHERE a.agent_id=NEW.id AND a.manager_user_id=NEW.owner_user_id)
   THEN RAISE EXCEPTION 'managed_agent_native_receipt_required'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE FUNCTION hagency_agent_v1.fence_managed_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE id=NEW.agent_id AND execution_kind='shared_project') THEN RAISE EXCEPTION 'managed_agent_execution_unavailable'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER fence_managed_binding BEFORE INSERT OR UPDATE ON hagency_agent_v1.bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_managed_binding();

-- Revocation creates a cleanup obligation in the same transaction, even when
-- reservation outcome is unknown. The exact fence is materialized from the
-- immutable original command and this row's fixed first-fence timestamp.
CREATE FUNCTION hagency_agent_v1.fence_agent_dependency() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE requests text[];
BEGIN
 IF TG_TABLE_NAME='management_allocations' THEN
  IF NEW.state IN ('reserved','approved_waiting_executor','outcome_unknown') AND NEW.release_command_id IS NULL THEN RETURN NEW; END IF;
  SELECT array_agg(id) INTO requests FROM hagency_agent_v1.management_agent_requests WHERE allocation_id=NEW.id;
 ELSIF TG_TABLE_NAME='management_project_setups' THEN
  IF NEW.state='ready' THEN RETURN NEW; END IF;
  SELECT array_agg(id) INTO requests FROM hagency_agent_v1.management_agent_requests WHERE setup_id=NEW.id;
 ELSIF TG_TABLE_NAME='management_agent_requests' THEN
  IF NEW.state NOT IN ('cancelled','revoked','rejected') THEN RETURN NEW; END IF;
  requests=ARRAY[NEW.id];
 ELSIF TG_TABLE_NAME='management_resource_grants' THEN
  IF NEW.state='active' THEN RETURN NEW; END IF;
  SELECT array_agg(r.id) INTO requests FROM hagency_agent_v1.management_agent_requests r JOIN hagency_agent_v1.management_allocations a ON a.id=r.allocation_id WHERE a.grant_id=NEW.id;
 ELSIF TG_TABLE_NAME='management_contributions' THEN
  IF NEW.state='admitted' THEN RETURN NEW; END IF;
  SELECT array_agg(r.id) INTO requests FROM hagency_agent_v1.management_agent_requests r JOIN hagency_agent_v1.management_projects p ON p.id=r.plan_id WHERE p.contribution_id=NEW.id;
 ELSIF TG_TABLE_NAME='management_projects' THEN
  IF NEW.state='approved' THEN RETURN NEW; END IF;
  SELECT array_agg(id) INTO requests FROM hagency_agent_v1.management_agent_requests WHERE plan_id=NEW.id;
 ELSIF TG_TABLE_NAME='projects' THEN
  IF NEW.active THEN RETURN NEW; END IF;
  SELECT array_agg(id) INTO requests FROM hagency_agent_v1.management_agent_requests WHERE domain_project_id=NEW.id;
 ELSIF TG_TABLE_NAME='rooms' THEN
  IF NEW.active THEN RETURN NEW; END IF;
  SELECT array_agg(id) INTO requests FROM hagency_agent_v1.management_agent_requests WHERE room_id=NEW.room_id;
 ELSIF TG_TABLE_NAME='devices' THEN
  IF NOT NEW.revoked AND NEW.generation=OLD.generation THEN RETURN NEW; END IF;
  SELECT array_agg(request_id) INTO requests FROM hagency_agent_v1.management_agent_intents WHERE executor_device_id=NEW.id;
 ELSIF TG_TABLE_NAME='sessions' THEN
  IF NOT NEW.revoked AND NEW.valid_until_ms>=OLD.valid_until_ms THEN RETURN NEW; END IF;
  SELECT array_agg(i.request_id) INTO requests FROM hagency_agent_v1.management_agent_intents i JOIN hagency_agent_v1.devices d ON d.id=i.executor_device_id WHERE d.session_id=NEW.id;
 ELSIF TG_TABLE_NAME='users' THEN
  IF NEW.active THEN RETURN NEW; END IF;
  SELECT array_agg(r.id) INTO requests FROM hagency_agent_v1.management_agent_requests r JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=r.id WHERE r.manager_user_id=NEW.id OR r.administrator_user_id=NEW.id OR i.executor_owner_user_id=NEW.id;
 END IF;
 UPDATE hagency_agent_v1.management_agent_intents SET state='fenced' WHERE request_id=ANY(requests) AND state<>'fenced';
 RETURN NEW;
END;
$$;
CREATE TRIGGER managed_agent_allocation_fence AFTER UPDATE ON hagency_agent_v1.management_allocations FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_setup_fence AFTER UPDATE ON hagency_agent_v1.management_project_setups FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_request_fence AFTER UPDATE ON hagency_agent_v1.management_agent_requests FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_grant_fence AFTER UPDATE ON hagency_agent_v1.management_resource_grants FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_contribution_fence AFTER UPDATE ON hagency_agent_v1.management_contributions FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_plan_fence AFTER UPDATE ON hagency_agent_v1.management_projects FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_project_fence AFTER UPDATE ON hagency_agent_v1.projects FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_room_fence AFTER UPDATE ON hagency_agent_v1.rooms FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_device_fence AFTER UPDATE ON hagency_agent_v1.devices FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_session_fence AFTER UPDATE ON hagency_agent_v1.sessions FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
CREATE TRIGGER managed_agent_user_fence AFTER UPDATE ON hagency_agent_v1.users FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=9;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=9);
