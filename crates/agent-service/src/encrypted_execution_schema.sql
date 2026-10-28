CREATE TABLE IF NOT EXISTS hagency_agent_v1.encrypted_publications (
 id text PRIMARY KEY, dispatch_id text NOT NULL REFERENCES hagency_agent_v1.encrypted_dispatches(dispatch_id),
 execution_id text NOT NULL, request_id text NOT NULL, kind text NOT NULL CHECK(kind IN ('reply','file')),
 payload jsonb NOT NULL, state text NOT NULL CHECK(state IN ('possible','sent')),
 matrix_event_id text, created_at_ms bigint NOT NULL,
 UNIQUE(dispatch_id,kind,request_id), CHECK((state='sent')=(matrix_event_id IS NOT NULL))
);
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_encrypted_publication() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'encrypted_publication_retained'; END IF;
 IF (NEW.id,NEW.dispatch_id,NEW.execution_id,NEW.request_id,NEW.kind,NEW.payload,NEW.created_at_ms) IS DISTINCT FROM (OLD.id,OLD.dispatch_id,OLD.execution_id,OLD.request_id,OLD.kind,OLD.payload,OLD.created_at_ms) OR (OLD.state='sent' AND NEW IS DISTINCT FROM OLD) THEN RAISE EXCEPTION 'immutable_encrypted_publication'; END IF;
 RETURN NEW;
END; $$;
DROP TRIGGER IF EXISTS retain_encrypted_publication ON hagency_agent_v1.encrypted_publications;
CREATE TRIGGER retain_encrypted_publication BEFORE UPDATE OR DELETE ON hagency_agent_v1.encrypted_publications FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_encrypted_publication();
