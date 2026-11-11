-- V2 retains SHA-256 of exact JSON bytes. V3 proofs use canonical JSON.
-- This helper is confined to the closed original-block DTO: ASCII object keys,
-- exact integers, booleans and ASCII digest/reason strings. It is not a general
-- JavaScript canonicalizer (numeric and non-ASCII object keys are out of scope).
CREATE FUNCTION hagency_agent_v1.canonical_shared_block_json(v jsonb) RETURNS text
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE result text;
BEGIN
 CASE jsonb_typeof(v)
 WHEN 'object' THEN
  SELECT '{'||coalesce(string_agg(to_jsonb(key)::text||':'||hagency_agent_v1.canonical_shared_block_json(value),',' ORDER BY key COLLATE "C"),'')||'}' INTO result FROM jsonb_each(v);
 WHEN 'array' THEN
  SELECT '['||coalesce(string_agg(hagency_agent_v1.canonical_shared_block_json(value),',' ORDER BY ordinal),'')||']' INTO result FROM jsonb_array_elements(v) WITH ORDINALITY AS a(value,ordinal);
 ELSE result=v::text;
 END CASE;
 RETURN result;
END;$$;
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_budget_demand() RETURNS trigger LANGUAGE plpgsql AS $$
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
 IF FOUND AND (encode(sha256(convert_to(prior_block.payload,'UTF8')),'hex') IS DISTINCT FROM prior_block.digest OR prior_block.payload::jsonb->>'reason' IS DISTINCT FROM prior_block.reason) THEN RAISE EXCEPTION 'shared_demand_block_changed';END IF;
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
   OR v->'originalBlock' IS DISTINCT FROM (CASE WHEN prior_block.demand_id IS NULL THEN 'null'::jsonb ELSE jsonb_build_object('reason',prior_block.reason,'payloadDigest',encode(sha256(convert_to(hagency_agent_v1.canonical_shared_block_json(prior_block.payload::jsonb),'UTF8')),'hex')) END)
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
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=26 WHERE singleton AND version=25;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=26);
