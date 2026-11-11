-- Any joined Room member may send work to a shared Agent. The service checks the
-- sender's fresh membership and mention per message; work no longer has to come
-- from the manager. Other work, receipt and reply checks are unchanged.
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_work() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_work' THEN
  IF NEW.payload::jsonb->>'id' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'eventId' IS DISTINCT FROM NEW.event_id OR (NEW.payload::jsonb->>'createdAtMs')::bigint IS DISTINCT FROM NEW.created_at_ms
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id WHERE r.id=NEW.runtime_id AND r.state='qualified' AND NOT r.permanent_fence AND a.state='pending_native' AND m.state='matrix_verified' AND NEW.payload::jsonb->>'assignmentId'=a.id AND NEW.payload::jsonb->>'runtimeCommandDigest'=r.digest AND NEW.payload::jsonb->>'roomId'=m.room_id)
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

ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=28 WHERE singleton AND version=27;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=28);
