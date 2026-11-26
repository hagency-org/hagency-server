-- What can be redone is redone: the line is the start of a turn.
--
-- A message is given to a process (bound) before the process begins its turn,
-- and the start of a turn is recorded here before the turn. A process that
-- ended in between left a message that nothing had run and nothing would:
-- its binding was permanent, so no later process could be given it. Its
-- sender was told the Agent had not processed it.
--
-- From here on such a message goes back to the queue at the moment its
-- process is fenced, as if it had not been handed out. Everything that
-- applies to a waiting message then applies to it: a Stop cancels it with
-- the others, its time and its period end it, and its attempts are counted
-- (the first and at most three more). The retry itself is still opened only
-- at the executor's request with its evidence that the process stopped; an
-- executor that never asks ends the message as before.
--
-- The proof needs nobody's word: a start is recorded only for a process with
-- a live lease, a fence ends the lease, and writes are serialized. So a
-- fenced process whose work has no start never began that work.
--
-- Four guards made a binding permanent. Each is relaxed for exactly this
-- proven case and nothing else. Every statement can run again, and the new
-- table holds no foreign key, because the upgrade tests rewind the schema by
-- hand and replay the migrations.

CREATE OR REPLACE FUNCTION hagency_agent_v1.shared_work_never_began(work text) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w
  JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id
  WHERE w.id=work AND r.permanent_fence AND w.state IN('queued','acknowledged','cancelled')
   AND w.started_at_ms IS NULL AND w.latest_receipt IS NULL
   AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work_refusals x WHERE x.work_id=w.id))
$$;

-- The bindings that were released, exactly as they were. This is the history
-- the binding table used to keep by never changing.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.management_shared_demand_released_bindings (
 work_id text PRIMARY KEY,
 demand_id text NOT NULL,
 runtime_id text NOT NULL UNIQUE,
 payload text NOT NULL,digest text NOT NULL,
 released_at_ms bigint NOT NULL CHECK(released_at_ms>0)
);
CREATE INDEX IF NOT EXISTS shared_demand_released_bindings_demand ON hagency_agent_v1.management_shared_demand_released_bindings(demand_id);
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_demand_release() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_bindings b
   WHERE b.demand_id=NEW.demand_id AND b.work_id=NEW.work_id AND b.runtime_id=NEW.runtime_id AND b.payload=NEW.payload AND b.digest=NEW.digest)
  OR NOT hagency_agent_v1.shared_work_never_began(NEW.work_id)
  -- Work that a decision had already ended (its period turned, its budget
  -- changed) is not released: only work that was still to run.
  OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w WHERE w.id=NEW.work_id AND w.state IN('queued','acknowledged')) THEN
  RAISE EXCEPTION 'shared_demand_release_unproven';
 END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS validate_shared_demand_release ON hagency_agent_v1.management_shared_demand_released_bindings;
CREATE TRIGGER validate_shared_demand_release BEFORE INSERT ON hagency_agent_v1.management_shared_demand_released_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_demand_release();
-- Its own function: one shared with the demand tables would tie this table
-- to them when they are dropped and replayed.
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_shared_demand_release() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'immutable_shared_demand'; END;
$$;
DROP TRIGGER IF EXISTS retain_shared_demand_release ON hagency_agent_v1.management_shared_demand_released_bindings;
CREATE TRIGGER retain_shared_demand_release BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_demand_released_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand_release();

-- Guard 1: a binding is deleted only after it was recorded as released.
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_shared_demand_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' AND hagency_agent_v1.shared_work_never_began(OLD.work_id)
  AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_released_bindings x
   WHERE x.work_id=OLD.work_id AND x.demand_id=OLD.demand_id AND x.runtime_id=OLD.runtime_id AND x.payload=OLD.payload AND x.digest=OLD.digest) THEN
  RETURN OLD;
 END IF;
 RAISE EXCEPTION 'immutable_shared_demand';
END;
$$;
DROP TRIGGER IF EXISTS retain_shared_demand_binding ON hagency_agent_v1.management_shared_demand_bindings;
CREATE TRIGGER retain_shared_demand_binding BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_demand_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand_binding();

-- Guard 2: an event belongs to its first work, unless that work was released.
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_shared_event_pin() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' AND hagency_agent_v1.shared_work_never_began(OLD.work_id)
  AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_released_bindings x WHERE x.work_id=OLD.work_id) THEN
  RETURN OLD;
 END IF;
 RAISE EXCEPTION 'permanent_shared_runtime_history';
END;
$$;
DROP TRIGGER IF EXISTS retain_shared_event_pin ON hagency_agent_v1.management_shared_event_pins;
CREATE TRIGGER retain_shared_event_pin BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_event_pins FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_event_pin();

