-- Append-only successor links. Existing v1 command/receipt bytes are untouched.
CREATE TABLE hagency_agent_v1.management_shared_attempts (
 id text PRIMARY KEY,
 assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_assignments(id),
 runtime_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_runtimes(id) DEFERRABLE INITIALLY DEFERRED,
 predecessor_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 original_runtime_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 process_attempt bigint NOT NULL CHECK(process_attempt BETWEEN 2 AND 9007199254740991),
 input_payload text NOT NULL,
 input_digest text NOT NULL,
 created_at_ms bigint NOT NULL,
 UNIQUE(assignment_id,process_attempt)
);
CREATE TRIGGER retain_shared_attempt BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_attempts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_runtime();
ALTER TABLE hagency_agent_v1.management_shared_runtimes DROP CONSTRAINT management_shared_runtimes_assignment_id_key;
CREATE UNIQUE INDEX original_shared_runtime ON hagency_agent_v1.management_shared_runtimes(assignment_id) WHERE payload::jsonb->>'version'='1';
CREATE UNIQUE INDEX active_shared_runtime_head ON hagency_agent_v1.management_shared_runtimes(assignment_id) WHERE NOT permanent_fence;
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_process_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'commandId' IS DISTINCT FROM NEW.runtime_id
   OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision THEN RAISE EXCEPTION 'shared_process_receipt_mismatch'; END IF;
 ELSIF NEW.payload::jsonb->>'version'='2' THEN
  IF NEW.payload::jsonb->>'capability' IS DISTINCT FROM 'hagency.managed_agents.execution.v2'
   OR NEW.payload::jsonb->>'operation' IS DISTINCT FROM 'agent.execution.provision'
   OR NEW.payload::jsonb->>'commandId' IS DISTINCT FROM NEW.id
   OR NEW.payload::jsonb->>'assignmentId' IS DISTINCT FROM NEW.assignment_id
   OR (NEW.payload::jsonb->>'issuedAtMs')::bigint IS DISTINCT FROM NEW.created_at_ms
   OR (NEW.payload::jsonb->>'expiresAtMs')::bigint IS DISTINCT FROM NEW.expires_at_ms
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts x
     JOIN hagency_agent_v1.management_shared_runtimes prior ON prior.id=x.predecessor_id
     JOIN hagency_agent_v1.management_shared_runtimes original ON original.id=x.original_runtime_id
     JOIN hagency_agent_v1.management_shared_assignments a ON a.id=x.assignment_id
     JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id
     JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id
     WHERE x.runtime_id=NEW.id AND x.assignment_id=NEW.assignment_id
       AND a.state='pending_native' AND i.state<>'fenced' AND m.state='matrix_verified'
       AND prior.assignment_id=a.id AND prior.state='stopped' AND prior.permanent_fence
       AND original.assignment_id=a.id AND original.payload::jsonb->>'version'='1'
       AND NEW.payload::jsonb->'originalRuntimeCommand'=original.payload::jsonb
       AND NEW.payload::jsonb->>'originalRuntimeDigest'=original.digest
       AND NEW.payload::jsonb->>'agentReservationId'=a.agent_reservation_id
       AND (NEW.payload::jsonb->>'assignmentGeneration')::bigint=a.generation
       AND NEW.payload::jsonb->>'startIntentId'=x.id
       AND (NEW.payload::jsonb->>'processAttempt')::bigint=x.process_attempt
       AND NEW.payload::jsonb->'predecessor'->>'commandId'=prior.id
       AND NEW.payload::jsonb->'predecessor'->>'commandDigest'=prior.digest
       AND NEW.payload::jsonb->'predecessor'->>'processDigest'=prior.process_digest
       AND (NEW.payload::jsonb->'predecessor'->>'processAttempt')::bigint=x.process_attempt-1
       AND coalesce((prior.payload::jsonb->>'processAttempt')::bigint,1)=x.process_attempt-1
       AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts o
         WHERE o.runtime_id=prior.id AND o.digest=NEW.payload::jsonb->'predecessor'->>'stoppedReceiptDigest'
          AND o.payload::jsonb->>'state'='stopped' AND o.payload::jsonb->>'processDigest'=prior.process_digest))
  THEN RAISE EXCEPTION 'fresh_shared_attempt_required';END IF;
 ELSE
  IF NEW.payload::jsonb->>'capability' IS DISTINCT FROM 'hagency.managed_agents.execution.v1'
   OR NEW.payload::jsonb->>'operation' IS DISTINCT FROM 'agent.execution.provision'
   OR NEW.payload::jsonb->>'policy' IS DISTINCT FROM 'codex_text_v1'
   OR (NEW.payload::jsonb->>'version')::integer IS DISTINCT FROM 1
   OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=NEW.assignment_id)
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id
    WHERE a.id=NEW.assignment_id AND a.state='pending_native' AND i.state<>'fenced' AND m.state='matrix_verified'
    AND NEW.payload::jsonb->>'commandId'=NEW.id AND NEW.payload::jsonb->>'assignmentId'=a.id
    AND NEW.payload::jsonb->>'agentReservationId'=a.agent_reservation_id
    AND NEW.payload::jsonb->>'reserveReceiptDigest'=a.reserve_receipt_digest
    AND NEW.payload::jsonb->>'reserveCommandDigest'=i.digest AND NEW.payload::jsonb->'reserveCommand'=i.payload::jsonb
    AND (NEW.payload::jsonb->>'assignmentGeneration')::bigint=a.generation
    AND (NEW.payload::jsonb->>'issuedAtMs')::bigint=NEW.created_at_ms
    AND (NEW.payload::jsonb->>'expiresAtMs')::bigint=NEW.expires_at_ms)
  THEN RAISE EXCEPTION 'fresh_shared_assignment_required'; END IF;
 END IF; RETURN NEW;
END;
$$;

-- A previously delivered Matrix event belongs permanently to its first work.
CREATE TABLE hagency_agent_v1.management_shared_event_pins (
 assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_assignments(id),
 event_id text NOT NULL,
 work_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_work(id) DEFERRABLE INITIALLY DEFERRED,
 PRIMARY KEY(assignment_id,event_id)
);
INSERT INTO hagency_agent_v1.management_shared_event_pins SELECT r.assignment_id,w.event_id,w.id FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id;
CREATE TRIGGER retain_shared_event_pin BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_event_pins FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_runtime();
CREATE FUNCTION hagency_agent_v1.pin_shared_event() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 INSERT INTO hagency_agent_v1.management_shared_event_pins(assignment_id,event_id,work_id)
 SELECT assignment_id,NEW.event_id,NEW.id FROM hagency_agent_v1.management_shared_runtimes WHERE id=NEW.runtime_id;
 RETURN NEW;
END;
$$;
CREATE TRIGGER pin_shared_event BEFORE INSERT ON hagency_agent_v1.management_shared_work FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.pin_shared_event();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=14;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=14);
