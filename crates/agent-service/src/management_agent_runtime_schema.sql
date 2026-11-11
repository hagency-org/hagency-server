-- Qualification process lifetime only. Personal leases and v9/v10 bytes unchanged.
CREATE TABLE hagency_agent_v1.management_agent_runtime_commands (
 id text PRIMARY KEY,
 assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_assignments(id),
 operation text NOT NULL CHECK(operation IN('agent.runtime.provision','agent.runtime.stop')),
 created_at_ms bigint NOT NULL CHECK(created_at_ms>0),
 payload text,
 digest text,
 expires_at_ms bigint,
 lease_hash text,
 lease_until_ms bigint NOT NULL DEFAULT 0,
 CHECK((operation='agent.runtime.provision')=(payload IS NOT NULL AND digest IS NOT NULL AND expires_at_ms IS NOT NULL)),
 UNIQUE(assignment_id,operation)
);
CREATE TABLE hagency_agent_v1.management_agent_runtime_observations (
 id text PRIMARY KEY,
 command_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_runtime_commands(id),
 payload text NOT NULL,
 digest text NOT NULL,
 ledger_revision bigint NOT NULL CHECK(ledger_revision>0 AND ledger_revision<=9007199254740991)
);
CREATE INDEX managed_runtime_observations ON hagency_agent_v1.management_agent_runtime_observations(command_id,ledger_revision DESC);
CREATE FUNCTION hagency_agent_v1.retain_managed_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME='management_agent_runtime_observations' THEN RAISE EXCEPTION 'permanent_managed_runtime_history';END IF;
 IF (to_jsonb(NEW)-'lease_hash'-'lease_until_ms') IS DISTINCT FROM (to_jsonb(OLD)-'lease_hash'-'lease_until_ms') THEN RAISE EXCEPTION 'immutable_managed_runtime_command';END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_managed_runtime_command BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_runtime_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_managed_runtime();
CREATE TRIGGER retain_managed_runtime_observation BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_runtime_observations FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_managed_runtime();
CREATE FUNCTION hagency_agent_v1.queue_managed_runtime_stop() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.state='fenced' THEN
 INSERT INTO hagency_agent_v1.management_agent_runtime_commands(id,assignment_id,operation,created_at_ms)
 SELECT 'stop_'||id,assignment_id,'agent.runtime.stop',(extract(epoch from clock_timestamp())*1000)::bigint FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=NEW.id AND operation='agent.runtime.provision' ON CONFLICT(assignment_id,operation) DO NOTHING;
 END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER queue_managed_runtime_stop AFTER UPDATE OF state ON hagency_agent_v1.management_shared_assignments FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.queue_managed_runtime_stop();
CREATE FUNCTION hagency_agent_v1.validate_managed_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_agent_runtime_commands' THEN
  IF NEW.operation='agent.runtime.provision' THEN
   IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id
    WHERE a.id=NEW.assignment_id AND a.state='pending_native' AND m.state='matrix_verified' AND i.state<>'fenced'
    AND NEW.payload::jsonb->>'commandId'=NEW.id AND NEW.payload::jsonb->>'assignmentId'=a.id AND NEW.payload::jsonb->>'operation'=NEW.operation
    AND NEW.payload::jsonb->>'agentReservationId'=a.agent_reservation_id AND NEW.payload::jsonb->>'reserveReceiptDigest'=a.reserve_receipt_digest
    AND NEW.payload::jsonb->>'reserveCommandDigest'=i.digest AND NEW.payload::jsonb->'reserveCommand'=i.payload::jsonb
    AND (NEW.payload::jsonb->>'assignmentGeneration')::bigint=a.generation AND (NEW.payload::jsonb->>'issuedAtMs')::bigint=NEW.created_at_ms
    AND (NEW.payload::jsonb->>'expiresAtMs')::bigint=NEW.expires_at_ms AND NEW.expires_at_ms>NEW.created_at_ms AND NEW.expires_at_ms<=NEW.created_at_ms+30000)
    THEN RAISE EXCEPTION 'managed_runtime_original_binding_required';END IF;
  ELSE
   IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_runtime_commands c ON c.assignment_id=a.id AND c.operation='agent.runtime.provision'
    WHERE a.id=NEW.assignment_id AND a.state='fenced' AND NEW.id='stop_'||c.id AND NEW.created_at_ms>=c.created_at_ms)
    THEN RAISE EXCEPTION 'managed_runtime_stop_binding_required';END IF;
  END IF;
 ELSE
  IF NEW.payload::jsonb->>'observationId'<>NEW.id OR NEW.payload::jsonb->>'commandId'<>NEW.command_id OR (NEW.payload::jsonb->>'ledgerRevision')::bigint<>NEW.ledger_revision THEN RAISE EXCEPTION 'managed_runtime_observation_binding_required';END IF;
 END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER validate_managed_runtime_command BEFORE INSERT ON hagency_agent_v1.management_agent_runtime_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_managed_runtime();
CREATE TRIGGER validate_managed_runtime_observation BEFORE INSERT ON hagency_agent_v1.management_agent_runtime_observations FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_managed_runtime();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=11;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=11);
