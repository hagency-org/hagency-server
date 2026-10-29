-- Run intent is separate from finite process custody. Existing assignments are
-- manual: migration cannot infer whether an old stop was human-requested.
CREATE TABLE hagency_agent_v1.management_shared_controls (
 assignment_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_assignments(id),
 revision bigint NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
 mode text NOT NULL CHECK(mode IN('manual','running','paused')),
 running_since_ms bigint,
 updated_at_ms bigint NOT NULL,
 origin text NOT NULL CHECK(origin IN('migration_unknown','legacy_manual','approved_native_assignment','manager','project_administrator','executor_stop')),
 CHECK((mode='running')=(running_since_ms IS NOT NULL))
);
INSERT INTO hagency_agent_v1.management_shared_controls SELECT id,1,'manual',NULL,(extract(epoch from clock_timestamp())*1000)::bigint,'migration_unknown' FROM hagency_agent_v1.management_shared_assignments;
CREATE TABLE hagency_agent_v1.management_shared_control_commands (
 actor_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),device_id text NOT NULL,
 FOREIGN KEY(device_id,actor_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 command_id text NOT NULL, assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_controls(assignment_id),
 input_digest text NOT NULL,result text NOT NULL,PRIMARY KEY(actor_user_id,device_id,command_id)
);
CREATE TABLE hagency_agent_v1.management_shared_intake (
 id text PRIMARY KEY,assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_controls(assignment_id),
 event_id text NOT NULL,event_payload text NOT NULL,event_digest text NOT NULL,
 control_revision bigint NOT NULL CHECK(control_revision>0),
 received_at_ms bigint NOT NULL CHECK(received_at_ms>0),expires_at_ms bigint NOT NULL CHECK(expires_at_ms>received_at_ms),
 state text NOT NULL CHECK(state IN('pending','paused','cancelled','expired','blocked','legacy_delivered')),
 reason text CHECK(reason IN('explicit_stop','paused_at_receipt','manual_start_required','before_running_intent','event_expired','assignment_authority_changed','already_delivered','room_authority_changed','run_control_changed')),
 CHECK((state='pending')=(reason IS NULL)),UNIQUE(assignment_id,event_id)
);
CREATE INDEX pending_shared_intake ON hagency_agent_v1.management_shared_intake(assignment_id,received_at_ms,id) WHERE state='pending';
CREATE FUNCTION hagency_agent_v1.retain_shared_intake() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME='management_shared_control_commands' THEN RAISE EXCEPTION 'retained_shared_intake_history';END IF;
 IF TG_TABLE_NAME='management_shared_controls' THEN
  IF NEW.assignment_id<>OLD.assignment_id OR NEW.revision<>OLD.revision+1 OR NEW.updated_at_ms<OLD.updated_at_ms OR NEW.mode='manual' THEN RAISE EXCEPTION 'shared_control_revision_required';END IF;
 ELSE
  IF (to_jsonb(NEW)-'state'-'reason') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'reason') OR OLD.state<>'pending' OR NEW.state NOT IN('cancelled','expired','blocked') THEN RAISE EXCEPTION 'immutable_shared_intake';END IF;
 END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER retain_shared_control BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_controls FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_intake();
CREATE TRIGGER retain_shared_control_command BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_control_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_intake();
CREATE TRIGGER retain_shared_intake BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_intake FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_intake();
CREATE FUNCTION hagency_agent_v1.apply_shared_pause() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.mode='paused' THEN
  UPDATE hagency_agent_v1.management_shared_runtimes SET permanent_fence=true,activity_until_ms=0 WHERE assignment_id=NEW.assignment_id AND NOT permanent_fence;
  UPDATE hagency_agent_v1.management_shared_intake SET state='cancelled',reason='explicit_stop' WHERE assignment_id=NEW.assignment_id AND state='pending';
 END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER apply_shared_pause AFTER INSERT OR UPDATE ON hagency_agent_v1.management_shared_controls FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.apply_shared_pause();
CREATE FUNCTION hagency_agent_v1.guard_shared_pause() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_controls WHERE assignment_id=NEW.assignment_id AND mode='paused') THEN RAISE EXCEPTION 'shared_assignment_paused';END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER guard_shared_pause BEFORE INSERT ON hagency_agent_v1.management_shared_runtimes FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.guard_shared_pause();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=16;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=16);
