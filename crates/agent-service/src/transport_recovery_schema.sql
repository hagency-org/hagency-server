-- Derived caches and first-send payloads; immutable execution evidence is retained.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.execution_history_snapshots (
 agent_id text PRIMARY KEY REFERENCES hagency_agent_v1.agents(id) ON DELETE CASCADE,
 count bigint NOT NULL CHECK(count>=0), digest text NOT NULL
);
CREATE OR REPLACE FUNCTION hagency_agent_v1.invalidate_execution_history_snapshot() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' THEN
  IF NEW.execution_id IS NOT NULL THEN DELETE FROM hagency_agent_v1.execution_history_snapshots WHERE agent_id=NEW.agent_id; END IF;
 ELSIF TG_OP='DELETE' THEN
  IF OLD.execution_id IS NOT NULL THEN DELETE FROM hagency_agent_v1.execution_history_snapshots WHERE agent_id=OLD.agent_id; END IF;
 ELSIF (NEW.id,NEW.agent_id,NEW.owner_user_id,NEW.execution_id) IS DISTINCT FROM (OLD.id,OLD.agent_id,OLD.owner_user_id,OLD.execution_id) THEN
  DELETE FROM hagency_agent_v1.execution_history_snapshots WHERE agent_id IN (OLD.agent_id,NEW.agent_id);
 END IF;
 RETURN NULL;
END;$$;
DROP TRIGGER IF EXISTS invalidate_execution_history_snapshot ON hagency_agent_v1.owner_events;
CREATE TRIGGER invalidate_execution_history_snapshot AFTER INSERT OR DELETE OR UPDATE OF id,agent_id,owner_user_id,execution_id ON hagency_agent_v1.owner_events
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.invalidate_execution_history_snapshot();
ALTER TABLE hagency_agent_v1.request_notice_outbox ADD COLUMN IF NOT EXISTS content text;
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_request_notice() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id<>OLD.id OR NEW.owner_event_id<>OLD.owner_event_id OR NEW.kind<>OLD.kind OR NEW.matrix_txn_id<>OLD.matrix_txn_id
 OR (OLD.content IS NOT NULL AND NEW.content IS DISTINCT FROM OLD.content) THEN RAISE EXCEPTION 'immutable_request_notice'; END IF;
 IF OLD.state IN ('sent','cancelled') AND NEW.state<>OLD.state THEN RAISE EXCEPTION 'terminal_request_notice'; END IF;
 IF OLD.state='sent' AND NEW.matrix_event_id IS DISTINCT FROM OLD.matrix_event_id THEN RAISE EXCEPTION 'immutable_request_notice_delivery'; END IF;
 RETURN NEW;
END;$$;
-- Schedule only requests whose state or delivery can produce new feedback.
-- Terminal history is visited once at rollout, never on every worker tick.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.request_notice_candidates (
 owner_event_id text PRIMARY KEY REFERENCES hagency_agent_v1.owner_events(id) ON DELETE CASCADE,
 next_check_ms bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS request_notice_candidates_due ON hagency_agent_v1.request_notice_candidates(next_check_ms,owner_event_id);
CREATE OR REPLACE FUNCTION hagency_agent_v1.request_notice_next_check(event text) RETURNS bigint LANGUAGE sql STABLE AS $$
 SELECT CASE
  WHEN e.state='unknown' AND r.id IS NULL THEN 0
  WHEN e.state='cancelled' AND e.outcome='unknown' AND e.execution_id IS NULL THEN 0
  WHEN e.state IN ('completed','cancelled') AND e.outcome IN ('failed','rejected') THEN 0
  WHEN e.state='cancelled' AND e.outcome='expired' THEN 0
  WHEN e.state IN ('pending','offered','acknowledged') THEN e.created_at_ms+30000
  WHEN e.state='running' THEN e.created_at_ms+120000
  WHEN r.id IS NOT NULL AND r.state<>'sent' AND NOT r.delivery_blocked THEN r.created_at_ms+30000
 END
 FROM hagency_agent_v1.owner_events e LEFT JOIN hagency_agent_v1.reply_outbox r ON r.owner_event_id=e.id
 WHERE e.id=event;
$$;
CREATE OR REPLACE FUNCTION hagency_agent_v1.schedule_request_notice() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE event text; due bigint;
BEGIN
 IF TG_TABLE_NAME='owner_events' THEN event=NEW.id;
 ELSIF TG_OP='DELETE' THEN event=OLD.owner_event_id;
 ELSE event=NEW.owner_event_id; END IF;
 due=hagency_agent_v1.request_notice_next_check(event);
 IF due IS NULL THEN DELETE FROM hagency_agent_v1.request_notice_candidates WHERE owner_event_id=event;
 ELSE INSERT INTO hagency_agent_v1.request_notice_candidates VALUES(event,due)
 ON CONFLICT(owner_event_id) DO UPDATE SET next_check_ms=excluded.next_check_ms; END IF;
 RETURN NULL;
END;$$;
DROP TRIGGER IF EXISTS schedule_request_notice_event ON hagency_agent_v1.owner_events;
CREATE TRIGGER schedule_request_notice_event AFTER INSERT OR UPDATE OF state,outcome,execution_id,created_at_ms ON hagency_agent_v1.owner_events
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.schedule_request_notice();
DROP TRIGGER IF EXISTS schedule_request_notice_reply ON hagency_agent_v1.reply_outbox;
CREATE TRIGGER schedule_request_notice_reply AFTER INSERT OR DELETE OR UPDATE OF state,delivery_blocked,created_at_ms ON hagency_agent_v1.reply_outbox
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.schedule_request_notice();
CREATE TABLE IF NOT EXISTS hagency_agent_v1.request_notice_candidate_deployment(singleton boolean PRIMARY KEY CHECK(singleton));
INSERT INTO hagency_agent_v1.request_notice_candidates(owner_event_id,next_check_ms)
 SELECT e.id,hagency_agent_v1.request_notice_next_check(e.id)
 FROM hagency_agent_v1.owner_events e
 JOIN hagency_agent_v1.request_notice_deployment d ON d.singleton AND e.created_at_ms>=d.since_ms
 WHERE NOT EXISTS(SELECT 1 FROM hagency_agent_v1.request_notice_candidate_deployment)
 AND hagency_agent_v1.request_notice_next_check(e.id) IS NOT NULL ON CONFLICT DO NOTHING;
INSERT INTO hagency_agent_v1.request_notice_candidate_deployment VALUES(true) ON CONFLICT DO NOTHING;
