-- Adopt pre-Diesel installations at their existing schema version.
DO $hagency_migration$
BEGIN
IF to_regclass('hagency_agent_v1.deployment') IS NULL THEN
CREATE SCHEMA IF NOT EXISTS hagency_agent_v1;
CREATE TABLE IF NOT EXISTS hagency_agent_v1.deployment (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
 version integer NOT NULL CHECK(version=1), server_name text NOT NULL, issuer text NOT NULL
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.users (
 id text PRIMARY KEY, issuer text NOT NULL, subject text NOT NULL, mxid text NOT NULL UNIQUE,
 active boolean NOT NULL DEFAULT true, UNIQUE(issuer,subject)
);
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_identity'; END IF;
 IF NEW.id<>OLD.id OR NEW.issuer<>OLD.issuer OR NEW.subject<>OLD.subject OR NEW.mxid<>OLD.mxid THEN
  RAISE EXCEPTION 'immutable_identity';
 END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_identity ON hagency_agent_v1.users;
CREATE TRIGGER retain_identity BEFORE UPDATE OR DELETE ON hagency_agent_v1.users
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_identity();
CREATE TABLE IF NOT EXISTS hagency_agent_v1.sessions (
 id text PRIMARY KEY, user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 token_hash text NOT NULL UNIQUE, client_id text NOT NULL, valid_until_ms bigint NOT NULL,
 revoked boolean NOT NULL DEFAULT false, UNIQUE(id,user_id)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.devices (
 id text PRIMARY KEY, user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 installation_id text NOT NULL, name text NOT NULL,
 session_id text NOT NULL REFERENCES hagency_agent_v1.sessions(id),
 token_hash text NOT NULL UNIQUE, generation bigint NOT NULL CHECK(generation>0),
 revoked boolean NOT NULL DEFAULT false, UNIQUE(user_id,installation_id), UNIQUE(id,user_id),
 FOREIGN KEY(session_id,user_id) REFERENCES hagency_agent_v1.sessions(id,user_id)
);
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_device_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id<>OLD.id OR NEW.user_id<>OLD.user_id OR NEW.installation_id<>OLD.installation_id THEN
  RAISE EXCEPTION 'immutable_device_scope';
 END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_device_scope ON hagency_agent_v1.devices;
CREATE TRIGGER retain_device_scope BEFORE UPDATE ON hagency_agent_v1.devices
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_device_scope();

CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_session_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id<>OLD.id OR NEW.user_id<>OLD.user_id OR NEW.client_id<>OLD.client_id OR NEW.token_hash<>OLD.token_hash OR (OLD.revoked AND NOT NEW.revoked) THEN
  RAISE EXCEPTION 'immutable_session_scope';
 END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_session_scope ON hagency_agent_v1.sessions;
CREATE TRIGGER retain_session_scope BEFORE UPDATE ON hagency_agent_v1.sessions
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_session_scope();
END IF;
END $hagency_migration$;
