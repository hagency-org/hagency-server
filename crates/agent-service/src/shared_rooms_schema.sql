-- Additional Rooms do not replace an Agent's original conversation or reservation.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.shared_agent_rooms (
 id text PRIMARY KEY,
 assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_assignments(id),
 agent_id text NOT NULL REFERENCES hagency_agent_v1.agents(id),
 project_id text NOT NULL,
 room_id text NOT NULL,
 state text NOT NULL CHECK(state IN ('joining','active','leaving','left')),
 generation bigint NOT NULL DEFAULT 1 CHECK(generation>0),
 UNIQUE(assignment_id,room_id),
 FOREIGN KEY(room_id,project_id) REFERENCES hagency_agent_v1.rooms(room_id,project_id)
);
-- Retain the actual departure cause. Legacy NULL means unknown, never proof
-- that it is safe to restore a membership that somebody deliberately removed.
ALTER TABLE hagency_agent_v1.shared_agent_rooms ADD COLUMN IF NOT EXISTS departure_reason text
 CHECK(departure_reason IN ('allocation_ended','membership_removed','room_authority_changed'));
ALTER TABLE hagency_agent_v1.shared_agent_rooms ADD COLUMN IF NOT EXISTS restoration_requires_membership boolean NOT NULL DEFAULT false;
CREATE TABLE IF NOT EXISTS hagency_agent_v1.shared_agent_room_commands (
 actor_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 key text NOT NULL,
 digest text NOT NULL,
 binding_id text NOT NULL REFERENCES hagency_agent_v1.shared_agent_rooms(id),
 PRIMARY KEY(actor_user_id,key)
);
CREATE OR REPLACE VIEW hagency_agent_v1.shared_room_memberships AS
 SELECT m.assignment_id,m.agent_id,m.puppet_mxid,m.room_id,m.space_id,m.state,
 NULL::text AS binding_id,NULL::bigint AS binding_generation
 FROM hagency_agent_v1.management_agent_matrix m
 UNION ALL
 SELECT b.assignment_id,b.agent_id,m.puppet_mxid,b.room_id,p.space_id,'matrix_verified'::text,
 b.id,b.generation FROM hagency_agent_v1.shared_agent_rooms b
 JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=b.assignment_id
 JOIN hagency_agent_v1.projects p ON p.id=b.project_id
 JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id
 WHERE b.state='active' AND m.state='matrix_verified' AND p.active AND r.active
 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.scope_pauses sp WHERE sp.paused AND ((sp.kind='project' AND sp.scope_id=b.project_id) OR (sp.kind='room' AND sp.scope_id=b.room_id)));

CREATE OR REPLACE FUNCTION hagency_agent_v1.guard_shared_room() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='UPDATE' THEN
  IF (NEW.id,NEW.assignment_id,NEW.agent_id,NEW.project_id,NEW.room_id) IS DISTINCT FROM
     (OLD.id,OLD.assignment_id,OLD.agent_id,OLD.project_id,OLD.room_id) THEN
   RAISE EXCEPTION 'shared room identity is immutable';
  END IF;
  IF NEW.restoration_requires_membership IS DISTINCT FROM OLD.restoration_requires_membership AND NOT
   (OLD.state='left' AND NEW.state='joining' AND NOT NEW.restoration_requires_membership)
  THEN RAISE EXCEPTION 'immutable shared room restoration'; END IF;
  IF NEW.departure_reason IS DISTINCT FROM OLD.departure_reason AND NOT
   ((OLD.state IN ('joining','active') AND NEW.state='leaving') OR
    (OLD.state='left' AND NEW.state='joining' AND NEW.departure_reason IS NULL))
  THEN RAISE EXCEPTION 'immutable shared room departure'; END IF;
  IF NOT ((NEW.state=OLD.state AND NEW.generation=OLD.generation)
   OR (OLD.state='joining' AND NEW.state='active' AND NEW.generation=OLD.generation)
   OR (OLD.state IN ('joining','active') AND NEW.state='leaving' AND NEW.generation=OLD.generation+1)
   OR (OLD.state='leaving' AND NEW.state='left' AND NEW.generation=OLD.generation)
   OR (OLD.state='left' AND NEW.state='joining' AND NEW.generation=OLD.generation+1)) THEN
   RAISE EXCEPTION 'invalid shared room transition';
  END IF;
 END IF;
 IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments s
   JOIN hagency_agent_v1.management_agent_requests q ON q.id=s.request_id
   JOIN hagency_agent_v1.agents a ON a.id=s.agent_id
   WHERE s.id=NEW.assignment_id AND s.agent_id=NEW.agent_id AND q.pool_contribution_id IS NOT NULL
   AND a.execution_kind='shared_project' AND a.owner_user_id=s.manager_user_id AND q.room_id<>NEW.room_id) THEN
  RAISE EXCEPTION 'invalid pooled Agent room';
 END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS shared_room_guard ON hagency_agent_v1.shared_agent_rooms;
