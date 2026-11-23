-- Installation is serialized with legacy inbox writers by Inbox::open.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.inbox_capacity (
    singleton boolean PRIMARY KEY CHECK (singleton),
    records bigint NOT NULL CHECK (records >= 0),
    bytes bigint NOT NULL CHECK (bytes >= 0)
);
INSERT INTO hagency_agent_v1.inbox_capacity
SELECT true, count(*), coalesce(sum(octet_length(t.body::text)), 0)
FROM hagency_agent_v1.routing_jobs j
JOIN hagency_agent_v1.inbound_transactions t ON t.id=j.transaction_id
WHERE j.state='pending' AND NOT EXISTS (SELECT FROM hagency_agent_v1.inbox_capacity)
ON CONFLICT DO NOTHING;

CREATE OR REPLACE FUNCTION hagency_agent_v1.update_inbox_capacity()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE delta integer := 0; txn text; payload_bytes bigint;
BEGIN
    IF TG_OP <> 'INSERT' AND OLD.state='pending' THEN delta := -1; txn := OLD.transaction_id; END IF;
    IF TG_OP <> 'DELETE' AND NEW.state='pending' THEN delta := delta+1; txn := NEW.transaction_id; END IF;
    IF delta <> 0 THEN
        -- Routed bodies are compacted only after the job transition, in the same transaction.
        SELECT octet_length(body::text) INTO STRICT payload_bytes
        FROM hagency_agent_v1.inbound_transactions WHERE id=txn;
        UPDATE hagency_agent_v1.inbox_capacity SET records=records+delta, bytes=bytes+delta*payload_bytes
        WHERE singleton;
    END IF;
    RETURN NULL;
END;
$$;
CREATE OR REPLACE TRIGGER update_inbox_capacity AFTER INSERT OR UPDATE OF state OR DELETE
ON hagency_agent_v1.routing_jobs FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.update_inbox_capacity();

CREATE INDEX IF NOT EXISTS readiness_receipts_by_age ON hagency_agent_v1.readiness_receipts(received_at_ms);
