-- Adopt pre-Diesel installations at their existing schema version.
DO $hagency_migration$
BEGIN
IF to_regclass('hagency_agent_v1.execution_leases') IS NULL THEN
CREATE TABLE IF NOT EXISTS hagency_agent_v1.execution_leases (
 agent_id text PRIMARY KEY REFERENCES hagency_agent_v1.agents(id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id), device_generation bigint NOT NULL,
 epoch bigint NOT NULL CHECK(epoch>0), expires_at_ms bigint NOT NULL,
 UNIQUE(agent_id,epoch)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.owner_events (
 id text PRIMARY KEY, binding_id text NOT NULL REFERENCES hagency_agent_v1.bindings(id),
 agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id), owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 event_id text NOT NULL, room_id text NOT NULL, requester_mxid text NOT NULL,
 thread_root text NOT NULL, body text NOT NULL, digest text NOT NULL,
 binding_generation bigint NOT NULL,
 state text NOT NULL CHECK(state IN ('pending','offered','acknowledged','running','completed','unknown','cancelled')),
 dispatch_epoch bigint, dispatch_device_id text REFERENCES hagency_agent_v1.devices(id),
 execution_id text, outcome text, created_at_ms bigint NOT NULL,
 UNIQUE(binding_id,event_id), UNIQUE(agent_id,execution_id)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.agent_threads (
 binding_id text NOT NULL REFERENCES hagency_agent_v1.bindings(id), thread_root text NOT NULL,
 PRIMARY KEY(binding_id,thread_root)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.reply_outbox (
 id text PRIMARY KEY, owner_event_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.owner_events(id),
 agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id), binding_id text NOT NULL REFERENCES hagency_agent_v1.bindings(id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id), room_id text NOT NULL,
 puppet_mxid text NOT NULL, thread_root text NOT NULL, body text NOT NULL,
 payload_digest text NOT NULL, matrix_txn_id text NOT NULL UNIQUE,
 binding_generation bigint NOT NULL, dispatch_epoch bigint NOT NULL,
 delivery_epoch bigint NOT NULL CHECK(delivery_epoch>0 AND delivery_epoch>=dispatch_epoch),
 state text NOT NULL CHECK(state IN ('pending','sending','sent','unknown','cancelled')),
 delivery_blocked boolean NOT NULL DEFAULT false,
 worker_token text, worker_until_ms bigint NOT NULL DEFAULT 0, matrix_event_id text,
 created_at_ms bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS owner_events_pending_idx ON hagency_agent_v1.owner_events(owner_user_id,agent_id,state,created_at_ms);
CREATE INDEX IF NOT EXISTS reply_outbox_pending_idx ON hagency_agent_v1.reply_outbox(state,created_at_ms);
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_transport_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='execution_leases' THEN
  IF NEW.agent_id<>OLD.agent_id OR NEW.owner_user_id<>OLD.owner_user_id OR NEW.epoch<OLD.epoch THEN RAISE EXCEPTION 'immutable_lease_scope'; END IF;
 ELSIF TG_TABLE_NAME='owner_events' THEN
  IF OLD.state IN ('completed','cancelled') AND NEW.state<>OLD.state THEN RAISE EXCEPTION 'terminal_dispatch'; END IF;
  IF OLD.state='unknown' AND NEW.state NOT IN ('unknown','completed') THEN RAISE EXCEPTION 'unknown_execution_cannot_replay'; END IF;
  IF OLD.execution_id IS NOT NULL AND (NEW.dispatch_epoch IS DISTINCT FROM OLD.dispatch_epoch OR NEW.dispatch_device_id IS DISTINCT FROM OLD.dispatch_device_id OR NEW.execution_id IS DISTINCT FROM OLD.execution_id) THEN RAISE EXCEPTION 'immutable_execution_identity'; END IF;
  IF NEW.id<>OLD.id OR NEW.binding_id<>OLD.binding_id OR NEW.agent_id<>OLD.agent_id OR NEW.owner_user_id<>OLD.owner_user_id OR NEW.event_id<>OLD.event_id OR NEW.room_id<>OLD.room_id OR NEW.requester_mxid<>OLD.requester_mxid OR NEW.thread_root<>OLD.thread_root OR NEW.body<>OLD.body OR NEW.digest<>OLD.digest OR NEW.binding_generation<>OLD.binding_generation THEN RAISE EXCEPTION 'immutable_event_scope'; END IF;
 ELSIF TG_TABLE_NAME='reply_outbox' THEN
  IF OLD.delivery_blocked AND NOT NEW.delivery_blocked THEN RAISE EXCEPTION 'permanent_reply_delivery_block'; END IF;
  IF OLD.state='sent' AND NEW.state<>OLD.state THEN RAISE EXCEPTION 'terminal_reply'; END IF;
  IF OLD.state='cancelled' AND NEW.state<>OLD.state AND NOT (NEW.state='pending' AND NOT OLD.delivery_blocked AND OLD.worker_token IS NULL AND NEW.delivery_epoch>OLD.delivery_epoch) THEN RAISE EXCEPTION 'terminal_reply'; END IF;
  IF NEW.delivery_epoch<OLD.delivery_epoch THEN RAISE EXCEPTION 'stale_reply_authority'; END IF;
  IF NEW.delivery_epoch<>OLD.delivery_epoch AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.execution_leases l JOIN hagency_agent_v1.devices d ON d.id=l.device_id JOIN hagency_agent_v1.sessions s ON s.id=d.session_id WHERE l.agent_id=NEW.agent_id AND l.owner_user_id=NEW.owner_user_id AND l.epoch=NEW.delivery_epoch AND l.device_generation=d.generation AND d.user_id=NEW.owner_user_id AND s.user_id=NEW.owner_user_id AND NOT d.revoked AND NOT s.revoked AND l.expires_at_ms>(extract(epoch from clock_timestamp())*1000)::bigint AND s.valid_until_ms>(extract(epoch from clock_timestamp())*1000)::bigint) THEN RAISE EXCEPTION 'reply_authority_mismatch'; END IF;
  IF OLD.state='sent' AND NEW.matrix_event_id IS DISTINCT FROM OLD.matrix_event_id THEN RAISE EXCEPTION 'immutable_matrix_reply'; END IF;
  IF NEW.id<>OLD.id OR NEW.owner_event_id<>OLD.owner_event_id OR NEW.agent_id<>OLD.agent_id OR NEW.binding_id<>OLD.binding_id OR NEW.owner_user_id<>OLD.owner_user_id OR NEW.room_id<>OLD.room_id OR NEW.puppet_mxid<>OLD.puppet_mxid OR NEW.thread_root<>OLD.thread_root OR NEW.body<>OLD.body OR NEW.payload_digest<>OLD.payload_digest OR NEW.matrix_txn_id<>OLD.matrix_txn_id OR NEW.binding_generation<>OLD.binding_generation OR NEW.dispatch_epoch<>OLD.dispatch_epoch THEN RAISE EXCEPTION 'immutable_reply_scope'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_lease_scope BEFORE UPDATE ON hagency_agent_v1.execution_leases FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_transport_scope();
CREATE TRIGGER retain_event_scope BEFORE UPDATE ON hagency_agent_v1.owner_events FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_transport_scope();
CREATE TRIGGER retain_reply_scope BEFORE UPDATE ON hagency_agent_v1.reply_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_transport_scope();

CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_transport_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='execution_leases' THEN
  IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.agents a JOIN hagency_agent_v1.devices d ON d.user_id=a.owner_user_id WHERE a.id=NEW.agent_id AND a.owner_user_id=NEW.owner_user_id AND d.id=NEW.device_id) THEN RAISE EXCEPTION 'lease_owner_mismatch'; END IF;
 ELSIF TG_TABLE_NAME='owner_events' THEN
  IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id WHERE b.id=NEW.binding_id AND a.id=NEW.agent_id AND a.owner_user_id=NEW.owner_user_id AND b.room_id=NEW.room_id AND b.generation=NEW.binding_generation) THEN RAISE EXCEPTION 'event_owner_mismatch'; END IF;
 ELSIF TG_TABLE_NAME='reply_outbox' THEN
  IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.owner_events e JOIN hagency_agent_v1.agents a ON a.id=e.agent_id WHERE e.id=NEW.owner_event_id AND e.agent_id=NEW.agent_id AND e.binding_id=NEW.binding_id AND e.owner_user_id=NEW.owner_user_id AND e.room_id=NEW.room_id AND e.thread_root=NEW.thread_root AND e.binding_generation=NEW.binding_generation AND e.dispatch_epoch=NEW.dispatch_epoch AND a.puppet_mxid=NEW.puppet_mxid) THEN RAISE EXCEPTION 'reply_owner_mismatch'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER validate_lease_scope BEFORE INSERT OR UPDATE ON hagency_agent_v1.execution_leases FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_transport_scope();
CREATE TRIGGER validate_event_scope BEFORE INSERT ON hagency_agent_v1.owner_events FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_transport_scope();
CREATE TRIGGER validate_reply_scope BEFORE INSERT ON hagency_agent_v1.reply_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_transport_scope();

-- Started execution identity is permanent recovery evidence, not queue payload.
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_started_execution() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF OLD.execution_id IS NOT NULL THEN RAISE EXCEPTION 'permanent_started_execution'; END IF;
 RETURN OLD;
END;
$$;
CREATE TRIGGER retain_started_execution BEFORE DELETE ON hagency_agent_v1.owner_events FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_started_execution();
CREATE INDEX IF NOT EXISTS owner_events_started_history_idx ON hagency_agent_v1.owner_events(agent_id,id COLLATE "C") WHERE execution_id IS NOT NULL;
END IF;
END $hagency_migration$;
