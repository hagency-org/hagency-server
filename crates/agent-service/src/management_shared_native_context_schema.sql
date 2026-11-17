-- Current native context only. Existing immutable records are retained for audit.
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_context() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE w hagency_agent_v1.management_shared_work; r hagency_agent_v1.management_shared_replies; d jsonb; c jsonb;
BEGIN
 SELECT * INTO STRICT w FROM hagency_agent_v1.management_shared_work WHERE id=NEW.work_id;
 IF TG_TABLE_NAME='management_shared_input_modes' THEN
  IF w.started_at_ms IS NOT NULL THEN RAISE EXCEPTION 'input_mode_before_start_required';END IF;
  IF NEW.mode IS DISTINCT FROM (CASE WHEN coalesce((w.payload::jsonb->>'encrypted')::boolean,false) THEN 'plain_text_v1' ELSE 'thread_context_v1' END) THEN RAISE EXCEPTION 'native_input_mode_required';END IF;
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
   OR w.started_at_ms IS NOT NULL OR (d->>'version')::integer IS DISTINCT FROM 3
   OR d->>'workId' IS DISTINCT FROM w.id OR d->>'workDigest' IS DISTINCT FROM w.digest
   OR d->>'assignmentId' IS DISTINCT FROM NEW.assignment_id OR NEW.assignment_id IS DISTINCT FROM c->>'assignmentId'
   OR d->>'runtimeCommandDigest' IS DISTINCT FROM c->>'runtimeCommandDigest'
   OR d->>'roomId' IS DISTINCT FROM NEW.room_id OR NEW.room_id IS DISTINCT FROM c->>'roomId'
   OR d->>'threadRoot' IS DISTINCT FROM NEW.thread_root OR NEW.thread_root IS DISTINCT FROM c->>'threadRoot'
   OR d->>'eventId' IS DISTINCT FROM c->>'eventId' OR (d->>'receivedAtMs')::bigint IS DISTINCT FROM w.created_at_ms
   OR (d->>'contextOrdinal')::bigint IS DISTINCT FROM NEW.ordinal
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_context_sequences WHERE assignment_id=NEW.assignment_id AND room_id=NEW.room_id AND thread_root=NEW.thread_root AND ordinal=NEW.ordinal)
   THEN RAISE EXCEPTION 'shared_context_binding_required';END IF;
  IF (d->>'inputProfile' IS DISTINCT FROM 'shared_native_text_v3'
   OR d->>'coverage' IS DISTINCT FROM 'native_session_and_paged_room_history'
   OR d->'history' IS DISTINCT FROM '[]'::jsonb OR d ? 'budget'
   OR d->'omittedEarlier' IS DISTINCT FROM 'true'::jsonb
   OR coalesce((c->>'encrypted')::boolean,false)) THEN RAISE EXCEPTION 'shared_native_context_required';END IF;

 ELSE
  IF w.started_at_ms IS NOT NULL OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_contexts x WHERE x.work_id=w.id AND x.digest=NEW.context_digest AND x.input_digest=NEW.input_digest) THEN RAISE EXCEPTION 'shared_context_start_required';END IF;
 END IF;RETURN NEW;
END; $$;

-- Unsupported queued inputs must not permanently occupy the head of a room.
-- Only unstarted work is cancelled. Payloads, receipts and accounting remain
-- immutable; an old started/unknown call is never settled or refunded here.
UPDATE hagency_agent_v1.management_shared_work w SET state='cancelled'
WHERE w.started_at_ms IS NULL AND w.state IN ('queued','acknowledged') AND (
 NOT EXISTS (SELECT 1 FROM hagency_agent_v1.management_shared_input_modes m WHERE m.work_id=w.id)
 OR (NOT coalesce((w.payload::jsonb->>'encrypted')::boolean,false) AND (
   EXISTS (SELECT 1 FROM hagency_agent_v1.management_shared_input_modes m WHERE m.work_id=w.id AND m.mode<>'thread_context_v1')
   OR EXISTS (SELECT 1 FROM hagency_agent_v1.management_shared_contexts c WHERE c.work_id=w.id AND (c.payload::jsonb->>'version')::integer IS DISTINCT FROM 3)
 ))
);
