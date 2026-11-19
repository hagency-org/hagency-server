-- Adopt pre-Diesel installations at their existing schema version.
DO $hagency_migration$
BEGIN
IF to_regprocedure('hagency_agent_v1.retain_direct_room()') IS NULL THEN
CREATE FUNCTION hagency_agent_v1.retain_direct_room() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF OLD.owner_direct_room_id IS NOT NULL AND NEW.owner_direct_room_id IS DISTINCT FROM OLD.owner_direct_room_id THEN RAISE EXCEPTION 'permanent_owner_direct_room'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_direct_room BEFORE UPDATE ON hagency_agent_v1.agents FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_direct_room();
CREATE FUNCTION hagency_agent_v1.retain_binding_kind() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.scope_kind<>OLD.scope_kind OR NEW.project_id IS DISTINCT FROM OLD.project_id THEN RAISE EXCEPTION 'immutable_binding_scope'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_binding_kind BEFORE UPDATE ON hagency_agent_v1.bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_binding_kind();
END IF;
END $hagency_migration$;
