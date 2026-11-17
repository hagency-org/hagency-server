-- Thread context version 2: budgets derived from the model window, explicit
-- coverage counts, and retained edits/redactions of events the shared journal
-- already knows. Version 1 records keep their exact bytes and validation.
CREATE TABLE hagency_agent_v1.management_shared_event_revisions (
 revision_event_id text PRIMARY KEY,
 room_id text NOT NULL,
 target_event_id text NOT NULL,
 kind text NOT NULL CHECK(kind IN('edit','redaction')),
 sender_mxid text NOT NULL,
 body text CHECK(body IS NULL OR octet_length(body)<=65536),
 body_digest text CHECK(body_digest IS NULL OR length(body_digest)=64),
 received_at_ms bigint NOT NULL CHECK(received_at_ms>0),
 CHECK((kind='edit')=(body IS NOT NULL AND body_digest IS NOT NULL)),
 CHECK(revision_event_id<>target_event_id)
);
CREATE INDEX shared_event_revision_targets ON hagency_agent_v1.management_shared_event_revisions(room_id,target_event_id,kind,received_at_ms);
CREATE INDEX shared_work_event_lookup ON hagency_agent_v1.management_shared_work(event_id);
CREATE INDEX shared_reply_event_lookup ON hagency_agent_v1.management_shared_replies(matrix_event_id);
CREATE FUNCTION hagency_agent_v1.retain_shared_event_revision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'immutable_shared_event_revision'; END; $$;
CREATE TRIGGER retain_shared_event_revision BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_event_revisions FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_event_revision();
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_context() RETURNS trigger LANGUAGE plpgsql AS $$
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
   OR w.started_at_ms IS NOT NULL OR coalesce((d->>'version')::integer,0) NOT IN (1,2)
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
END; $$;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=32 WHERE singleton AND version=31;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=32);
