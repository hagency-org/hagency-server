-- Immutable demand offer, original start wrapper and one exact work binding.
CREATE TABLE hagency_agent_v1.management_shared_demands (
 id text PRIMARY KEY,intake_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_intake(id),
 payload text NOT NULL,digest text NOT NULL
);
CREATE TABLE hagency_agent_v1.management_shared_demand_starts (
 demand_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_demands(id),
 start_intent_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_attempts(id),
 runtime_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 payload text NOT NULL,digest text NOT NULL
);
CREATE TABLE hagency_agent_v1.management_shared_demand_bindings (
 demand_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_demands(id),
 work_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_work(id),
 runtime_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 payload text NOT NULL,digest text NOT NULL
);
CREATE TABLE hagency_agent_v1.management_shared_demand_blocks (
 demand_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_demands(id),
 reason text NOT NULL CHECK(reason IN('head_changed','budget_blocked','usage_unrepresentable')),
 payload text NOT NULL,digest text NOT NULL
);
CREATE FUNCTION hagency_agent_v1.retain_shared_demand() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'immutable_shared_demand';END;
$$;
CREATE TRIGGER retain_shared_demand BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_demands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
CREATE TRIGGER retain_shared_demand_start BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_demand_starts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
CREATE TRIGGER retain_shared_demand_binding BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_demand_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
CREATE TRIGGER retain_shared_demand_block BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_demand_blocks FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_demand();
ALTER TABLE hagency_agent_v1.management_shared_intake DROP CONSTRAINT management_shared_intake_state_check;
ALTER TABLE hagency_agent_v1.management_shared_intake ADD CONSTRAINT management_shared_intake_state_check CHECK(state IN('pending','paused','cancelled','expired','blocked','legacy_delivered','bound'));
ALTER TABLE hagency_agent_v1.management_shared_intake DROP CONSTRAINT management_shared_intake_reason_check;
ALTER TABLE hagency_agent_v1.management_shared_intake ADD CONSTRAINT management_shared_intake_reason_check CHECK(reason IN('explicit_stop','paused_at_receipt','manual_start_required','before_running_intent','event_expired','assignment_authority_changed','already_delivered','room_authority_changed','run_control_changed','head_changed','budget_blocked','usage_unrepresentable'));
ALTER TABLE hagency_agent_v1.management_shared_intake DROP CONSTRAINT management_shared_intake_check1;
ALTER TABLE hagency_agent_v1.management_shared_intake ADD CONSTRAINT management_shared_intake_check1 CHECK((state IN('pending','bound'))=(reason IS NULL));
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
CREATE FUNCTION hagency_agent_v1.validate_shared_demand() RETURNS trigger LANGUAGE plpgsql AS $$
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
   IF d->>'version' IS DISTINCT FROM '1' OR d->>'demandId' IS DISTINCT FROM x.id OR d->>'demandDigest' IS DISTINCT FROM x.digest OR (d->>'controlRevision')::bigint IS DISTINCT FROM q.control_revision OR d->>'assignmentId' IS DISTINCT FROM q.assignment_id OR d->>'eventId' IS DISTINCT FROM q.event_id OR (d->>'receivedAtMs')::bigint IS DISTINCT FROM q.received_at_ms OR d->>'runtimeCommandId' IS DISTINCT FROM NEW.runtime_id OR d->>'workId' IS DISTINCT FROM NEW.work_id OR NEW.runtime_id IS DISTINCT FROM coalesce((SELECT runtime_id FROM hagency_agent_v1.management_shared_demand_starts WHERE demand_id=x.id),o->>'predecessorCommandId') OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE w.id=NEW.work_id AND w.runtime_id=NEW.runtime_id AND r.assignment_id=q.assignment_id AND w.event_id=q.event_id AND w.created_at_ms=q.received_at_ms AND w.digest=d->>'workDigest' AND r.digest=d->>'runtimeCommandDigest' AND w.payload::jsonb->>'body'=q.event_payload::jsonb->>'body' AND w.payload::jsonb->>'roomId'=q.event_payload::jsonb->>'room_id' AND w.payload::jsonb->>'requesterMxid'=q.event_payload::jsonb->>'sender_mxid' AND w.payload::jsonb->>'threadRoot'=coalesce(q.event_payload::jsonb->>'thread_root',q.event_id)) THEN RAISE EXCEPTION 'shared_demand_work_changed';END IF;
  END IF;
 END IF;
 IF q.state<>'pending' OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_controls c WHERE c.assignment_id=q.assignment_id AND c.mode='running' AND c.revision=q.control_revision) THEN RAISE EXCEPTION 'shared_demand_control_changed';END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER validate_shared_demand BEFORE INSERT ON hagency_agent_v1.management_shared_demands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_demand();
CREATE TRIGGER validate_shared_demand_start BEFORE INSERT ON hagency_agent_v1.management_shared_demand_starts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_demand();
CREATE TRIGGER validate_shared_demand_binding BEFORE INSERT ON hagency_agent_v1.management_shared_demand_bindings FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_demand();
CREATE TRIGGER validate_shared_demand_block BEFORE INSERT ON hagency_agent_v1.management_shared_demand_blocks FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_demand();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=17;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=17);
