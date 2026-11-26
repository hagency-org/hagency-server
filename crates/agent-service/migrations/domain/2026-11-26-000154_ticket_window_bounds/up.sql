-- A pooled command, ticket or lease lasts as long as the authorization window
-- of the session it was issued to, which is thirty seconds by default and two
-- minutes at the most. These rules held each of them to thirty seconds flat.
-- The server still issues thirty seconds to a session that does not follow
-- its window, so rows written by an older server satisfy the new rules too.
-- Every statement can run again over its own result.
DO $$
DECLARE c record;
BEGIN
 FOR c IN SELECT conrelid::regclass AS tbl,conname FROM pg_constraint
  WHERE contype='c' AND conrelid IN(
   'hagency_agent_v1.management_shared_runtimes'::regclass,
   'hagency_agent_v1.management_shared_result_tickets'::regclass,
   'hagency_agent_v1.management_agent_budget_tickets'::regclass,
   'hagency_agent_v1.management_agent_settlement_tickets'::regclass)
   AND pg_get_constraintdef(oid) ~ '\m30000\M' AND pg_get_constraintdef(oid) !~ '\m120000\M'
 LOOP
  EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I',c.tbl,c.conname);
 END LOOP;
END;
$$;
ALTER TABLE hagency_agent_v1.management_shared_runtimes DROP CONSTRAINT IF EXISTS management_shared_runtimes_window;
ALTER TABLE hagency_agent_v1.management_shared_runtimes ADD CONSTRAINT management_shared_runtimes_window CHECK(expires_at_ms>created_at_ms AND expires_at_ms<=created_at_ms+120000);
ALTER TABLE hagency_agent_v1.management_shared_result_tickets DROP CONSTRAINT IF EXISTS management_shared_result_tickets_window;
ALTER TABLE hagency_agent_v1.management_shared_result_tickets ADD CONSTRAINT management_shared_result_tickets_window CHECK(expires_at_ms>issued_at_ms AND expires_at_ms<=issued_at_ms+120000);
ALTER TABLE hagency_agent_v1.management_agent_budget_tickets DROP CONSTRAINT IF EXISTS management_agent_budget_tickets_window;
ALTER TABLE hagency_agent_v1.management_agent_budget_tickets ADD CONSTRAINT management_agent_budget_tickets_window CHECK(expires_at_ms>issued_at_ms AND expires_at_ms-issued_at_ms<=120000);
ALTER TABLE hagency_agent_v1.management_agent_settlement_tickets DROP CONSTRAINT IF EXISTS management_agent_settlement_tickets_window;
ALTER TABLE hagency_agent_v1.management_agent_settlement_tickets ADD CONSTRAINT management_agent_settlement_tickets_window CHECK(expires_at_ms>issued_at_ms AND expires_at_ms-issued_at_ms<=120000);
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_managed_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_agent_runtime_commands' THEN
  IF NEW.operation='agent.runtime.provision' THEN
   IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id
    WHERE a.id=NEW.assignment_id AND a.state='pending_native' AND m.state='matrix_verified' AND i.state<>'fenced'
    AND NEW.payload::jsonb->>'commandId'=NEW.id AND NEW.payload::jsonb->>'assignmentId'=a.id AND NEW.payload::jsonb->>'operation'=NEW.operation
    AND NEW.payload::jsonb->>'agentReservationId'=a.agent_reservation_id AND NEW.payload::jsonb->>'reserveReceiptDigest'=a.reserve_receipt_digest
    AND NEW.payload::jsonb->>'reserveCommandDigest'=i.digest AND NEW.payload::jsonb->'reserveCommand'=i.payload::jsonb
    AND (NEW.payload::jsonb->>'assignmentGeneration')::bigint=a.generation AND (NEW.payload::jsonb->>'issuedAtMs')::bigint=NEW.created_at_ms
    AND (NEW.payload::jsonb->>'expiresAtMs')::bigint=NEW.expires_at_ms AND NEW.expires_at_ms>NEW.created_at_ms AND NEW.expires_at_ms<=NEW.created_at_ms+120000)
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
