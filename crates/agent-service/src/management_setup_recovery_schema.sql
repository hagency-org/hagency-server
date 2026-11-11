-- Creation identity stays immutable; authorization may advance independently.
CREATE TABLE hagency_agent_v1.management_setup_bindings (
 setup_id text NOT NULL REFERENCES hagency_agent_v1.management_project_setups(id),
 revision bigint NOT NULL CHECK(revision>0),
 allocation_id text NOT NULL REFERENCES hagency_agent_v1.management_allocations(id),
 allocation_receipt_digest text NOT NULL,
 plan_revision bigint NOT NULL CHECK(plan_revision>0),
 authority_revision bigint NOT NULL CHECK(authority_revision>0),
 administrator_mxid text NOT NULL,
 PRIMARY KEY(setup_id,revision)
);
INSERT INTO hagency_agent_v1.management_setup_bindings(setup_id,revision,allocation_id,allocation_receipt_digest,plan_revision,authority_revision,administrator_mxid)
 SELECT id,1,allocation_id,allocation_receipt_digest,plan_revision,authority_revision,administrator_mxid FROM hagency_agent_v1.management_project_setups;
ALTER TABLE hagency_agent_v1.management_project_setups ADD COLUMN current_binding_revision bigint NOT NULL DEFAULT 1 CHECK(current_binding_revision>0);
ALTER TABLE hagency_agent_v1.management_project_setups ADD CONSTRAINT management_current_setup_binding FOREIGN KEY(id,current_binding_revision) REFERENCES hagency_agent_v1.management_setup_bindings(setup_id,revision) DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE hagency_agent_v1.management_project_setups DROP CONSTRAINT management_project_setups_state_check;
ALTER TABLE hagency_agent_v1.management_project_setups ADD CONSTRAINT management_project_setups_state_check CHECK(state IN ('awaiting_matrix_setup','ready','cancelled'));
CREATE FUNCTION hagency_agent_v1.retain_setup_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 RAISE EXCEPTION 'immutable_setup_binding';
END;
$$;
CREATE TRIGGER retain_setup_binding BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_setup_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_setup_binding();
CREATE FUNCTION hagency_agent_v1.advance_setup_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.name<>OLD.name THEN RAISE EXCEPTION 'immutable_setup_creation_descriptor'; END IF;
 IF NEW.state='cancelled' AND NEW.verified_at_ms IS NOT NULL THEN RAISE EXCEPTION 'cancelled_setup_not_ready'; END IF;
 IF NEW.current_binding_revision<OLD.current_binding_revision OR (NEW.current_binding_revision<>OLD.current_binding_revision AND (NEW.revision<=OLD.revision OR NEW.verified_at_ms IS NOT NULL OR NEW.state<>'awaiting_matrix_setup')) THEN RAISE EXCEPTION 'setup_binding_revision_required'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER advance_setup_binding BEFORE UPDATE ON hagency_agent_v1.management_project_setups FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.advance_setup_binding();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=7;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=7);
