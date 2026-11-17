-- Test-only removal of the newer immutable settlement journal before rewind.
DROP VIEW IF EXISTS hagency_agent_v1.management_agent_settlement_evidence;
DROP TABLE IF EXISTS hagency_agent_v1.management_agent_settlement_tickets,hagency_agent_v1.management_agent_settlement_issuance,hagency_agent_v1.management_agent_settlement_results,hagency_agent_v1.management_agent_settlement_commands;
DROP FUNCTION IF EXISTS hagency_agent_v1.validate_agent_settlement();
-- Test-only rewind from20 to19; original function definitions restored verbatim.
DROP TABLE hagency_agent_v1.management_shared_budget_demand_starts,hagency_agent_v1.management_shared_budget_demand_bindings,hagency_agent_v1.management_shared_budget_demand_blocks,hagency_agent_v1.management_shared_budget_demand_terminals,hagency_agent_v1.management_shared_budget_demand_heads,hagency_agent_v1.management_shared_work_refusals,hagency_agent_v1.management_shared_budget_attempts;
DROP TRIGGER fence_shared_unissued_work ON hagency_agent_v1.management_shared_work;
DROP FUNCTION hagency_agent_v1.validate_shared_budget_demand(),hagency_agent_v1.validate_shared_budget_pin(),hagency_agent_v1.validate_shared_work_refusal(),hagency_agent_v1.fence_shared_unissued_work();
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
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_work() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_work' THEN
  IF NEW.payload::jsonb->>'id' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'eventId' IS DISTINCT FROM NEW.event_id OR (NEW.payload::jsonb->>'createdAtMs')::bigint IS DISTINCT FROM NEW.created_at_ms
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id WHERE r.id=NEW.runtime_id AND r.state='qualified' AND NOT r.permanent_fence AND a.state='pending_native' AND m.state='matrix_verified' AND NEW.payload::jsonb->>'assignmentId'=a.id AND NEW.payload::jsonb->>'runtimeCommandDigest'=r.digest AND NEW.payload::jsonb->>'roomId'=m.room_id AND NEW.payload::jsonb->>'requesterMxid'=u.mxid)
  THEN RAISE EXCEPTION 'shared_work_binding_changed';END IF;
 ELSIF TG_TABLE_NAME='management_shared_work_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'workId' IS DISTINCT FROM NEW.work_id OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE w.id=NEW.work_id AND w.started_at_ms IS NOT NULL AND NEW.payload::jsonb->>'workDigest'=w.digest AND NEW.payload::jsonb->>'executionId'=w.payload::jsonb->>'executionId' AND NEW.payload::jsonb->>'runtimeCommandDigest'=r.digest AND NEW.payload::jsonb->>'processDigest'=r.process_digest)
  THEN RAISE EXCEPTION 'shared_work_receipt_binding_changed';END IF;
 ELSE
  IF NEW.matrix_txn_id IS DISTINCT FROM 'hagency_shared_'||NEW.work_id OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_work_receipts o ON o.id=w.latest_receipt WHERE w.id=NEW.work_id AND w.state='completed' AND NEW.content::jsonb->>'msgtype'='m.text' AND NEW.content::jsonb->>'body'=o.payload::jsonb->>'body' AND NEW.content::jsonb->'m.relates_to'->>'event_id'=w.payload::jsonb->>'threadRoot') THEN RAISE EXCEPTION 'shared_reply_binding_changed';END IF;
 END IF;RETURN NEW;
END;
$$;
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_shared_intake() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME='management_shared_control_commands' THEN RAISE EXCEPTION 'retained_shared_intake_history';END IF;
 IF TG_TABLE_NAME='management_shared_controls' THEN
  IF NEW.assignment_id<>OLD.assignment_id OR NEW.revision<>OLD.revision+1 OR NEW.updated_at_ms<OLD.updated_at_ms OR NEW.mode='manual' THEN RAISE EXCEPTION 'shared_control_revision_required';END IF;
 ELSE
  IF (to_jsonb(NEW)-'state'-'reason') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'reason') OR OLD.state<>'pending' OR NEW.state NOT IN('cancelled','expired','blocked','bound') THEN RAISE EXCEPTION 'immutable_shared_intake';END IF;
  IF NEW.state='bound' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_demand_bindings b ON b.demand_id=d.id WHERE d.intake_id=NEW.id) THEN RAISE EXCEPTION 'shared_binding_required';END IF;
 END IF;RETURN NEW;
END;
$$;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=19;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=19);
