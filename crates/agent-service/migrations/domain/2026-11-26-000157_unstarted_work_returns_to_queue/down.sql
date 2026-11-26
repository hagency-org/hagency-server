-- Back to permanent bindings. Messages that are waiting again stay waiting;
-- the record of released bindings is kept, because the bindings it replaced
-- are gone.
DROP TRIGGER IF EXISTS fence_shared_release ON hagency_agent_v1.management_shared_runtimes;
DROP FUNCTION IF EXISTS hagency_agent_v1.release_unstarted_shared_work();
DROP TRIGGER IF EXISTS retain_shared_demand_binding ON hagency_agent_v1.management_shared_demand_bindings;
CREATE TRIGGER retain_shared_demand_binding BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_demand_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
DROP FUNCTION IF EXISTS hagency_agent_v1.retain_shared_demand_binding();
DROP TRIGGER IF EXISTS retain_shared_event_pin ON hagency_agent_v1.management_shared_event_pins;
CREATE TRIGGER retain_shared_event_pin BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_event_pins FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_runtime();
DROP FUNCTION IF EXISTS hagency_agent_v1.retain_shared_event_pin();
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
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_demand_retry() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE x hagency_agent_v1.management_shared_demands; q hagency_agent_v1.management_shared_intake; d jsonb; previous text;
BEGIN
 SELECT * INTO STRICT x FROM hagency_agent_v1.management_shared_demands WHERE id=NEW.demand_id;
 SELECT * INTO STRICT q FROM hagency_agent_v1.management_shared_intake WHERE id=x.intake_id;
 IF q.state<>'pending'
  OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_controls c WHERE c.assignment_id=q.assignment_id AND c.mode='running' AND c.revision=q.control_revision)
  OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_bindings WHERE demand_id=x.id)
  OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_blocks WHERE demand_id=x.id) THEN
  RAISE EXCEPTION 'shared_demand_retry_changed';
 END IF;
 IF TG_TABLE_NAME='management_shared_demand_retries' THEN
  -- Only the demand's latest started attempt, proven never started, and
  -- still the head of its chain, can be retried; numbering has no gaps.
  previous=CASE WHEN NEW.retry=1
   THEN (SELECT runtime_id FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=x.id)
   ELSE (SELECT runtime_id FROM hagency_agent_v1.management_shared_demand_retry_starts WHERE demand_id=x.id AND retry=NEW.retry-1) END;
  IF previous IS DISTINCT FROM NEW.previous_runtime_id
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts o JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=o.runtime_id
     WHERE o.id=NEW.receipt_id AND o.runtime_id=NEW.previous_runtime_id AND o.payload::jsonb->>'state'='not_started'
       AND r.state='stopped' AND r.permanent_fence AND r.process_digest IS NULL AND r.activity_epoch=0)
   OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_runtimes newer ON newer.assignment_id=r.assignment_id
     WHERE r.id=NEW.previous_runtime_id AND coalesce((newer.payload::jsonb->>'processAttempt')::bigint,1)>coalesce((r.payload::jsonb->>'processAttempt')::bigint,1))
   OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_retries WHERE demand_id=NEW.demand_id AND retry>=NEW.retry) THEN
   RAISE EXCEPTION 'shared_demand_retry_changed';
  END IF;
 ELSE
  d=NEW.payload::jsonb;
  IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_retries WHERE demand_id=NEW.demand_id AND retry=NEW.retry AND previous_runtime_id=d->'start'->'predecessor'->>'commandId')
   OR (d->>'retry')::bigint IS DISTINCT FROM NEW.retry OR d->>'demandDigest' IS DISTINCT FROM x.digest
   OR (d->>'expectedControlRevision')::bigint IS DISTINCT FROM q.control_revision
   OR d->'start'->>'assignmentId' IS DISTINCT FROM q.assignment_id OR d->'start'->>'startIntentId' IS DISTINCT FROM NEW.start_intent_id
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts a WHERE a.id=NEW.start_intent_id AND a.runtime_id=NEW.runtime_id
     AND a.assignment_id=q.assignment_id AND a.input_payload::jsonb=d->'start' AND a.predecessor_id=d->'start'->'predecessor'->>'commandId') THEN
   RAISE EXCEPTION 'shared_demand_retry_start_changed';
  END IF;
 END IF;
 RETURN NEW;
END;
$$;
