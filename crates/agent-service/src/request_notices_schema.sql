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