CREATE TRIGGER shared_room_guard BEFORE INSERT OR UPDATE ON hagency_agent_v1.shared_agent_rooms
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.guard_shared_room();

-- Any joined Room member may send work to a shared Agent. The service checks the
-- sender's fresh membership and mention per message; work no longer has to come
-- from the manager. Other work, receipt and reply checks are unchanged.
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_work() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_work' THEN
  IF NEW.payload::jsonb->>'id' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'eventId' IS DISTINCT FROM NEW.event_id OR (NEW.payload::jsonb->>'createdAtMs')::bigint IS DISTINCT FROM NEW.created_at_ms
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id JOIN hagency_agent_v1.shared_room_memberships m ON m.assignment_id=a.id JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id WHERE r.id=NEW.runtime_id AND r.state='qualified' AND NOT r.permanent_fence AND a.state='pending_native' AND m.state='matrix_verified' AND NEW.payload::jsonb->>'assignmentId'=a.id AND NEW.payload::jsonb->>'runtimeCommandDigest'=r.digest AND NEW.payload::jsonb->>'roomId'=m.room_id
    AND ((m.binding_id IS NULL AND NOT (NEW.payload::jsonb ? 'roomBinding'))
     OR (m.binding_id IS NOT NULL AND NEW.payload::jsonb->'roomBinding'=jsonb_build_object(
      'id',m.binding_id,'generation',m.binding_generation,'assignmentId',a.id,'roomId',m.room_id,'spaceId',m.space_id))))
  THEN RAISE EXCEPTION 'shared_work_binding_changed';END IF;
 ELSIF TG_TABLE_NAME='management_shared_work_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'workId' IS DISTINCT FROM NEW.work_id OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE w.id=NEW.work_id AND (w.started_at_ms IS NOT NULL OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work_refusals f WHERE f.work_id=w.id AND NEW.payload::jsonb->>'state'='failed' AND NEW.payload::jsonb->>'knownConsumedTokens'='0' AND NEW.payload::jsonb->>'retainedTokens'='0' AND NEW.payload::jsonb->>'usageComplete'='true')) AND NEW.payload::jsonb->>'workDigest'=w.digest AND NEW.payload::jsonb->>'executionId'=w.payload::jsonb->>'executionId' AND NEW.payload::jsonb->>'runtimeCommandDigest'=r.digest AND NEW.payload::jsonb->>'processDigest'=r.process_digest)
  THEN RAISE EXCEPTION 'shared_work_receipt_binding_changed';END IF;
 ELSE
  IF NEW.matrix_txn_id IS DISTINCT FROM 'hagency_shared_'||NEW.work_id OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_work_receipts o ON o.id=w.latest_receipt WHERE w.id=NEW.work_id AND w.state='completed' AND NEW.content::jsonb->>'msgtype'='m.text' AND NEW.content::jsonb->>'body'=o.payload::jsonb->>'body' AND NEW.content::jsonb->'m.relates_to'->>'event_id'=w.payload::jsonb->>'threadRoot') THEN RAISE EXCEPTION 'shared_reply_binding_changed';END IF;
 END IF;RETURN NEW;
END;
$$;


ALTER TABLE hagency_agent_v1.management_shared_intake ADD COLUMN IF NOT EXISTS room_binding_id text REFERENCES hagency_agent_v1.shared_agent_rooms(id);
ALTER TABLE hagency_agent_v1.management_shared_intake ADD COLUMN IF NOT EXISTS room_binding_generation bigint;
ALTER TABLE hagency_agent_v1.management_shared_e2ee_candidates ADD COLUMN IF NOT EXISTS room_binding_id text REFERENCES hagency_agent_v1.shared_agent_rooms(id);
ALTER TABLE hagency_agent_v1.management_shared_e2ee_candidates ADD COLUMN IF NOT EXISTS room_binding_generation bigint;
