-- Additive input/delivery evidence. No original work/command/receipt rewrite.
CREATE TABLE hagency_agent_v1.management_shared_input_modes (
 work_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_work(id),
 mode text NOT NULL CHECK(mode IN('plain_text_v1','thread_context_v1'))
);
CREATE TABLE hagency_agent_v1.management_shared_reply_deliveries (
 work_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_replies(work_id),
 payload text NOT NULL CHECK(octet_length(payload)<=65536),
 digest text NOT NULL CHECK(length(digest)=64),
 matrix_content text NOT NULL CHECK(octet_length(matrix_content)<=65536)
);
CREATE TABLE hagency_agent_v1.management_shared_context_sequences (
 assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_assignments(id),
 room_id text NOT NULL, thread_root text NOT NULL,
 ordinal bigint NOT NULL CHECK(ordinal BETWEEN 1 AND 9007199254740991),
 PRIMARY KEY(assignment_id,room_id,thread_root)
);
CREATE TABLE hagency_agent_v1.management_shared_contexts (
 work_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_work(id),
 assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_assignments(id),
 room_id text NOT NULL,thread_root text NOT NULL,ordinal bigint NOT NULL,
 payload text NOT NULL CHECK(octet_length(payload)<=98304),
 digest text NOT NULL CHECK(length(digest)=64),input_digest text NOT NULL CHECK(length(input_digest)=64),
 UNIQUE(assignment_id,room_id,thread_root,ordinal),
 FOREIGN KEY(assignment_id,room_id,thread_root) REFERENCES hagency_agent_v1.management_shared_context_sequences(assignment_id,room_id,thread_root)
);
CREATE TABLE hagency_agent_v1.management_shared_context_starts (
 work_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_contexts(work_id),
 context_digest text NOT NULL CHECK(length(context_digest)=64),
 input_digest text NOT NULL CHECK(length(input_digest)=64),
 activity_epoch bigint NOT NULL CHECK(activity_epoch BETWEEN 1 AND 9007199254740991)
);
CREATE FUNCTION hagency_agent_v1.retain_shared_context() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME<>'management_shared_context_sequences' THEN RAISE EXCEPTION 'immutable_shared_context'; END IF;
 IF NEW.assignment_id<>OLD.assignment_id OR NEW.room_id<>OLD.room_id OR NEW.thread_root<>OLD.thread_root OR NEW.ordinal<>OLD.ordinal+1 THEN RAISE EXCEPTION 'context_ordinal_required';END IF;
 RETURN NEW;
END; $$;
CREATE TRIGGER retain_shared_input_mode BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_input_modes FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_context();
CREATE TRIGGER retain_shared_delivery BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_reply_deliveries FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_context();
CREATE TRIGGER retain_shared_context_sequence BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_context_sequences FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_context();
CREATE TRIGGER retain_shared_context BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_contexts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_context();
CREATE TRIGGER retain_shared_context_start BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_context_starts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_context();
CREATE FUNCTION hagency_agent_v1.validate_shared_context() RETURNS trigger LANGUAGE plpgsql AS $$
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
END; $$;
CREATE TRIGGER validate_shared_input_mode BEFORE INSERT ON hagency_agent_v1.management_shared_input_modes FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_context();
CREATE TRIGGER validate_shared_delivery BEFORE INSERT ON hagency_agent_v1.management_shared_reply_deliveries FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_context();
CREATE TRIGGER validate_shared_context BEFORE INSERT ON hagency_agent_v1.management_shared_contexts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_context();
CREATE TRIGGER validate_shared_context_start BEFORE INSERT ON hagency_agent_v1.management_shared_context_starts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_context();
CREATE FUNCTION hagency_agent_v1.require_shared_context_start() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF OLD.started_at_ms IS NULL AND NEW.started_at_ms IS NOT NULL AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_input_modes WHERE work_id=NEW.id AND mode='thread_context_v1') AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_context_starts WHERE work_id=NEW.id AND activity_epoch=NEW.start_epoch) THEN RAISE EXCEPTION 'shared_context_start_required';END IF;
 RETURN NEW;
END; $$;
CREATE TRIGGER require_shared_context_start BEFORE UPDATE ON hagency_agent_v1.management_shared_work FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.require_shared_context_start();
CREATE INDEX shared_work_context_scope ON hagency_agent_v1.management_shared_work((payload::jsonb->>'assignmentId'),(payload::jsonb->>'roomId'),(payload::jsonb->>'threadRoot'),created_at_ms,id);
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=18;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=18);
