CREATE TABLE hagency_agent_v1.management_project_authorities (
 plan_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_projects(id),
 administrator_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 revision bigint NOT NULL CHECK(revision>0),
 assigned_by_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id)
);
CREATE TABLE hagency_agent_v1.management_project_setups (
 id text PRIMARY KEY,
 plan_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_projects(id),
 allocation_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_allocations(id),
 manager_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 plan_revision bigint NOT NULL CHECK(plan_revision>0),
 allocation_receipt_digest text NOT NULL,
 authority_revision bigint NOT NULL CHECK(authority_revision>0),
 definition_digest text NOT NULL,
 mode text NOT NULL CHECK(mode IN ('create','adopt')),
 state text NOT NULL CHECK(state IN ('awaiting_matrix_setup','ready')),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 name text NOT NULL, manager_mxid text NOT NULL, administrator_mxid text NOT NULL,
 space_creation_command_id text NOT NULL UNIQUE,
 room_creation_command_id text NOT NULL UNIQUE,
 space_id text UNIQUE,
 room_id text UNIQUE,
 domain_project_id text UNIQUE REFERENCES hagency_agent_v1.projects(id),
 verified_at_ms bigint,
 CHECK((state='ready')=(space_id IS NOT NULL AND room_id IS NOT NULL AND domain_project_id IS NOT NULL AND verified_at_ms IS NOT NULL))
);
-- P1 deliberately has no agent admission. No generic/personal route, policy
-- update or worker activation may bypass the future assigned-admin decision.
CREATE FUNCTION hagency_agent_v1.require_managed_project_admission() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5210750088328904);
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_project_setups s WHERE s.domain_project_id=NEW.project_id OR s.room_id=NEW.room_id OR s.domain_project_id IN (SELECT project_id FROM hagency_agent_v1.rooms WHERE room_id=NEW.room_id)) THEN
  RAISE EXCEPTION 'managed_agent_admission_required';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER require_managed_project_admission BEFORE INSERT OR UPDATE ON hagency_agent_v1.bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.require_managed_project_admission();
CREATE FUNCTION hagency_agent_v1.require_empty_managed_project() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_managed_project_scope'; END IF;
 IF TG_OP='UPDATE' AND (NEW.id<>OLD.id OR NEW.plan_id<>OLD.plan_id OR NEW.allocation_id<>OLD.allocation_id OR NEW.manager_user_id<>OLD.manager_user_id OR NEW.plan_revision<>OLD.plan_revision OR NEW.allocation_receipt_digest<>OLD.allocation_receipt_digest OR NEW.authority_revision<>OLD.authority_revision OR NEW.definition_digest<>OLD.definition_digest OR NEW.mode<>OLD.mode OR NEW.manager_mxid<>OLD.manager_mxid OR NEW.administrator_mxid<>OLD.administrator_mxid OR NEW.space_creation_command_id<>OLD.space_creation_command_id OR NEW.room_creation_command_id<>OLD.room_creation_command_id) THEN
  RAISE EXCEPTION 'immutable_managed_project_scope';
 END IF;
 PERFORM pg_advisory_xact_lock(5210750088328904);
 IF NEW.domain_project_id IS NOT NULL AND EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.project_id=NEW.domain_project_id OR b.room_id=NEW.room_id OR b.room_id IN (SELECT room_id FROM hagency_agent_v1.rooms WHERE project_id=NEW.domain_project_id)) THEN
  RAISE EXCEPTION 'managed_project_has_bindings';
 END IF;
 IF TG_OP='UPDATE' AND OLD.domain_project_id IS NOT NULL AND (NEW.domain_project_id IS DISTINCT FROM OLD.domain_project_id OR NEW.space_id IS DISTINCT FROM OLD.space_id OR NEW.room_id IS DISTINCT FROM OLD.room_id) THEN
  RAISE EXCEPTION 'immutable_managed_project_link';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER require_empty_managed_project BEFORE INSERT OR UPDATE OR DELETE ON hagency_agent_v1.management_project_setups FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.require_empty_managed_project();
-- Registering another child cannot launder an existing personal/direct binding
-- into a managed project. Managed child identities cannot be moved out either.
CREATE FUNCTION hagency_agent_v1.require_empty_managed_room() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(5210750088328904);
 IF TG_OP='DELETE' THEN
  IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_project_setups WHERE domain_project_id=OLD.project_id) THEN RAISE EXCEPTION 'permanent_managed_room_scope'; END IF;
  RETURN OLD;
 END IF;
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_project_setups WHERE domain_project_id=NEW.project_id) AND EXISTS(SELECT 1 FROM hagency_agent_v1.bindings WHERE room_id=NEW.room_id) THEN
  RAISE EXCEPTION 'managed_project_has_bindings';
 END IF;
 IF TG_OP='UPDATE' AND (OLD.project_id IS DISTINCT FROM NEW.project_id OR OLD.room_id IS DISTINCT FROM NEW.room_id) AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_project_setups WHERE domain_project_id=OLD.project_id) THEN
  RAISE EXCEPTION 'immutable_managed_project_link';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER require_empty_managed_room BEFORE INSERT OR UPDATE OR DELETE ON hagency_agent_v1.rooms FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.require_empty_managed_room();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=6;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=6);
