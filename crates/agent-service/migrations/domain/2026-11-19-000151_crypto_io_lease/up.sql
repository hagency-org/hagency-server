-- A short exclusive reservation replaces the authorization row locks previously held
-- over Matrix HTTP. Each resource uses the same unique key for grants and writers.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.crypto_io_leases (
    resource text NOT NULL,
    resource_id text NOT NULL,
    nonce text NOT NULL,
    until_ms bigint NOT NULL,
    PRIMARY KEY (resource, resource_id)
);
CREATE INDEX IF NOT EXISTS crypto_io_leases_nonce ON hagency_agent_v1.crypto_io_leases(nonce);

CREATE OR REPLACE FUNCTION hagency_agent_v1.guard_crypto_io() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND NEW IS NOT DISTINCT FROM OLD THEN RETURN NEW; END IF;
    -- UPSERT also interlocks a writer whose snapshot predates a concurrent grant.
    INSERT INTO hagency_agent_v1.crypto_io_leases AS lease(resource, resource_id, nonce, until_ms)
        VALUES (TG_TABLE_NAME, OLD.id, '', 0)
        ON CONFLICT (resource, resource_id) DO UPDATE SET nonce='', until_ms=0
        WHERE lease.until_ms <= (extract(epoch FROM clock_timestamp()) * 1000)::bigint;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'agent_crypto_in_progress' USING ERRCODE='55P03';
    END IF;
    DELETE FROM hagency_agent_v1.crypto_io_leases WHERE resource=TG_TABLE_NAME AND resource_id=OLD.id;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
CREATE OR REPLACE TRIGGER guard_crypto_io BEFORE UPDATE OF id, issuer, subject, mxid, active OR DELETE
    ON hagency_agent_v1.users FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.guard_crypto_io();
CREATE OR REPLACE TRIGGER guard_crypto_io BEFORE UPDATE OF id, user_id, client_id, token_hash, revoked, valid_until_ms OR DELETE
    ON hagency_agent_v1.sessions FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.guard_crypto_io();
CREATE OR REPLACE TRIGGER guard_crypto_io BEFORE UPDATE OF id, user_id, session_id, generation, token_hash, revoked OR DELETE
    ON hagency_agent_v1.devices FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.guard_crypto_io();
CREATE OR REPLACE TRIGGER guard_crypto_io BEFORE UPDATE OF id, owner_user_id, puppet_mxid, execution_device_id, state, generation OR DELETE
    ON hagency_agent_v1.agents FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.guard_crypto_io();
