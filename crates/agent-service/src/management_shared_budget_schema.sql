-- V3 budget pins and definitive no-start evidence. Original commands and amounts are retained.
CREATE TABLE hagency_agent_v1.management_shared_budget_attempts (
 runtime_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_runtimes(id) DEFERRABLE INITIALLY DEFERRED,
 assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_assignments(id),
 budget_revision bigint NOT NULL CHECK(budget_revision BETWEEN 2 AND 9007199254740991),
 budget_digest text NOT NULL CHECK(length(budget_digest)=64),
 applied_command_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_budget_results(command_id),
 pin_payload text NOT NULL CHECK(octet_length(pin_payload)<=4096),pin_digest text NOT NULL CHECK(length(pin_digest)=64)
);
CREATE TRIGGER retain_shared_budget_attempt BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_budget_attempts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_runtime();
CREATE FUNCTION hagency_agent_v1.validate_shared_budget_pin() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE p jsonb := NEW.pin_payload::jsonb;
BEGIN
 IF p->>'version' IS DISTINCT FROM '1' OR p->>'budgetDigest' IS DISTINCT FROM NEW.budget_digest
 OR (p->>'budgetRevision')::bigint IS DISTINCT FROM NEW.budget_revision OR p->>'appliedCommandId' IS DISTINCT FROM NEW.applied_command_id
 OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_results r
    JOIN hagency_agent_v1.management_agent_budget_commands c ON c.id=r.command_id
    JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=c.request_id
    WHERE r.command_id=NEW.applied_command_id AND a.id=NEW.assignment_id AND r.state='applied'
    AND r.budget_revision=NEW.budget_revision AND r.payload::jsonb->>'budgetDigest'=NEW.budget_digest
    AND c.digest=p->>'appliedCommandDigest' AND r.digest=p->>'appliedReceiptDigest'
    AND a.reserve_receipt_digest=p->>'originalReserveReceiptDigest'
    AND r.payload::jsonb->>'effectiveCeilingTokens'=p->>'effectiveCeilingTokens'
    AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_results nr
       JOIN hagency_agent_v1.management_agent_budget_commands nc ON nc.id=nr.command_id
       WHERE nc.request_id=c.request_id AND nr.state='applied' AND nr.budget_revision>r.budget_revision))
 THEN RAISE EXCEPTION 'shared_applied_budget_required';END IF;
 RETURN NEW;
END;$$;
CREATE TRIGGER validate_shared_budget_pin BEFORE INSERT ON hagency_agent_v1.management_shared_budget_attempts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_budget_pin();
CREATE TABLE hagency_agent_v1.management_shared_work_refusals (
 work_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_work(id),
 payload text NOT NULL CHECK(octet_length(payload)<=8192),digest text NOT NULL CHECK(length(digest)=64)
);
CREATE TRIGGER retain_shared_work_refusal BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_work_refusals FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_runtime();
CREATE FUNCTION hagency_agent_v1.fence_shared_unissued_work() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.started_at_ms IS NOT NULL AND OLD.started_at_ms IS NULL
 AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work_refusals WHERE work_id=NEW.id)
 THEN RAISE EXCEPTION 'shared_work_definitively_not_started';END IF;
 RETURN NEW;
END;$$;
CREATE TRIGGER fence_shared_unissued_work BEFORE UPDATE ON hagency_agent_v1.management_shared_work FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_shared_unissued_work();
CREATE FUNCTION hagency_agent_v1.validate_shared_work_refusal() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE f jsonb := NEW.payload::jsonb;
BEGIN
 IF f->>'version' IS DISTINCT FROM '1' OR f->>'state' IS DISTINCT FROM 'not_started'
 OR f->>'reason' IS DISTINCT FROM 'budget_head_changed' OR f->>'refusalId' IS DISTINCT FROM 'sbr_'||NEW.work_id
 OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w
   JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id
   JOIN hagency_agent_v1.management_shared_budget_attempts b ON b.runtime_id=r.id
   WHERE w.id=NEW.work_id AND w.started_at_ms IS NULL AND w.latest_receipt IS NULL
   AND w.state IN('queued','acknowledged','cancelled')
   AND f->>'workId'=w.id AND f->>'workDigest'=w.digest
   AND f->>'runtimeCommandId'=r.id AND f->>'runtimeCommandDigest'=r.digest
   AND f->'executionBudget'=b.pin_payload::jsonb
   AND f->>'currentBudgetDigest'<>b.budget_digest
   AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_results br
      JOIN hagency_agent_v1.management_agent_budget_commands bc ON bc.id=br.command_id
      JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=bc.request_id
      WHERE a.id=r.assignment_id AND br.state='applied' AND br.budget_revision>b.budget_revision
      AND br.payload::jsonb->>'budgetDigest'=f->>'currentBudgetDigest'))
 THEN RAISE EXCEPTION 'shared_work_no_start_proof_required';END IF;
 RETURN NEW;
