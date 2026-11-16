CREATE TABLE IF NOT EXISTS hagency_agent_v1.domain_deployment (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
 version integer NOT NULL CHECK(version=4),
 namespace text NOT NULL
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.projects (
 id text PRIMARY KEY, space_id text NOT NULL UNIQUE,
 active boolean NOT NULL DEFAULT true, creation_policy text NOT NULL,
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.rooms (
 room_id text NOT NULL, project_id text NOT NULL REFERENCES hagency_agent_v1.projects(id),
 active boolean NOT NULL DEFAULT true, creation_policy text NOT NULL,
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0), UNIQUE(room_id,project_id),
 PRIMARY KEY(room_id,project_id)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.agents (
 id text PRIMARY KEY, owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 puppet_mxid text NOT NULL UNIQUE, execution_device_id text,
 FOREIGN KEY(execution_device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 display_name text NOT NULL,
 state text NOT NULL CHECK(state IN ('creating','active','suspended','retiring','retired')),
 generation bigint NOT NULL DEFAULT 1 CHECK(generation>0),
 owner_direct_room_id text UNIQUE, UNIQUE(id,owner_user_id)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.bindings (
 id text PRIMARY KEY, agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id),
 project_id text REFERENCES hagency_agent_v1.projects(id), room_id text NOT NULL,
 scope_kind text NOT NULL DEFAULT 'project' CHECK(scope_kind IN ('project','owner_direct')),
 CHECK((scope_kind='project' AND project_id IS NOT NULL) OR (scope_kind='owner_direct' AND project_id IS NULL)),
 state text NOT NULL CHECK(state IN ('joining','active','suspended','leaving','left','revoked')),
 generation bigint NOT NULL DEFAULT 1 CHECK(generation>0),
 admin_project_paused boolean NOT NULL DEFAULT false,
 admin_room_paused boolean NOT NULL DEFAULT false,
 owner_service_paused boolean NOT NULL DEFAULT false,
 UNIQUE(agent_id,room_id),
 FOREIGN KEY(room_id,project_id) REFERENCES hagency_agent_v1.rooms(room_id,project_id)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.scope_pauses (
 kind text NOT NULL CHECK(kind IN ('project','room')), scope_id text NOT NULL,
 paused boolean NOT NULL DEFAULT false, PRIMARY KEY(kind,scope_id)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.domain_commands (
 actor_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id), operation text NOT NULL,
 key text NOT NULL, digest text NOT NULL, agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id),
 binding_id text REFERENCES hagency_agent_v1.bindings(id),
 PRIMARY KEY(actor_user_id,operation,key)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.domain_audit (
 sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, actor_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 operation text NOT NULL, object_id text NOT NULL, at_ms bigint NOT NULL
);
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_agent_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_agent_identity'; END IF;
 IF NEW.id<>OLD.id OR NEW.owner_user_id<>OLD.owner_user_id OR NEW.puppet_mxid<>OLD.puppet_mxid THEN
  RAISE EXCEPTION 'immutable_agent_identity';
 END IF;
 IF NEW.generation<OLD.generation OR (NEW.execution_device_id IS DISTINCT FROM OLD.execution_device_id AND NEW.generation<=OLD.generation) THEN RAISE EXCEPTION 'execution_device_generation_required'; END IF;
 IF OLD.state='retired' AND NEW.state<>'retired' THEN RAISE EXCEPTION 'permanent_retirement'; END IF;
 IF OLD.state='retiring' AND NEW.state NOT IN ('retiring','retired') THEN RAISE EXCEPTION 'permanent_retirement'; END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_agent_scope ON hagency_agent_v1.agents;
CREATE TRIGGER retain_agent_scope BEFORE UPDATE OR DELETE ON hagency_agent_v1.agents FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_scope();
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_binding_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id<>OLD.id OR NEW.agent_id<>OLD.agent_id OR NEW.project_id<>OLD.project_id OR NEW.room_id<>OLD.room_id THEN RAISE EXCEPTION 'immutable_binding_scope'; END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_binding_scope ON hagency_agent_v1.bindings;
CREATE TRIGGER retain_binding_scope BEFORE UPDATE ON hagency_agent_v1.bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_binding_scope();
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_project_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id<>OLD.id OR NEW.space_id<>OLD.space_id THEN RAISE EXCEPTION 'immutable_project_scope'; END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_project_scope ON hagency_agent_v1.projects;
CREATE TRIGGER retain_project_scope BEFORE UPDATE ON hagency_agent_v1.projects FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_project_scope();
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_room_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.room_id<>OLD.room_id OR NEW.project_id<>OLD.project_id THEN RAISE EXCEPTION 'immutable_room_scope'; END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_room_scope ON hagency_agent_v1.rooms;
CREATE TRIGGER retain_room_scope BEFORE UPDATE ON hagency_agent_v1.rooms FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_room_scope();
CREATE INDEX IF NOT EXISTS agents_owner_idx ON hagency_agent_v1.agents(owner_user_id);
CREATE INDEX IF NOT EXISTS bindings_project_idx ON hagency_agent_v1.bindings(project_id,state);
