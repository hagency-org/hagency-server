-- Admission only: no shared identity may enter personal workers or transport.
ALTER TABLE hagency_agent_v1.agents ADD COLUMN execution_kind text NOT NULL DEFAULT 'personal' CHECK(execution_kind IN ('personal','shared_project'));
ALTER TABLE hagency_agent_v1.agents ADD CONSTRAINT shared_agent_no_personal_device CHECK(execution_kind='personal' OR execution_device_id IS NULL);
CREATE FUNCTION hagency_agent_v1.fence_unimplemented_shared_agent() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='UPDATE' AND NEW.execution_kind<>OLD.execution_kind THEN RAISE EXCEPTION 'immutable_agent_execution_kind'; END IF;
 IF NEW.execution_kind<>'personal' THEN RAISE EXCEPTION 'managed_agent_native_receipt_required'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER fence_unimplemented_shared_agent BEFORE INSERT OR UPDATE ON hagency_agent_v1.agents FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_unimplemented_shared_agent();

CREATE TABLE hagency_agent_v1.management_agent_requests (
 id text PRIMARY KEY,
 plan_id text NOT NULL REFERENCES hagency_agent_v1.management_projects(id),
 setup_id text NOT NULL REFERENCES hagency_agent_v1.management_project_setups(id),
 setup_binding_revision bigint NOT NULL,
 FOREIGN KEY(setup_id,setup_binding_revision) REFERENCES hagency_agent_v1.management_setup_bindings(setup_id,revision),
 allocation_id text NOT NULL REFERENCES hagency_agent_v1.management_allocations(id),
 allocation_receipt_digest text NOT NULL,
 manager_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 domain_project_id text NOT NULL REFERENCES hagency_agent_v1.projects(id),
 room_id text NOT NULL,
 FOREIGN KEY(room_id,domain_project_id) REFERENCES hagency_agent_v1.rooms(room_id,project_id),
 proposed_agent_id text NOT NULL UNIQUE,
 definition text NOT NULL, definition_digest text NOT NULL,
 requested_tokens bigint NOT NULL CHECK(requested_tokens BETWEEN 1 AND 9007199254740991),
 period_key text NOT NULL,
 administrator_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 authority_revision bigint NOT NULL CHECK(authority_revision>0),
 state text NOT NULL CHECK(state IN ('pending_approval','approved_waiting_executor','rejected','cancelled','revoked')),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 created_at_ms bigint NOT NULL, expires_at_ms bigint NOT NULL CHECK(expires_at_ms>created_at_ms)
);
CREATE INDEX managed_agent_inbox ON hagency_agent_v1.management_agent_requests(administrator_user_id,state,id);
CREATE TABLE hagency_agent_v1.management_agent_decisions (
 request_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_agent_requests(id),
 request_revision bigint NOT NULL,
 administrator_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 authority_revision bigint NOT NULL,
 command_id text NOT NULL, digest text NOT NULL, decision text NOT NULL CHECK(decision IN ('approve','reject')),
 decided_at_ms bigint NOT NULL
);
CREATE TABLE hagency_agent_v1.management_agent_intents (
 id text PRIMARY KEY, request_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_agent_requests(id),
 executor_owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 executor_device_id text NOT NULL, executor_generation bigint NOT NULL,
 FOREIGN KEY(executor_device_id,executor_owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 payload text NOT NULL, digest text NOT NULL,
 state text NOT NULL CHECK(state IN ('unavailable','fenced')) DEFAULT 'unavailable'
);
-- Transactional notification references, with no definition, credentials or model output.
CREATE TABLE hagency_agent_v1.management_agent_outbox (
 id text PRIMARY KEY, request_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_requests(id),
 request_revision bigint NOT NULL, recipient_mxid text NOT NULL,
 transaction_id text NOT NULL UNIQUE,
 state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','cancelled')),
 created_at_ms bigint NOT NULL,
 UNIQUE(request_id,request_revision,recipient_mxid)
);
CREATE FUNCTION hagency_agent_v1.retain_managed_agent_admission() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_managed_agent_admission'; END IF;
 IF TG_TABLE_NAME='management_agent_requests' THEN
  IF (to_jsonb(NEW)-'administrator_user_id'-'authority_revision'-'revision'-'state') IS DISTINCT FROM (to_jsonb(OLD)-'administrator_user_id'-'authority_revision'-'revision'-'state') THEN RAISE EXCEPTION 'immutable_managed_agent_request'; END IF;
  IF NEW.revision<OLD.revision OR (NEW.state<>OLD.state OR NEW.administrator_user_id<>OLD.administrator_user_id OR NEW.authority_revision<>OLD.authority_revision) AND NEW.revision<=OLD.revision THEN RAISE EXCEPTION 'managed_agent_revision_required'; END IF;
  IF OLD.state IN ('cancelled','revoked','rejected') AND NEW.state<>OLD.state THEN RAISE EXCEPTION 'terminal_managed_agent_request'; END IF;
  IF OLD.state='approved_waiting_executor' AND NEW.state NOT IN ('approved_waiting_executor','cancelled','revoked') THEN RAISE EXCEPTION 'terminal_managed_agent_decision'; END IF;
 ELSIF TG_TABLE_NAME='management_agent_intents' THEN
  IF (to_jsonb(NEW)-'state') IS DISTINCT FROM (to_jsonb(OLD)-'state') OR OLD.state='fenced' AND NEW.state<>'fenced' THEN RAISE EXCEPTION 'immutable_managed_agent_intent'; END IF;
 ELSE RAISE EXCEPTION 'immutable_managed_agent_decision';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_managed_agent_request BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_requests FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_managed_agent_admission();
CREATE TRIGGER retain_managed_agent_decision BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_decisions FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_managed_agent_admission();
CREATE TRIGGER retain_managed_agent_intent BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_intents FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_managed_agent_admission();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=8;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=8);
