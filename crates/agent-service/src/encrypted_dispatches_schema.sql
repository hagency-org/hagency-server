-- Separate state names keep old plaintext poll/start code closed on downgrade.
ALTER TABLE hagency_agent_v1.owner_events DROP CONSTRAINT owner_events_state_check;
ALTER TABLE hagency_agent_v1.owner_events ADD CONSTRAINT owner_events_state_check CHECK(state IN ('pending','offered','acknowledged','running','completed','unknown','cancelled','encrypted_pending','encrypted_offered','encrypted_acknowledged','encrypted_running','encrypted_completed','encrypted_unknown','encrypted_cancelled'));
CREATE TABLE hagency_agent_v1.encrypted_dispatches (
 dispatch_id text PRIMARY KEY REFERENCES hagency_agent_v1.owner_events(id) DEFERRABLE INITIALLY DEFERRED,
 inbox_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.ciphertext_inbox(id),
 source_digest text NOT NULL, projection jsonb NOT NULL, seal_digest text NOT NULL UNIQUE
);
CREATE FUNCTION hagency_agent_v1.retain_encrypted_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'immutable_encrypted_dispatch'; END;
$$;
CREATE TRIGGER retain_encrypted_dispatch BEFORE UPDATE OR DELETE ON hagency_agent_v1.encrypted_dispatches FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_encrypted_dispatch();
CREATE FUNCTION hagency_agent_v1.fence_encrypted_event() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE encrypted boolean;
BEGIN
 SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.encrypted_dispatches WHERE dispatch_id=NEW.id) INTO encrypted;
 IF encrypted IS DISTINCT FROM (NEW.state LIKE 'encrypted_%') THEN RAISE EXCEPTION 'encrypted_event_mode_mismatch'; END IF;
 IF TG_OP='INSERT' AND encrypted AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.encrypted_dispatches d JOIN hagency_agent_v1.ciphertext_inbox c ON c.id=d.inbox_id WHERE d.dispatch_id=NEW.id AND d.source_digest=c.source_digest AND c.binding_id=NEW.binding_id AND c.agent_id=NEW.agent_id AND c.owner_user_id=NEW.owner_user_id AND c.event_id=NEW.event_id AND c.room_id=NEW.room_id AND c.sender_mxid=NEW.requester_mxid AND c.binding_generation=NEW.binding_generation AND NEW.body::jsonb=c.event) THEN RAISE EXCEPTION 'encrypted_event_source_mismatch'; END IF;
 IF TG_OP='UPDATE' THEN
  IF OLD.state IN ('encrypted_completed','encrypted_cancelled') AND NEW.state<>OLD.state THEN RAISE EXCEPTION 'terminal_encrypted_dispatch'; END IF;
  IF OLD.state='encrypted_unknown' AND NEW.state NOT IN ('encrypted_unknown','encrypted_completed') THEN RAISE EXCEPTION 'unknown_encrypted_execution_cannot_replay'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER fence_encrypted_event BEFORE INSERT OR UPDATE ON hagency_agent_v1.owner_events FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_encrypted_event();