END;$$;
CREATE TRIGGER validate_shared_work_refusal BEFORE INSERT ON hagency_agent_v1.management_shared_work_refusals FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_work_refusal();
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_process_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'commandId' IS DISTINCT FROM NEW.runtime_id
   OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision THEN RAISE EXCEPTION 'shared_process_receipt_mismatch'; END IF;
 ELSIF NEW.payload::jsonb->>'version' IN('2','3') THEN
  IF NEW.payload::jsonb->>'capability' IS DISTINCT FROM (CASE NEW.payload::jsonb->>'version' WHEN '3' THEN 'hagency.managed_agents.execution.v3' ELSE 'hagency.managed_agents.execution.v2' END)
   OR NEW.payload::jsonb->>'operation' IS DISTINCT FROM 'agent.execution.provision'
   OR NEW.payload::jsonb->>'commandId' IS DISTINCT FROM NEW.id
   OR NEW.payload::jsonb->>'assignmentId' IS DISTINCT FROM NEW.assignment_id
   OR (NEW.payload::jsonb->>'issuedAtMs')::bigint IS DISTINCT FROM NEW.created_at_ms
   OR (NEW.payload::jsonb->>'expiresAtMs')::bigint IS DISTINCT FROM NEW.expires_at_ms
   OR (NEW.payload::jsonb->>'version'='3' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_attempts bp WHERE bp.runtime_id=NEW.id AND bp.assignment_id=NEW.assignment_id AND bp.pin_payload::jsonb=NEW.payload::jsonb->'executionBudget'))
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

CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_work() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_work' THEN
  IF NEW.payload::jsonb->>'id' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'eventId' IS DISTINCT FROM NEW.event_id OR (NEW.payload::jsonb->>'createdAtMs')::bigint IS DISTINCT FROM NEW.created_at_ms
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id WHERE r.id=NEW.runtime_id AND r.state='qualified' AND NOT r.permanent_fence AND a.state='pending_native' AND m.state='matrix_verified' AND NEW.payload::jsonb->>'assignmentId'=a.id AND NEW.payload::jsonb->>'runtimeCommandDigest'=r.digest AND NEW.payload::jsonb->>'roomId'=m.room_id AND NEW.payload::jsonb->>'requesterMxid'=u.mxid)
  THEN RAISE EXCEPTION 'shared_work_binding_changed';END IF;
 ELSIF TG_TABLE_NAME='management_shared_work_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'workId' IS DISTINCT FROM NEW.work_id OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE w.id=NEW.work_id AND (w.started_at_ms IS NOT NULL OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work_refusals f WHERE f.work_id=w.id AND NEW.payload::jsonb->>'state'='failed' AND NEW.payload::jsonb->>'knownConsumedTokens'='0' AND NEW.payload::jsonb->>'retainedTokens'='0' AND NEW.payload::jsonb->>'usageComplete'='true')) AND NEW.payload::jsonb->>'workDigest'=w.digest AND NEW.payload::jsonb->>'executionId'=w.payload::jsonb->>'executionId' AND NEW.payload::jsonb->>'runtimeCommandDigest'=r.digest AND NEW.payload::jsonb->>'processDigest'=r.process_digest)
  THEN RAISE EXCEPTION 'shared_work_receipt_binding_changed';END IF;
 ELSE
  IF NEW.matrix_txn_id IS DISTINCT FROM 'hagency_shared_'||NEW.work_id OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_work_receipts o ON o.id=w.latest_receipt WHERE w.id=NEW.work_id AND w.state='completed' AND NEW.content::jsonb->>'msgtype'='m.text' AND NEW.content::jsonb->>'body'=o.payload::jsonb->>'body' AND NEW.content::jsonb->'m.relates_to'->>'event_id'=w.payload::jsonb->>'threadRoot') THEN RAISE EXCEPTION 'shared_reply_binding_changed';END IF;
 END IF;RETURN NEW;
