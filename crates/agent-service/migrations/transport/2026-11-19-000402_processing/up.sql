-- Adopt pre-Diesel installations at their existing schema version.
DO $hagency_migration$
BEGIN
IF to_regclass('hagency_agent_v1.processing_outbox') IS NULL THEN
CREATE TABLE hagency_agent_v1.processing_outbox (
 id text PRIMARY KEY,
 owner_event_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.owner_events(id),
 execution_id text NOT NULL,
 agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id),
 binding_id text NOT NULL REFERENCES hagency_agent_v1.bindings(id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 room_id text NOT NULL, event_id text NOT NULL, puppet_mxid text NOT NULL,
 binding_generation bigint NOT NULL, dispatch_epoch bigint NOT NULL,
 dispatch_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id),
 content jsonb NOT NULL, matrix_txn_id text NOT NULL UNIQUE,
 state text NOT NULL CHECK(state IN ('pending','sending','sent','unknown','cancelled')),
 delivery_blocked boolean NOT NULL DEFAULT false,
 worker_token text, worker_until_ms bigint NOT NULL DEFAULT 0,
 matrix_event_id text, created_at_ms bigint NOT NULL
);
CREATE INDEX processing_pending_idx ON hagency_agent_v1.processing_outbox(state,id);
CREATE FUNCTION hagency_agent_v1.retain_processing_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_processing_intent'; END IF;
 IF (NEW.id,NEW.owner_event_id,NEW.execution_id,NEW.agent_id,NEW.binding_id,NEW.owner_user_id,NEW.room_id,NEW.event_id,NEW.puppet_mxid,NEW.binding_generation,NEW.dispatch_epoch,NEW.dispatch_device_id,NEW.content,NEW.matrix_txn_id,NEW.created_at_ms)
 IS DISTINCT FROM (OLD.id,OLD.owner_event_id,OLD.execution_id,OLD.agent_id,OLD.binding_id,OLD.owner_user_id,OLD.room_id,OLD.event_id,OLD.puppet_mxid,OLD.binding_generation,OLD.dispatch_epoch,OLD.dispatch_device_id,OLD.content,OLD.matrix_txn_id,OLD.created_at_ms) THEN RAISE EXCEPTION 'immutable_processing_scope'; END IF;
 IF OLD.delivery_blocked AND NOT NEW.delivery_blocked THEN RAISE EXCEPTION 'permanent_processing_block'; END IF;
 IF OLD.state IN ('sent','cancelled') AND (NEW.state<>OLD.state OR NEW.matrix_event_id IS DISTINCT FROM OLD.matrix_event_id) THEN RAISE EXCEPTION 'terminal_processing'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_processing_scope BEFORE UPDATE OR DELETE ON hagency_agent_v1.processing_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_processing_scope();
CREATE FUNCTION hagency_agent_v1.validate_processing_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.owner_events e JOIN hagency_agent_v1.agents a ON a.id=e.agent_id
 WHERE e.id=NEW.owner_event_id AND e.execution_id=NEW.execution_id AND e.state='running'
 AND e.agent_id=NEW.agent_id AND e.binding_id=NEW.binding_id AND e.owner_user_id=NEW.owner_user_id
 AND e.room_id=NEW.room_id AND e.event_id=NEW.event_id AND e.binding_generation=NEW.binding_generation
 AND e.dispatch_epoch=NEW.dispatch_epoch AND e.dispatch_device_id=NEW.dispatch_device_id
 AND a.puppet_mxid=NEW.puppet_mxid) THEN RAISE EXCEPTION 'processing_scope_mismatch'; END IF;
 IF NEW.content<>jsonb_build_object('m.relates_to',jsonb_build_object('rel_type','m.annotation','event_id',NEW.event_id,'key','👀')) THEN RAISE EXCEPTION 'fixed_processing_reaction'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER validate_processing_scope BEFORE INSERT ON hagency_agent_v1.processing_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_processing_scope();
-- Actual clock, live original device generation/session/lease, and durable scope.
CREATE VIEW hagency_agent_v1.processing_sendable AS
 SELECT o.id FROM hagency_agent_v1.processing_outbox o
 JOIN hagency_agent_v1.owner_events e ON e.id=o.owner_event_id
 JOIN hagency_agent_v1.bindings b ON b.id=o.binding_id
 JOIN hagency_agent_v1.agents a ON a.id=o.agent_id
 JOIN hagency_agent_v1.users u ON u.id=o.owner_user_id
 LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id
 LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id
 JOIN hagency_agent_v1.execution_leases l ON l.agent_id=a.id
 JOIN hagency_agent_v1.devices d ON d.id=l.device_id
 JOIN hagency_agent_v1.sessions s ON s.id=d.session_id
 WHERE e.state IN ('running','completed') AND e.execution_id=o.execution_id
 AND e.dispatch_epoch=o.dispatch_epoch AND e.dispatch_device_id=o.dispatch_device_id
 AND b.generation=o.binding_generation AND b.state='active'
 AND NOT b.admin_project_paused AND NOT b.admin_room_paused
 AND a.state='active' AND u.active AND a.owner_user_id=o.owner_user_id
 AND ((b.scope_kind='project' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))
 AND l.owner_user_id=o.owner_user_id AND l.epoch=o.dispatch_epoch
 AND l.device_id=o.dispatch_device_id AND a.execution_device_id=l.device_id
 AND l.device_generation=d.generation AND d.user_id=o.owner_user_id AND s.user_id=o.owner_user_id
 AND NOT d.revoked AND NOT s.revoked
 AND l.expires_at_ms>(extract(epoch from clock_timestamp())*1000)::bigint
 AND s.valid_until_ms>(extract(epoch from clock_timestamp())*1000)::bigint;
END IF;
END $hagency_migration$;
