CREATE TABLE hagency_agent_v1.ciphertext_inbox (
 id text PRIMARY KEY, transaction_id text NOT NULL,
 binding_id text NOT NULL REFERENCES hagency_agent_v1.bindings(id),
 agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 agent_generation bigint NOT NULL, binding_generation bigint NOT NULL,
 event_id text NOT NULL, room_id text NOT NULL, sender_mxid text NOT NULL,
 source_digest text NOT NULL, event jsonb, received_at_ms bigint NOT NULL,
 state text NOT NULL CHECK(state IN ('pending','expired')),
 CHECK((state='pending')=(event IS NOT NULL)),
 UNIQUE(binding_id,event_id)
);
CREATE INDEX ciphertext_pending ON hagency_agent_v1.ciphertext_inbox(owner_user_id,agent_id,binding_id,received_at_ms,id) WHERE state='pending';
CREATE FUNCTION hagency_agent_v1.retain_ciphertext() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'ciphertext_tombstone_required'; END IF;
 IF NEW.id<>OLD.id OR NEW.transaction_id<>OLD.transaction_id OR NEW.binding_id<>OLD.binding_id OR NEW.agent_id<>OLD.agent_id OR NEW.owner_user_id<>OLD.owner_user_id OR NEW.agent_generation<>OLD.agent_generation OR NEW.binding_generation<>OLD.binding_generation OR NEW.event_id<>OLD.event_id OR NEW.room_id<>OLD.room_id OR NEW.sender_mxid<>OLD.sender_mxid OR NEW.source_digest<>OLD.source_digest OR NEW.received_at_ms<>OLD.received_at_ms THEN RAISE EXCEPTION 'immutable_ciphertext_scope'; END IF;
 IF OLD.state='expired' OR (NEW.event IS DISTINCT FROM OLD.event AND NOT(NEW.state='expired' AND NEW.event IS NULL)) THEN RAISE EXCEPTION 'immutable_ciphertext'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_ciphertext BEFORE UPDATE OR DELETE ON hagency_agent_v1.ciphertext_inbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_ciphertext();