END;
$$;
CREATE TABLE hagency_agent_v1.management_shared_budget_demand_heads (
 demand_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_demands(id),
 budget_revision bigint NOT NULL CHECK(budget_revision>=2),budget_digest text NOT NULL,
 payload text NOT NULL,digest text NOT NULL,PRIMARY KEY(demand_id,budget_revision)
);
CREATE TABLE hagency_agent_v1.management_shared_budget_demand_starts (
 demand_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_demands(id),
 start_intent_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_attempts(id),
 runtime_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 payload text NOT NULL,digest text NOT NULL,budget_revision bigint NOT NULL,
 FOREIGN KEY(demand_id,budget_revision) REFERENCES hagency_agent_v1.management_shared_budget_demand_heads(demand_id,budget_revision)
);
CREATE TABLE hagency_agent_v1.management_shared_budget_demand_bindings (
 demand_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_demands(id),
 work_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_work(id),
 runtime_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 payload text NOT NULL,digest text NOT NULL,budget_revision bigint NOT NULL,
 FOREIGN KEY(demand_id,budget_revision) REFERENCES hagency_agent_v1.management_shared_budget_demand_heads(demand_id,budget_revision)
);
CREATE TABLE hagency_agent_v1.management_shared_budget_demand_blocks (
 demand_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_demands(id),
 budget_revision bigint NOT NULL,reason text NOT NULL CHECK(reason IN('budget_blocked','usage_unrepresentable')),
 payload text NOT NULL,digest text NOT NULL,PRIMARY KEY(demand_id,budget_revision),
 FOREIGN KEY(demand_id,budget_revision) REFERENCES hagency_agent_v1.management_shared_budget_demand_heads(demand_id,budget_revision)
);
CREATE TABLE hagency_agent_v1.management_shared_budget_demand_terminals (
 demand_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_demands(id),
 reason text NOT NULL CHECK(reason IN('expired','cancelled','head_changed')),payload text NOT NULL,digest text NOT NULL
);
CREATE TRIGGER retain_shared_budget_demand_head BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_budget_demand_heads FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
CREATE TRIGGER retain_shared_budget_demand_start BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_budget_demand_starts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
CREATE TRIGGER retain_shared_budget_demand_binding BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_budget_demand_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
CREATE TRIGGER retain_shared_budget_demand_block BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_budget_demand_blocks FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
CREATE TRIGGER retain_shared_budget_demand_terminal BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_budget_demand_terminals FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
CREATE FUNCTION hagency_agent_v1.validate_shared_budget_demand() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE x hagency_agent_v1.management_shared_demands; q hagency_agent_v1.management_shared_intake;
 v jsonb := NEW.payload::jsonb; o jsonb; p jsonb; prior_block record;
