-- Finite process-result tickets are independent of discovery's latest lease.
-- Observation/recovery cannot replace an in-flight original execute ticket.
CREATE TABLE hagency_agent_v1.management_shared_result_tickets (
 runtime_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 token_hash text NOT NULL CHECK(token_hash ~ '^[0-9a-f]{64}$'),
 executor_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id),
 executor_generation bigint NOT NULL CHECK(executor_generation BETWEEN 1 AND 9007199254740991),
 mode text NOT NULL CHECK(mode IN('execute','observe','cleanup')),
 issued_at_ms bigint NOT NULL,
 expires_at_ms bigint NOT NULL CHECK(expires_at_ms>issued_at_ms AND expires_at_ms<=issued_at_ms+30000),
 PRIMARY KEY(runtime_id,token_hash)
);
-- Keep an already-issued finite ticket valid across this additive migration.
-- Legacy rows do not retain the issuer generation. Bind conservatively to the
-- original assignment generation; newer-generation cleanup recovers a fresh
-- ticket instead of inheriting an ambiguously issued capability.
INSERT INTO hagency_agent_v1.management_shared_result_tickets
 SELECT r.id,r.lease_hash,a.executor_device_id,a.executor_generation,r.lease_mode,
        r.lease_until_ms-30000,r.lease_until_ms
 FROM hagency_agent_v1.management_shared_runtimes r
 JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id
 WHERE r.lease_hash IS NOT NULL AND r.lease_until_ms>(extract(epoch from clock_timestamp())*1000)::bigint;
CREATE FUNCTION hagency_agent_v1.retain_shared_result_ticket() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='UPDATE' OR OLD.expires_at_ms>(extract(epoch from clock_timestamp())*1000)::bigint THEN
  RAISE EXCEPTION 'immutable_live_shared_result_ticket';
 END IF;
 RETURN OLD;
END;
$$;
CREATE TRIGGER retain_shared_result_ticket BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_result_tickets FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_result_ticket();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=15;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=15);
