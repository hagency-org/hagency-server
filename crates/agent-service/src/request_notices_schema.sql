-- Delivery feedback is server-owned, independent of device leases and usage.
-- The rollout watermark prevents unsolicited notices for historical requests.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.request_notice_deployment (
 singleton boolean PRIMARY KEY CHECK(singleton), since_ms bigint NOT NULL
);
INSERT INTO hagency_agent_v1.request_notice_deployment VALUES(true,(extract(epoch from clock_timestamp())*1000)::bigint) ON CONFLICT DO NOTHING;
CREATE TABLE IF NOT EXISTS hagency_agent_v1.request_notice_outbox (
 id text PRIMARY KEY, owner_event_id text NOT NULL REFERENCES hagency_agent_v1.owner_events(id) ON DELETE CASCADE,
 kind text NOT NULL CHECK(kind IN ('waiting','processing','failed','rejected','interrupted','expired','delivery')),
 matrix_txn_id text NOT NULL UNIQUE,
 state text NOT NULL CHECK(state IN ('pending','sending','unknown','sent','cancelled')),
 worker_token text, worker_until_ms bigint NOT NULL DEFAULT 0, matrix_event_id text,
 UNIQUE(owner_event_id,kind)
);
CREATE INDEX IF NOT EXISTS request_notice_pending_idx ON hagency_agent_v1.request_notice_outbox(state,id);
CREATE OR REPLACE VIEW hagency_agent_v1.request_notice_needed AS
 SELECT e.id AS owner_event_id, e.binding_id, e.requester_mxid, e.thread_root, e.binding_generation,
 CASE
  WHEN e.state='unknown' AND r.id IS NULL THEN 'interrupted'
  WHEN e.state='cancelled' AND e.outcome='unknown' AND e.execution_id IS NULL THEN 'failed'
  WHEN e.state IN ('completed','cancelled') AND e.outcome IN ('failed','rejected') THEN e.outcome
  WHEN e.state='cancelled' AND e.outcome='expired' THEN 'expired'
  WHEN e.state IN ('pending','offered','acknowledged') AND e.created_at_ms<= (extract(epoch from clock_timestamp())*1000)::bigint-30000 THEN 'waiting'
  WHEN e.state='running' AND e.created_at_ms<= (extract(epoch from clock_timestamp())*1000)::bigint-120000 THEN 'processing'
  WHEN r.id IS NOT NULL AND r.state<>'sent' AND NOT r.delivery_blocked AND e.created_at_ms<= (extract(epoch from clock_timestamp())*1000)::bigint-30000 THEN 'delivery'
 END AS kind
 FROM hagency_agent_v1.owner_events e
 LEFT JOIN hagency_agent_v1.reply_outbox r ON r.owner_event_id=e.id
 JOIN hagency_agent_v1.request_notice_deployment d ON d.singleton AND e.created_at_ms>=d.since_ms
 JOIN hagency_agent_v1.bindings b ON b.id=e.binding_id AND b.generation=e.binding_generation
 JOIN hagency_agent_v1.agents a ON a.id=b.agent_id
 JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id
 LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id
 LEFT JOIN hagency_agent_v1.rooms room ON room.room_id=b.room_id AND room.project_id=b.project_id
 WHERE b.state='active' AND a.state='active' AND u.active AND NOT b.admin_project_paused AND NOT b.admin_room_paused
 AND ((b.scope_kind='project' AND p.active AND room.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id));
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_request_notice() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id<>OLD.id OR NEW.owner_event_id<>OLD.owner_event_id OR NEW.kind<>OLD.kind OR NEW.matrix_txn_id<>OLD.matrix_txn_id THEN
  RAISE EXCEPTION 'immutable_request_notice';
 END IF;
 IF OLD.state IN ('sent','cancelled') AND NEW.state<>OLD.state THEN RAISE EXCEPTION 'terminal_request_notice'; END IF;
 IF OLD.state='sent' AND NEW.matrix_event_id IS DISTINCT FROM OLD.matrix_event_id THEN RAISE EXCEPTION 'immutable_request_notice_delivery'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_request_notice BEFORE UPDATE ON hagency_agent_v1.request_notice_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_request_notice();