BEGIN
 SELECT * INTO STRICT x FROM hagency_agent_v1.management_shared_demands WHERE id=NEW.demand_id;
 SELECT * INTO STRICT q FROM hagency_agent_v1.management_shared_intake WHERE id=x.intake_id;
 o=x.payload::jsonb;
 IF TG_TABLE_NAME='management_shared_budget_demand_terminals' THEN RETURN NEW; END IF;
 IF q.state NOT IN('pending','blocked') OR q.expires_at_ms<=(extract(epoch from clock_timestamp())*1000)::bigint
 OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_controls c WHERE c.assignment_id=q.assignment_id AND c.mode='running' AND c.revision=q.control_revision)
 OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=x.id)
 OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_bindings WHERE demand_id=x.id)
 OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_terminals WHERE demand_id=x.id)
 THEN RAISE EXCEPTION 'shared_budget_demand_not_current'; END IF;
 SELECT * INTO prior_block FROM hagency_agent_v1.management_shared_demand_blocks WHERE demand_id=x.id;
 IF FOUND AND prior_block.reason<>'budget_blocked' THEN RAISE EXCEPTION 'shared_budget_demand_blocked';END IF;
 IF TG_TABLE_NAME='management_shared_budget_demand_heads' THEN
  IF v->>'version' IS DISTINCT FROM '1' OR (v->>'budgetRevision')::bigint IS DISTINCT FROM NEW.budget_revision OR v->>'budgetDigest' IS DISTINCT FROM NEW.budget_digest
  OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_starts WHERE demand_id=x.id)
  OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_bindings WHERE demand_id=x.id)
  OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_results r JOIN hagency_agent_v1.management_agent_budget_commands c ON c.id=r.command_id JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=c.request_id WHERE r.state='applied' AND a.id=q.assignment_id AND c.id=v->>'appliedCommandId' AND c.digest=v->>'appliedCommandDigest' AND r.digest=v->>'appliedReceiptDigest' AND r.budget_revision=NEW.budget_revision AND r.payload::jsonb->>'budgetDigest'=NEW.budget_digest AND r.payload::jsonb->>'effectiveCeilingTokens'=v->>'effectiveCeilingTokens' AND a.reserve_receipt_digest=v->>'originalReserveReceiptDigest')
  THEN RAISE EXCEPTION 'shared_budget_demand_head_changed';END IF;
 ELSE
  SELECT payload::jsonb INTO STRICT p FROM hagency_agent_v1.management_shared_budget_demand_heads WHERE demand_id=x.id AND budget_revision=NEW.budget_revision;
  IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_bindings WHERE demand_id=x.id) OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_demand_blocks WHERE demand_id=x.id AND budget_revision=NEW.budget_revision) THEN RAISE EXCEPTION 'shared_budget_demand_terminal';END IF;
  IF TG_TABLE_NAME='management_shared_budget_demand_starts' THEN
   IF v->>'demandDigest' IS DISTINCT FROM x.digest OR (v->>'expectedControlRevision')::bigint IS DISTINCT FROM q.control_revision
   OR v->'start'->'executionBudget' IS DISTINCT FROM p
   OR v->'originalBlock' IS DISTINCT FROM (CASE WHEN prior_block.demand_id IS NULL THEN 'null'::jsonb ELSE jsonb_build_object('reason',prior_block.reason,'payloadDigest',prior_block.digest) END)
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts a JOIN hagency_agent_v1.management_shared_budget_attempts b ON b.runtime_id=a.runtime_id WHERE a.id=NEW.start_intent_id AND a.runtime_id=NEW.runtime_id AND a.assignment_id=q.assignment_id AND a.input_payload::jsonb=v->'start' AND a.predecessor_id=o->>'predecessorCommandId' AND b.pin_payload::jsonb=p)
   THEN RAISE EXCEPTION 'shared_budget_demand_start_changed';END IF;
  ELSIF TG_TABLE_NAME='management_shared_budget_demand_bindings' THEN
   IF v->>'version' IS DISTINCT FROM '1' OR v->>'demandId' IS DISTINCT FROM x.id OR v->>'demandDigest' IS DISTINCT FROM x.digest OR (v->>'controlRevision')::bigint IS DISTINCT FROM q.control_revision OR v->>'assignmentId' IS DISTINCT FROM q.assignment_id OR v->>'eventId' IS DISTINCT FROM q.event_id OR (v->>'receivedAtMs')::bigint IS DISTINCT FROM q.received_at_ms OR v->>'runtimeCommandId' IS DISTINCT FROM NEW.runtime_id OR v->>'workId' IS DISTINCT FROM NEW.work_id OR NEW.runtime_id IS DISTINCT FROM coalesce((SELECT runtime_id FROM hagency_agent_v1.management_shared_budget_demand_starts WHERE demand_id=x.id),o->>'predecessorCommandId')
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id JOIN hagency_agent_v1.management_shared_budget_attempts b ON b.runtime_id=r.id WHERE w.id=NEW.work_id AND w.runtime_id=NEW.runtime_id AND b.pin_payload::jsonb=p AND r.assignment_id=q.assignment_id AND w.event_id=q.event_id AND w.created_at_ms=q.received_at_ms AND w.digest=v->>'workDigest' AND r.digest=v->>'runtimeCommandDigest' AND w.payload::jsonb->>'body'=q.event_payload::jsonb->>'body' AND w.payload::jsonb->>'roomId'=q.event_payload::jsonb->>'room_id' AND w.payload::jsonb->>'requesterMxid'=q.event_payload::jsonb->>'sender_mxid' AND w.payload::jsonb->>'threadRoot'=coalesce(q.event_payload::jsonb->>'thread_root',q.event_id))
   THEN RAISE EXCEPTION 'shared_budget_demand_work_changed';END IF;
  END IF;
 END IF;
 RETURN NEW;
END;$$;
CREATE TRIGGER validate_shared_budget_demand_head BEFORE INSERT ON hagency_agent_v1.management_shared_budget_demand_heads FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_budget_demand();
CREATE TRIGGER validate_shared_budget_demand_start BEFORE INSERT ON hagency_agent_v1.management_shared_budget_demand_starts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_budget_demand();
CREATE TRIGGER validate_shared_budget_demand_binding BEFORE INSERT ON hagency_agent_v1.management_shared_budget_demand_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_budget_demand();
CREATE TRIGGER validate_shared_budget_demand_block BEFORE INSERT ON hagency_agent_v1.management_shared_budget_demand_blocks FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_budget_demand();
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_shared_intake() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME='management_shared_control_commands' THEN RAISE EXCEPTION 'retained_shared_intake_history';END IF;
 IF TG_TABLE_NAME='management_shared_controls' THEN
  IF NEW.assignment_id<>OLD.assignment_id OR NEW.revision<>OLD.revision+1 OR NEW.updated_at_ms<OLD.updated_at_ms OR NEW.mode='manual' THEN RAISE EXCEPTION 'shared_control_revision_required';END IF;
 ELSE
  IF (to_jsonb(NEW)-'state'-'reason') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'reason') OR OLD.state<>'pending' OR NEW.state NOT IN('cancelled','expired','blocked','bound') THEN RAISE EXCEPTION 'immutable_shared_intake';END IF;
  IF NEW.state='bound' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_budget_demand_bindings b ON b.demand_id=d.id WHERE d.intake_id=NEW.id) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_demand_bindings b ON b.demand_id=d.id WHERE d.intake_id=NEW.id) THEN RAISE EXCEPTION 'shared_binding_required';END IF;
 END IF;RETURN NEW;
END;
$$;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=20;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=20);
