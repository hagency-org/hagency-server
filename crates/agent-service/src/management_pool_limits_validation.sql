-- Schema 33: provider-specific immutable roots share the same lifetime tables. V5 and V6 calendar grants retain the
-- same native no-issuance proof requirements as V4; a missing receipt is no proof. V6 (the pool owns the renewal
-- policy) was left out of the list at first, so a first command that never started under a V6 grant could not be
-- retired and its Agent stayed in pending_native for good.
-- This function is replaced on every start, so changing it needs no schema
-- version: v2 successors accept the same not_started proof as v3.
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_runtimes' THEN
  IF NEW.issuance_void THEN RAISE EXCEPTION 'shared_original_not_unstarted'; END IF;
 END IF;
 IF TG_TABLE_NAME='management_shared_process_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'commandId' IS DISTINCT FROM NEW.runtime_id
   OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision THEN RAISE EXCEPTION 'shared_process_receipt_mismatch'; END IF;
  -- Only an explicit native no-issuance proof may retire a never-started
  -- recurring original command. No absence of evidence grants a retry.
  IF NEW.payload::jsonb->>'version'='1' AND NEW.payload::jsonb->>'state'='not_started' THEN
   IF NEW.payload::jsonb->>'processDigest' IS NOT NULL OR NEW.payload::jsonb->>'policyDigest' IS NOT NULL
    OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r WHERE r.id=NEW.runtime_id AND r.payload::jsonb->>'version' IN ('1','4') AND r.payload::jsonb->'reserveCommand'->'poolGrant'->>'version' IN ('4','5','6') AND r.payload::jsonb->'reserveCommand'->'poolGrant'->'calendar' IS NOT NULL AND r.payload::jsonb->'reserveCommand'->'poolGrant'->'calendar'<>'null'::jsonb AND r.permanent_fence AND r.state IN('pending','issued') AND r.activity_epoch=0 AND r.process_digest IS NULL)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w WHERE w.runtime_id=NEW.runtime_id)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts p WHERE p.runtime_id=NEW.runtime_id)
   THEN RAISE EXCEPTION 'shared_original_not_unstarted'; END IF;
   RETURN NEW;
  END IF;
  -- Successor non-issuance (v2 or v3, matching its command). A receipt-less
  -- "stopped" attempt can only have been stopped by the uncertainty time bound.
  IF NEW.payload::jsonb->>'state'='not_started' AND (
    coalesce(NEW.payload::jsonb->>'version','') NOT IN ('2','3')
    OR NEW.payload::jsonb->>'processDigest' IS NOT NULL OR NEW.payload::jsonb->>'policyDigest' IS NOT NULL
    OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r WHERE r.id=NEW.runtime_id AND r.payload::jsonb->>'version'=NEW.payload::jsonb->>'version' AND r.permanent_fence AND r.state IN('pending','issued','stopped') AND r.activity_epoch=0 AND r.process_digest IS NULL)
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
       AND original.assignment_id=a.id AND original.payload::jsonb->>'version' IN ('1','4')
       AND NEW.payload::jsonb->'originalRuntimeCommand'=original.payload::jsonb
       AND NEW.payload::jsonb->>'originalRuntimeDigest'=original.digest
       AND NEW.payload::jsonb->>'agentReservationId'=a.agent_reservation_id
       AND (NEW.payload::jsonb->>'assignmentGeneration')::bigint=a.generation
       AND NEW.payload::jsonb->>'startIntentId'=x.id
       AND (NEW.payload::jsonb->>'processAttempt')::bigint=x.process_attempt
       AND NEW.payload::jsonb->'predecessor'->>'commandId'=prior.id
       AND NEW.payload::jsonb->'predecessor'->>'commandDigest'=prior.digest
       AND (NEW.payload::jsonb->'predecessor'->>'processDigest'=prior.process_digest OR (NEW.payload::jsonb->'predecessor'->>'processDigest'='' AND prior.process_digest IS NULL))
       AND (NEW.payload::jsonb->'predecessor'->>'processAttempt')::bigint=x.process_attempt-1
       AND coalesce((prior.payload::jsonb->>'processAttempt')::bigint,1)=x.process_attempt-1
       AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts o
         WHERE o.runtime_id=prior.id AND o.digest=NEW.payload::jsonb->'predecessor'->>'stoppedReceiptDigest'
          AND ((o.payload::jsonb->>'state'='stopped' AND o.payload::jsonb->>'processDigest'=prior.process_digest) OR (o.payload::jsonb->>'version' IN ('2','3') AND o.payload::jsonb->>'state'='not_started' AND o.payload::jsonb->>'processDigest' IS NULL AND prior.process_digest IS NULL AND NEW.payload::jsonb->'predecessor'->>'processDigest'=''))))
  THEN RAISE EXCEPTION 'fresh_shared_attempt_required';END IF;
 ELSE
  IF NOT COALESCE(
    (NEW.payload::jsonb->>'version'='1' AND NEW.payload::jsonb->>'capability'='hagency.managed_agents.execution.v1' AND NEW.payload::jsonb->>'policy'='codex_text_v1')
    OR (NEW.payload::jsonb->>'version'='4' AND NEW.payload::jsonb->>'capability'='hagency.managed_agents.execution.v4' AND NEW.payload::jsonb->>'policy'='claude_text_v1'), false)
   OR NEW.payload::jsonb->>'operation' IS DISTINCT FROM 'agent.execution.provision'
   OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=NEW.assignment_id)
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id
    JOIN hagency_agent_v1.management_agent_requests ar ON ar.id=a.request_id
    LEFT JOIN hagency_agent_v1.management_projects project ON project.id=ar.plan_id
    JOIN hagency_agent_v1.management_contributions contribution ON contribution.id=COALESCE(ar.pool_contribution_id,project.contribution_id)
    WHERE contribution.runtime=CASE NEW.payload::jsonb->>'version' WHEN '4' THEN 'claude' ELSE 'codex' END
    AND a.id=NEW.assignment_id AND a.state='pending_native' AND i.state<>'fenced' AND m.state='matrix_verified'
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

-- Validate existing V5 rows before accepting the same bound for future writes.
-- Older receipts retain their original, unsplit allocation limit.
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid='hagency_agent_v1.management_resource_grants'::regclass AND conname='management_pool_split_limits') THEN
  ALTER TABLE hagency_agent_v1.management_resource_grants ADD CONSTRAINT management_pool_split_limits CHECK (
   receipt::jsonb->>'version' IS DISTINCT FROM '5' OR COALESCE(
    jsonb_typeof(receipt::jsonb->'perRequestTokens')='number'
    AND jsonb_typeof(receipt::jsonb->'allocatedTokens')='number'
    AND (receipt::jsonb->>'perRequestTokens')::numeric BETWEEN 1 AND (receipt::jsonb->>'allocatedTokens')::numeric
    AND (receipt::jsonb->>'allocatedTokens')::numeric BETWEEN 1 AND 9007199254740991
    AND trunc((receipt::jsonb->>'perRequestTokens')::numeric)=(receipt::jsonb->>'perRequestTokens')::numeric
    AND trunc((receipt::jsonb->>'allocatedTokens')::numeric)=(receipt::jsonb->>'allocatedTokens')::numeric, false)
  );
 END IF;
END $$;
