-- A terminal non-issuance receipt is distinct from an actual process stop.
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_process_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'commandId' IS DISTINCT FROM NEW.runtime_id
   OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision THEN RAISE EXCEPTION 'shared_process_receipt_mismatch'; END IF;
  IF NEW.payload::jsonb->>'state'='not_started' AND (
    NEW.payload::jsonb->>'version' IS DISTINCT FROM '3'
    OR NEW.payload::jsonb->>'processDigest' IS NOT NULL OR NEW.payload::jsonb->>'policyDigest' IS NOT NULL
    OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r WHERE r.id=NEW.runtime_id AND r.payload::jsonb->>'version'='3' AND r.permanent_fence AND r.state IN('pending','issued') AND r.activity_epoch=0 AND r.process_digest IS NULL)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w WHERE w.runtime_id=NEW.runtime_id)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts p WHERE p.runtime_id=NEW.runtime_id)
  ) THEN RAISE EXCEPTION 'shared_attempt_not_unstarted'; END IF;
  IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts p WHERE p.runtime_id=NEW.runtime_id AND p.payload::jsonb->>'state'='not_started') THEN RAISE EXCEPTION 'shared_attempt_permanently_unstarted';END IF;
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
       AND (NEW.payload::jsonb->'predecessor'->>'processDigest'=prior.process_digest OR (NEW.payload::jsonb->>'version'='3' AND NEW.payload::jsonb->'predecessor'->>'processDigest'='' AND prior.process_digest IS NULL))
       AND (NEW.payload::jsonb->'predecessor'->>'processAttempt')::bigint=x.process_attempt-1
       AND coalesce((prior.payload::jsonb->>'processAttempt')::bigint,1)=x.process_attempt-1
       AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts o
         WHERE o.runtime_id=prior.id AND o.digest=NEW.payload::jsonb->'predecessor'->>'stoppedReceiptDigest'
          AND ((o.payload::jsonb->>'state'='stopped' AND o.payload::jsonb->>'processDigest'=prior.process_digest) OR (NEW.payload::jsonb->>'version'='3' AND o.payload::jsonb->>'version'='3' AND o.payload::jsonb->>'state'='not_started' AND o.payload::jsonb->>'processDigest' IS NULL AND prior.process_digest IS NULL AND NEW.payload::jsonb->'predecessor'->>'processDigest'=''))))
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

ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=27 WHERE singleton AND version=26;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=27);
