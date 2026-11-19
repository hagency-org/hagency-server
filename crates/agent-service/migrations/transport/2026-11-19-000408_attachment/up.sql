-- Adopt pre-Diesel installations at their existing schema version.
DO $hagency_migration$
BEGIN
IF to_regclass('hagency_agent_v1.event_attachments') IS NULL THEN
CREATE TABLE IF NOT EXISTS hagency_agent_v1.event_attachments (
 dispatch_id text PRIMARY KEY REFERENCES hagency_agent_v1.owner_events(id),
 descriptor jsonb NOT NULL, descriptor_digest text NOT NULL CHECK(length(descriptor_digest)=64)
);
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_event_attachment() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'immutable_event_attachment'; END;
$$;
CREATE TRIGGER retain_event_attachment BEFORE UPDATE OR DELETE ON hagency_agent_v1.event_attachments FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_event_attachment();
END IF;
END $hagency_migration$;
