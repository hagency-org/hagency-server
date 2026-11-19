-- ADR-193 amendment: retry an event whose attempt provably never started.
-- The demand, its event and its FIFO place stay; a retry is a new attempt
-- that names the not_started one as predecessor. Additive and idempotent:
-- run at every start, no version change, and an older binary ignores it.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.management_shared_demand_retries (
 demand_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_demands(id),
 retry bigint NOT NULL CHECK(retry BETWEEN 1 AND 3),
 previous_runtime_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 receipt_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_process_receipts(id),
 created_at_ms bigint NOT NULL,
 PRIMARY KEY(demand_id,retry)
);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.management_shared_demand_retry_starts (
 demand_id text NOT NULL,
 retry bigint NOT NULL,
 start_intent_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_attempts(id),
 runtime_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 payload text NOT NULL,digest text NOT NULL,
 PRIMARY KEY(demand_id,retry),
 FOREIGN KEY(demand_id,retry) REFERENCES hagency_agent_v1.management_shared_demand_retries(demand_id,retry)
);
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
DROP TRIGGER IF EXISTS retain_shared_demand_retry ON hagency_agent_v1.management_shared_demand_retries;
CREATE TRIGGER retain_shared_demand_retry BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_demand_retries FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
DROP TRIGGER IF EXISTS retain_shared_demand_retry_start ON hagency_agent_v1.management_shared_demand_retry_starts;
CREATE TRIGGER retain_shared_demand_retry_start BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_demand_retry_starts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
DROP TRIGGER IF EXISTS validate_shared_demand_retry ON hagency_agent_v1.management_shared_demand_retries;
CREATE TRIGGER validate_shared_demand_retry BEFORE INSERT ON hagency_agent_v1.management_shared_demand_retries FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_demand_retry();
DROP TRIGGER IF EXISTS validate_shared_demand_retry_start ON hagency_agent_v1.management_shared_demand_retry_starts;
CREATE TRIGGER validate_shared_demand_retry_start BEFORE INSERT ON hagency_agent_v1.management_shared_demand_retry_starts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_demand_retry();
-- validate_shared_demand() has one owner: this file. It is the schema-17
-- validator (management_shared_demand_schema.sql), unchanged except that a
-- binding may name the demand's latest retry start, and it is re-created here
-- at every start, after the versioned migrations. Replacing it is safe only
-- while the installed body is the schema-17 one (by digest) or this file's
-- own (by its marker); anything else means a later versioned migration
-- changed it, and startup fails instead of silently undoing that change.
-- A future change to this function belongs here, not in a migration.
DO $guard$
DECLARE src text;
BEGIN
 SELECT p.prosrc INTO src FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
  WHERE n.nspname='hagency_agent_v1' AND p.proname='validate_shared_demand';
 IF src IS NOT NULL AND md5(src)<>'f5e681b0cf2eaf20f626f542ed21c833'
  AND position('retry-aware demand validator' in src)=0 THEN
  RAISE EXCEPTION 'validate_shared_demand_changed_by_migration';
 END IF;
