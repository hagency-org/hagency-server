-- Shared text process custody is separate from immutable metadata runtime v1.
CREATE TABLE hagency_agent_v1.management_shared_runtimes (
 id text PRIMARY KEY,
 assignment_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_assignments(id),
 payload text NOT NULL,
 digest text NOT NULL,
 created_at_ms bigint NOT NULL,
 expires_at_ms bigint NOT NULL CHECK(expires_at_ms>created_at_ms AND expires_at_ms<=created_at_ms+30000),
 state text NOT NULL DEFAULT 'pending' CHECK(state IN('pending','issued','qualified','unknown','stopped')),
 permanent_fence boolean NOT NULL DEFAULT false,
 process_digest text,
 lease_hash text,
 lease_mode text NOT NULL DEFAULT 'observe' CHECK(lease_mode IN('execute','observe','cleanup')),
 lease_until_ms bigint NOT NULL DEFAULT 0,
 activity_epoch bigint NOT NULL DEFAULT 0 CHECK(activity_epoch BETWEEN 0 AND 9007199254740991),
 activity_hash text,
 activity_until_ms bigint NOT NULL DEFAULT 0,
 last_renewal_id text,
 last_renewal_epoch bigint
);
CREATE TABLE hagency_agent_v1.management_shared_process_receipts (
 id text PRIMARY KEY,
 runtime_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 payload text NOT NULL,
 digest text NOT NULL,
 ledger_revision bigint NOT NULL CHECK(ledger_revision BETWEEN 1 AND 9007199254740991),
 UNIQUE(runtime_id,ledger_revision)
);
CREATE TABLE hagency_agent_v1.management_shared_activity_renewals (
 id text PRIMARY KEY,
 runtime_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 from_epoch bigint NOT NULL CHECK(from_epoch BETWEEN 1 AND 9007199254740990),
 to_epoch bigint NOT NULL CHECK(to_epoch=from_epoch+1),
 expires_at_ms bigint NOT NULL,
 authority_hash text NOT NULL,
 UNIQUE(runtime_id,from_epoch)
);
CREATE FUNCTION hagency_agent_v1.retain_shared_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME<>'management_shared_runtimes' THEN RAISE EXCEPTION 'permanent_shared_runtime_history'; END IF;
 IF (to_jsonb(NEW)-'state'-'permanent_fence'-'process_digest'-'lease_hash'-'lease_mode'-'lease_until_ms'-'activity_epoch'-'activity_hash'-'activity_until_ms'-'last_renewal_id'-'last_renewal_epoch') IS DISTINCT FROM
    (to_jsonb(OLD)-'state'-'permanent_fence'-'process_digest'-'lease_hash'-'lease_mode'-'lease_until_ms'-'activity_epoch'-'activity_hash'-'activity_until_ms'-'last_renewal_id'-'last_renewal_epoch')
 OR (OLD.permanent_fence AND NOT NEW.permanent_fence)
 OR (OLD.process_digest IS NOT NULL AND NEW.process_digest IS DISTINCT FROM OLD.process_digest)
 OR NEW.activity_epoch<OLD.activity_epoch
 OR (OLD.state='stopped' AND NEW.state<>'stopped')
 OR (OLD.state<>'pending' AND NEW.state='pending')
 OR (NEW.permanent_fence AND NEW.activity_until_ms<>0)
 THEN RAISE EXCEPTION 'immutable_shared_runtime_scope'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER retain_shared_runtime BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_runtimes FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_runtime();
CREATE TRIGGER retain_shared_process_receipt BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_process_receipts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_runtime();
CREATE TRIGGER retain_shared_activity_renewal BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_activity_renewals FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_runtime();
CREATE FUNCTION hagency_agent_v1.validate_shared_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_process_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'commandId' IS DISTINCT FROM NEW.runtime_id
   OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision THEN RAISE EXCEPTION 'shared_process_receipt_mismatch'; END IF;
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
CREATE TRIGGER validate_shared_runtime BEFORE INSERT ON hagency_agent_v1.management_shared_runtimes FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_runtime();
CREATE TRIGGER validate_shared_process_receipt BEFORE INSERT ON hagency_agent_v1.management_shared_process_receipts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_runtime();
CREATE FUNCTION hagency_agent_v1.exclude_shared_runtime_v1() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.operation='agent.runtime.provision' AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes WHERE assignment_id=NEW.assignment_id) THEN RAISE EXCEPTION 'shared_assignment_already_issued'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER exclude_shared_runtime_v1 BEFORE INSERT ON hagency_agent_v1.management_agent_runtime_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.exclude_shared_runtime_v1();
CREATE FUNCTION hagency_agent_v1.fence_shared_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.state='fenced' THEN
  UPDATE hagency_agent_v1.management_shared_runtimes SET permanent_fence=true,activity_until_ms=0 WHERE assignment_id=NEW.id;
 END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER fence_shared_runtime AFTER UPDATE OF state ON hagency_agent_v1.management_shared_assignments FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_shared_runtime();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=12;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=12);
