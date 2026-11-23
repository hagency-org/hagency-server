-- Adopt pre-Diesel installations at their existing schema version.
DO $hagency_migration$
BEGIN
IF to_regclass('hagency_agent_v1.pause_notice_outbox') IS NULL THEN
-- Server-owned status notices never invoke a device, model, or token ledger.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.pause_notice_outbox (
 id text PRIMARY KEY, binding_id text NOT NULL REFERENCES hagency_agent_v1.bindings(id),
 event_id text NOT NULL, requester_mxid text NOT NULL, digest text NOT NULL,
 binding_generation bigint NOT NULL, matrix_txn_id text NOT NULL UNIQUE,
 thread_root text NOT NULL, created_at_ms bigint NOT NULL, queued_at_ms bigint NOT NULL,
 state text NOT NULL CHECK(state IN ('pending','sending','unknown','sent','cancelled')),
 worker_token text, worker_until_ms bigint NOT NULL DEFAULT 0,
 matrix_event_id text, UNIQUE(binding_id,event_id)
);
CREATE INDEX IF NOT EXISTS pause_notice_pending_idx ON hagency_agent_v1.pause_notice_outbox(state,id);
CREATE INDEX IF NOT EXISTS pause_notice_throttle_idx ON hagency_agent_v1.pause_notice_outbox(binding_id,requester_mxid,queued_at_ms);
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_pause_notice_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id<>OLD.id OR NEW.binding_id<>OLD.binding_id OR NEW.event_id<>OLD.event_id OR
    NEW.requester_mxid<>OLD.requester_mxid OR NEW.digest<>OLD.digest OR
    NEW.binding_generation<>OLD.binding_generation OR NEW.matrix_txn_id<>OLD.matrix_txn_id OR
    NEW.thread_root<>OLD.thread_root OR NEW.created_at_ms<>OLD.created_at_ms OR NEW.queued_at_ms<>OLD.queued_at_ms THEN
  RAISE EXCEPTION 'immutable_pause_notice_scope';
 END IF;
 IF OLD.state IN ('sent','cancelled') AND NEW.state<>OLD.state THEN RAISE EXCEPTION 'terminal_pause_notice'; END IF;
 IF OLD.state='sent' AND NEW.matrix_event_id IS DISTINCT FROM OLD.matrix_event_id THEN RAISE EXCEPTION 'immutable_pause_notice_delivery'; END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_pause_notice_scope ON hagency_agent_v1.pause_notice_outbox;
CREATE TRIGGER retain_pause_notice_scope BEFORE UPDATE ON hagency_agent_v1.pause_notice_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_pause_notice_scope();

CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_pause_notice_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id
  JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id WHERE b.id=NEW.binding_id
  AND b.generation=NEW.binding_generation AND b.state='suspended' AND b.owner_service_paused
  AND a.state='active' AND u.active AND NOT b.admin_project_paused AND NOT b.admin_room_paused) THEN
  RAISE EXCEPTION 'pause_notice_scope_mismatch';
 END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS validate_pause_notice_scope ON hagency_agent_v1.pause_notice_outbox;
CREATE TRIGGER validate_pause_notice_scope BEFORE INSERT ON hagency_agent_v1.pause_notice_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_pause_notice_scope();
END IF;
END $hagency_migration$;