END
$guard$;
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_demand() RETURNS trigger LANGUAGE plpgsql AS $$
-- retry-aware demand validator (owned by management_shared_demand_retry_schema.sql)
DECLARE d jsonb; o jsonb; q hagency_agent_v1.management_shared_intake; x hagency_agent_v1.management_shared_demands;
BEGIN
 d=NEW.payload::jsonb;
 IF TG_TABLE_NAME='management_shared_demands' THEN
  SELECT * INTO STRICT q FROM hagency_agent_v1.management_shared_intake WHERE id=NEW.intake_id;
  IF d->>'version' IS DISTINCT FROM '1' OR NEW.id IS DISTINCT FROM 'sdm_'||substr(q.id,5) OR d->>'demandId' IS DISTINCT FROM NEW.id OR d->>'assignmentId' IS DISTINCT FROM q.assignment_id OR d->>'eventId' IS DISTINCT FROM q.event_id OR d->>'eventDigest' IS DISTINCT FROM q.event_digest OR (d->>'controlRevision')::bigint IS DISTINCT FROM q.control_revision OR (d->>'receivedAtMs')::bigint IS DISTINCT FROM q.received_at_ms OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r WHERE r.id=d->>'predecessorCommandId' AND r.digest=d->>'predecessorCommandDigest' AND r.assignment_id=q.assignment_id AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes newer WHERE newer.assignment_id=r.assignment_id AND coalesce((newer.payload::jsonb->>'processAttempt')::bigint,1)>coalesce((r.payload::jsonb->>'processAttempt')::bigint,1))) THEN RAISE EXCEPTION 'shared_demand_binding_changed';END IF;
 ELSE
  SELECT * INTO STRICT x FROM hagency_agent_v1.management_shared_demands WHERE id=NEW.demand_id;
  SELECT * INTO STRICT q FROM hagency_agent_v1.management_shared_intake WHERE id=x.intake_id;
  o=x.payload::jsonb;
  IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_bindings WHERE demand_id=x.id) OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_blocks WHERE demand_id=x.id) THEN RAISE EXCEPTION 'shared_demand_terminal';END IF;
  IF TG_TABLE_NAME='management_shared_demand_starts' THEN
   IF d->>'demandDigest' IS DISTINCT FROM x.digest OR (d->>'expectedControlRevision')::bigint IS DISTINCT FROM q.control_revision OR d->'start'->>'assignmentId' IS DISTINCT FROM q.assignment_id OR d->'start'->>'startIntentId' IS DISTINCT FROM NEW.start_intent_id OR d->'start'->'predecessor'->>'commandId' IS DISTINCT FROM o->>'predecessorCommandId' OR d->'start'->'predecessor'->>'commandDigest' IS DISTINCT FROM o->>'predecessorCommandDigest' OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts a WHERE a.id=NEW.start_intent_id AND a.runtime_id=NEW.runtime_id AND a.assignment_id=q.assignment_id AND a.input_payload::jsonb=d->'start' AND a.predecessor_id=o->>'predecessorCommandId') THEN RAISE EXCEPTION 'shared_demand_start_changed';END IF;
  ELSIF TG_TABLE_NAME='management_shared_demand_bindings' THEN
   IF d->>'version' IS DISTINCT FROM '1' OR d->>'demandId' IS DISTINCT FROM x.id OR d->>'demandDigest' IS DISTINCT FROM x.digest OR (d->>'controlRevision')::bigint IS DISTINCT FROM q.control_revision OR d->>'assignmentId' IS DISTINCT FROM q.assignment_id OR d->>'eventId' IS DISTINCT FROM q.event_id OR (d->>'receivedAtMs')::bigint IS DISTINCT FROM q.received_at_ms OR d->>'runtimeCommandId' IS DISTINCT FROM NEW.runtime_id OR d->>'workId' IS DISTINCT FROM NEW.work_id OR NEW.runtime_id IS DISTINCT FROM coalesce((SELECT runtime_id FROM hagency_agent_v1.management_shared_demand_retry_starts WHERE demand_id=x.id ORDER BY retry DESC LIMIT 1),(SELECT runtime_id FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=x.id),o->>'predecessorCommandId') OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE w.id=NEW.work_id AND w.runtime_id=NEW.runtime_id AND r.assignment_id=q.assignment_id AND w.event_id=q.event_id AND w.created_at_ms=q.received_at_ms AND w.digest=d->>'workDigest' AND r.digest=d->>'runtimeCommandDigest' AND w.payload::jsonb->>'body'=q.event_payload::jsonb->>'body' AND w.payload::jsonb->>'roomId'=q.event_payload::jsonb->>'room_id' AND w.payload::jsonb->>'requesterMxid'=q.event_payload::jsonb->>'sender_mxid' AND w.payload::jsonb->>'threadRoot'=coalesce(q.event_payload::jsonb->>'thread_root',q.event_id)) THEN RAISE EXCEPTION 'shared_demand_work_changed';END IF;
  END IF;
 END IF;
 IF q.state<>'pending' OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_controls c WHERE c.assignment_id=q.assignment_id AND c.mode='running' AND c.revision=q.control_revision) THEN RAISE EXCEPTION 'shared_demand_control_changed';END IF;
 RETURN NEW;
END;
$$;
