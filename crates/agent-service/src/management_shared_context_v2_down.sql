-- Test-only rollback of thread context version 2. Tolerates partial state so
-- every test teardown chain can run it first. Production never downgrades:
-- a version 1 service cannot read a version 2 record.
DROP TRIGGER IF EXISTS retain_shared_event_revision ON hagency_agent_v1.management_shared_event_revisions;
DROP FUNCTION IF EXISTS hagency_agent_v1.retain_shared_event_revision();
DROP INDEX IF EXISTS hagency_agent_v1.shared_event_revision_targets;
DROP INDEX IF EXISTS hagency_agent_v1.shared_work_event_lookup;
DROP INDEX IF EXISTS hagency_agent_v1.shared_reply_event_lookup;
DROP TABLE IF EXISTS hagency_agent_v1.management_shared_event_revisions;
DO $$ BEGIN
 IF to_regclass('hagency_agent_v1.management_shared_contexts') IS NOT NULL THEN
  CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_context() RETURNS trigger LANGUAGE plpgsql AS $body$
  DECLARE w hagency_agent_v1.management_shared_work; r hagency_agent_v1.management_shared_replies; d jsonb; c jsonb; h jsonb;
  BEGIN
   SELECT * INTO STRICT w FROM hagency_agent_v1.management_shared_work WHERE id=NEW.work_id;
   IF TG_TABLE_NAME='management_shared_input_modes' THEN
    IF w.started_at_ms IS NOT NULL THEN RAISE EXCEPTION 'input_mode_before_start_required';END IF;
   ELSIF TG_TABLE_NAME='management_shared_reply_deliveries' THEN
    SELECT * INTO STRICT r FROM hagency_agent_v1.management_shared_replies WHERE work_id=NEW.work_id;
    d=NEW.payload::jsonb;c=w.payload::jsonb;
    IF r.state<>'sent' OR r.matrix_event_id IS NULL OR r.sent_at_ms IS NULL OR NEW.matrix_content<>r.content
     OR (d->>'version')::integer IS DISTINCT FROM 1 OR d->>'workId' IS DISTINCT FROM w.id OR d->>'workDigest' IS DISTINCT FROM w.digest
     OR d->>'assignmentId' IS DISTINCT FROM c->>'assignmentId' OR d->>'runtimeCommandDigest' IS DISTINCT FROM c->>'runtimeCommandDigest'
     OR d->>'roomId' IS DISTINCT FROM c->>'roomId' OR d->>'threadRoot' IS DISTINCT FROM c->>'threadRoot' OR d->>'sourceEventId' IS DISTINCT FROM c->>'eventId'
     OR d->>'matrixTxnId' IS DISTINCT FROM r.matrix_txn_id OR d->>'matrixEventId' IS DISTINCT FROM r.matrix_event_id
     OR d->>'matrixContentDigest' IS DISTINCT FROM r.content_digest OR d->>'body' IS DISTINCT FROM r.content::jsonb->>'body'
     OR (d->>'sentAtMs')::bigint IS DISTINCT FROM r.sent_at_ms THEN RAISE EXCEPTION 'shared_delivery_evidence_required';END IF;
   ELSIF TG_TABLE_NAME='management_shared_contexts' THEN
    d=NEW.payload::jsonb;c=w.payload::jsonb;
    IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_input_modes WHERE work_id=w.id AND mode='thread_context_v1')
     OR w.started_at_ms IS NOT NULL OR (d->>'version')::integer IS DISTINCT FROM 1
     OR d->>'workId' IS DISTINCT FROM w.id OR d->>'workDigest' IS DISTINCT FROM w.digest
     OR d->>'assignmentId' IS DISTINCT FROM NEW.assignment_id OR NEW.assignment_id IS DISTINCT FROM c->>'assignmentId'
     OR d->>'runtimeCommandDigest' IS DISTINCT FROM c->>'runtimeCommandDigest'
     OR d->>'roomId' IS DISTINCT FROM NEW.room_id OR NEW.room_id IS DISTINCT FROM c->>'roomId'
     OR d->>'threadRoot' IS DISTINCT FROM NEW.thread_root OR NEW.thread_root IS DISTINCT FROM c->>'threadRoot'
     OR d->>'eventId' IS DISTINCT FROM c->>'eventId' OR (d->>'receivedAtMs')::bigint IS DISTINCT FROM w.created_at_ms
     OR (d->>'contextOrdinal')::bigint IS DISTINCT FROM NEW.ordinal
     OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_context_sequences WHERE assignment_id=NEW.assignment_id AND room_id=NEW.room_id AND thread_root=NEW.thread_root AND ordinal=NEW.ordinal)
     THEN RAISE EXCEPTION 'shared_context_binding_required';END IF;
    FOR h IN SELECT value FROM jsonb_array_elements(d->'history') LOOP
     IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_reply_deliveries x JOIN hagency_agent_v1.management_shared_work prior_work ON prior_work.id=x.work_id WHERE x.work_id=h->>'workId' AND x.digest=h->>'deliveryDigest' AND x.payload::jsonb=h->'delivery' AND prior_work.digest=h->>'workDigest' AND prior_work.payload::jsonb->>'assignmentId'=NEW.assignment_id AND prior_work.payload::jsonb->>'roomId'=NEW.room_id AND prior_work.payload::jsonb->>'threadRoot'=NEW.thread_root AND prior_work.payload::jsonb->>'eventId'=h->>'eventId' AND prior_work.payload::jsonb->>'requesterMxid'=h->>'requesterMxid' AND prior_work.payload::jsonb->>'body'=h->>'userBody' AND prior_work.payload::jsonb->>'bodyDigest'=h->>'userBodyDigest' AND prior_work.created_at_ms=(h->>'receivedAtMs')::bigint AND (prior_work.created_at_ms,prior_work.id)<(w.created_at_ms,w.id)) THEN RAISE EXCEPTION 'shared_context_history_evidence_required';END IF;
    END LOOP;
   ELSE
    IF w.started_at_ms IS NOT NULL OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_contexts x WHERE x.work_id=w.id AND x.digest=NEW.context_digest AND x.input_digest=NEW.input_digest) THEN RAISE EXCEPTION 'shared_context_start_required';END IF;
   END IF;RETURN NEW;
  END; $body$;
 END IF;
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_deployment WHERE singleton AND version=32) THEN
  ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
  UPDATE hagency_agent_v1.management_deployment SET version=31 WHERE singleton AND version=32;
  ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=31);
 END IF;
END $$;