-- Guard 3: a bound message may wait again once its binding was released.
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_shared_intake() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME='management_shared_control_commands' THEN RAISE EXCEPTION 'retained_shared_intake_history';END IF;
 IF TG_TABLE_NAME='management_shared_controls' THEN
  IF NEW.assignment_id<>OLD.assignment_id OR NEW.revision<>OLD.revision+1 OR NEW.updated_at_ms<OLD.updated_at_ms OR NEW.mode='manual' THEN RAISE EXCEPTION 'shared_control_revision_required';END IF;
 ELSE
  IF (to_jsonb(NEW)-'state'-'reason') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'reason') THEN RAISE EXCEPTION 'immutable_shared_intake';END IF;
  -- A message whose process ended before a turn began for it waits again:
  -- its binding was released (and proven releasable) and it has no other.
  IF OLD.state='bound' AND NEW.state='pending' THEN
   IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_demand_released_bindings x ON x.demand_id=d.id WHERE d.intake_id=NEW.id)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_demand_bindings b ON b.demand_id=d.id WHERE d.intake_id=NEW.id)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_budget_demand_bindings b ON b.demand_id=d.id WHERE d.intake_id=NEW.id)
   THEN RAISE EXCEPTION 'immutable_shared_intake';END IF;
   RETURN NEW;
  END IF;
  IF OLD.state<>'pending' OR NEW.state NOT IN('cancelled','expired','blocked','bound') THEN RAISE EXCEPTION 'immutable_shared_intake';END IF;
  IF NEW.state='bound' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_budget_demand_bindings b ON b.demand_id=d.id WHERE d.intake_id=NEW.id) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demands d JOIN hagency_agent_v1.management_shared_demand_bindings b ON b.demand_id=d.id WHERE d.intake_id=NEW.id) THEN RAISE EXCEPTION 'shared_binding_required';END IF;
 END IF;RETURN NEW;
END;
$$;

-- Guard 4: a retry is also opened by the evidence that a process stopped,
-- when no turn began in it for this event.
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
  -- Only the demand's latest started attempt, proven never to have begun a
  -- turn for this event, and still the head of its chain, can be retried;
  -- numbering has no gaps.
  previous=CASE WHEN NEW.retry=1
   THEN (SELECT runtime_id FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=x.id)
   ELSE (SELECT runtime_id FROM hagency_agent_v1.management_shared_demand_retry_starts WHERE demand_id=x.id AND retry=NEW.retry-1) END;
  IF previous IS DISTINCT FROM NEW.previous_runtime_id
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts o JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=o.runtime_id
     WHERE o.id=NEW.receipt_id AND o.runtime_id=NEW.previous_runtime_id AND r.state='stopped' AND r.permanent_fence
       AND ((o.payload::jsonb->>'state'='not_started' AND r.process_digest IS NULL AND r.activity_epoch=0)
         -- or its process ran and stopped, and no turn began for this event
         -- in it: the start of a turn is recorded before the turn, and a
         -- fenced process is refused one.
         OR (o.payload::jsonb->>'state'='stopped' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w
              WHERE w.runtime_id=r.id AND w.event_id=q.event_id AND (w.started_at_ms IS NOT NULL OR w.latest_receipt IS NOT NULL)))))
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

-- The release itself, at the fence. Its trigger is named to run before
-- `fence_shared_work` (triggers on one event run in the order of their
-- names), which then cancels the work: here the work is still as the fence
-- found it, so work that a decision had already cancelled (its period
-- turned, its budget changed) is told apart and stays ended. Only a message
-- that was given to a process started for it is released: such a demand has
-- a start to retry.
CREATE OR REPLACE FUNCTION hagency_agent_v1.release_unstarted_shared_work() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE b record; at_ms bigint := (extract(epoch from clock_timestamp())*1000)::bigint;
BEGIN
 FOR b IN
  SELECT x.demand_id,x.work_id,x.runtime_id,x.payload,x.digest,d.intake_id
  FROM hagency_agent_v1.management_shared_demand_bindings x
  JOIN hagency_agent_v1.management_shared_demands d ON d.id=x.demand_id
  JOIN hagency_agent_v1.management_shared_intake q ON q.id=d.intake_id
  JOIN hagency_agent_v1.management_shared_work w ON w.id=x.work_id
  WHERE x.runtime_id=NEW.id AND q.state='bound' AND w.state IN('queued','acknowledged')
   AND hagency_agent_v1.shared_work_never_began(x.work_id)
   AND (EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_starts s WHERE s.demand_id=x.demand_id AND s.runtime_id=NEW.id)
     OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_demand_retry_starts s WHERE s.demand_id=x.demand_id AND s.runtime_id=NEW.id))
 LOOP
  -- All four steps or none. A release that cannot be made must not stand in
  -- the way of the fence: the message then ends as it did before.
  BEGIN
   INSERT INTO hagency_agent_v1.management_shared_demand_released_bindings(work_id,demand_id,runtime_id,payload,digest,released_at_ms)
    VALUES(b.work_id,b.demand_id,b.runtime_id,b.payload,b.digest,at_ms);
   DELETE FROM hagency_agent_v1.management_shared_demand_bindings WHERE demand_id=b.demand_id;
   DELETE FROM hagency_agent_v1.management_shared_event_pins WHERE work_id=b.work_id;
   UPDATE hagency_agent_v1.management_shared_intake SET state='pending',reason=NULL WHERE id=b.intake_id;
  EXCEPTION WHEN OTHERS THEN NULL;
  END;
 END LOOP;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS fence_shared_release ON hagency_agent_v1.management_shared_runtimes;
CREATE TRIGGER fence_shared_release AFTER UPDATE OF permanent_fence ON hagency_agent_v1.management_shared_runtimes
 FOR EACH ROW WHEN (NEW.permanent_fence AND NOT OLD.permanent_fence) EXECUTE FUNCTION hagency_agent_v1.release_unstarted_shared_work();
