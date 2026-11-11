-- Shared work uses its own exact process/manager/executor scope. Personal leases
-- and their owner/device foreign keys remain unchanged.
CREATE TABLE hagency_agent_v1.management_shared_work (
 id text PRIMARY KEY,
 runtime_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_runtimes(id),
 event_id text NOT NULL,
 payload text NOT NULL,
 digest text NOT NULL,
 created_at_ms bigint NOT NULL CHECK(created_at_ms>0),
 state text NOT NULL DEFAULT 'queued' CHECK(state IN('queued','acknowledged','started','completed','failed','unknown','cancelled')),
 acknowledged_at_ms bigint,
 started_at_ms bigint,
 completed_at_ms bigint,
 start_epoch bigint,
 call_id text UNIQUE,
 latest_receipt text,
 known_consumed_tokens bigint NOT NULL DEFAULT 0 CHECK(known_consumed_tokens BETWEEN 0 AND 9007199254740991),
 retained_tokens bigint CHECK(retained_tokens BETWEEN known_consumed_tokens AND 9007199254740991),
 usage_complete boolean NOT NULL DEFAULT false,
 lease_hash text,
 lease_until_ms bigint NOT NULL DEFAULT 0,
 UNIQUE(runtime_id,event_id),
 CHECK((started_at_ms IS NULL)=(start_epoch IS NULL)),
 CHECK(start_epoch IS NULL OR start_epoch BETWEEN 1 AND 9007199254740991),
 CHECK(NOT usage_complete OR (retained_tokens IS NOT NULL AND retained_tokens=known_consumed_tokens))
);
CREATE TABLE hagency_agent_v1.management_shared_work_receipts (
 id text PRIMARY KEY,
 work_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_work(id),
 payload text NOT NULL,
 digest text NOT NULL,
 ledger_revision bigint NOT NULL CHECK(ledger_revision BETWEEN 1 AND 9007199254740991),
 UNIQUE(work_id,ledger_revision)
);
CREATE TABLE hagency_agent_v1.management_shared_replies (
 work_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_shared_work(id),
 content text NOT NULL,
 content_digest text NOT NULL,
 matrix_txn_id text NOT NULL UNIQUE,
 created_at_ms bigint NOT NULL,
 state text NOT NULL DEFAULT 'pending' CHECK(state IN('pending','issued','sent','blocked','unknown')),
 matrix_event_id text,
 lease_hash text,
 lease_until_ms bigint NOT NULL DEFAULT 0,
 sent_at_ms bigint
);
CREATE FUNCTION hagency_agent_v1.retain_shared_work() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME='management_shared_work_receipts' THEN RAISE EXCEPTION 'permanent_shared_work_history';END IF;
 IF TG_TABLE_NAME='management_shared_work' THEN
  IF (to_jsonb(NEW)-'state'-'acknowledged_at_ms'-'started_at_ms'-'completed_at_ms'-'start_epoch'-'call_id'-'latest_receipt'-'known_consumed_tokens'-'retained_tokens'-'usage_complete'-'lease_hash'-'lease_until_ms') IS DISTINCT FROM
     (to_jsonb(OLD)-'state'-'acknowledged_at_ms'-'started_at_ms'-'completed_at_ms'-'start_epoch'-'call_id'-'latest_receipt'-'known_consumed_tokens'-'retained_tokens'-'usage_complete'-'lease_hash'-'lease_until_ms')
   OR (OLD.acknowledged_at_ms IS NOT NULL AND NEW.acknowledged_at_ms IS DISTINCT FROM OLD.acknowledged_at_ms)
   OR (OLD.started_at_ms IS NOT NULL AND (NEW.started_at_ms IS DISTINCT FROM OLD.started_at_ms OR NEW.start_epoch IS DISTINCT FROM OLD.start_epoch))
   OR (OLD.call_id IS NOT NULL AND NEW.call_id IS DISTINCT FROM OLD.call_id)
   OR NEW.known_consumed_tokens<OLD.known_consumed_tokens
   OR (OLD.usage_complete AND NOT NEW.usage_complete)
   OR (OLD.state<>'queued' AND NEW.state='queued')
   OR (OLD.started_at_ms IS NOT NULL AND NEW.state IN('queued','acknowledged','cancelled'))
  THEN RAISE EXCEPTION 'immutable_shared_work';END IF;
  IF (OLD.latest_receipt IS NOT NULL AND NEW.latest_receipt IS NULL)
   OR (NEW.latest_receipt IS NULL AND (NEW.call_id IS NOT NULL OR NEW.known_consumed_tokens<>0 OR NEW.retained_tokens IS NOT NULL OR NEW.usage_complete))
   OR (NEW.latest_receipt IS NOT NULL AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work_receipts o WHERE o.id=NEW.latest_receipt AND o.work_id=NEW.id AND o.payload::jsonb->>'callId'=NEW.call_id AND (o.payload::jsonb->>'knownConsumedTokens')::bigint=NEW.known_consumed_tokens AND (o.payload::jsonb->>'retainedTokens')::bigint IS NOT DISTINCT FROM NEW.retained_tokens AND (o.payload::jsonb->>'usageComplete')::boolean=NEW.usage_complete))
  THEN RAISE EXCEPTION 'shared_work_projection_requires_receipt';END IF;
 ELSE
  IF (to_jsonb(NEW)-'state'-'matrix_event_id'-'lease_hash'-'lease_until_ms'-'sent_at_ms') IS DISTINCT FROM
     (to_jsonb(OLD)-'state'-'matrix_event_id'-'lease_hash'-'lease_until_ms'-'sent_at_ms')
   OR (OLD.matrix_event_id IS NOT NULL AND NEW.matrix_event_id IS DISTINCT FROM OLD.matrix_event_id)
   OR (OLD.state='sent' AND NEW.state<>'sent')
  THEN RAISE EXCEPTION 'immutable_shared_reply';END IF;
 END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER retain_shared_work BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_work FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_work();
CREATE TRIGGER retain_shared_work_receipt BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_work_receipts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_work();
CREATE TRIGGER retain_shared_reply BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_shared_replies FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_shared_work();
CREATE FUNCTION hagency_agent_v1.validate_shared_work() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_work' THEN
  IF NEW.payload::jsonb->>'id' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'eventId' IS DISTINCT FROM NEW.event_id OR (NEW.payload::jsonb->>'createdAtMs')::bigint IS DISTINCT FROM NEW.created_at_ms
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id JOIN hagency_agent_v1.users u ON u.id=a.manager_user_id WHERE r.id=NEW.runtime_id AND r.state='qualified' AND NOT r.permanent_fence AND a.state='pending_native' AND m.state='matrix_verified' AND NEW.payload::jsonb->>'assignmentId'=a.id AND NEW.payload::jsonb->>'runtimeCommandDigest'=r.digest AND NEW.payload::jsonb->>'roomId'=m.room_id AND NEW.payload::jsonb->>'requesterMxid'=u.mxid)
  THEN RAISE EXCEPTION 'shared_work_binding_changed';END IF;
 ELSIF TG_TABLE_NAME='management_shared_work_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'workId' IS DISTINCT FROM NEW.work_id OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE w.id=NEW.work_id AND w.started_at_ms IS NOT NULL AND NEW.payload::jsonb->>'workDigest'=w.digest AND NEW.payload::jsonb->>'executionId'=w.payload::jsonb->>'executionId' AND NEW.payload::jsonb->>'runtimeCommandDigest'=r.digest AND NEW.payload::jsonb->>'processDigest'=r.process_digest)
  THEN RAISE EXCEPTION 'shared_work_receipt_binding_changed';END IF;
 ELSE
  IF NEW.matrix_txn_id IS DISTINCT FROM 'hagency_shared_'||NEW.work_id OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_work_receipts o ON o.id=w.latest_receipt WHERE w.id=NEW.work_id AND w.state='completed' AND NEW.content::jsonb->>'msgtype'='m.text' AND NEW.content::jsonb->>'body'=o.payload::jsonb->>'body' AND NEW.content::jsonb->'m.relates_to'->>'event_id'=w.payload::jsonb->>'threadRoot') THEN RAISE EXCEPTION 'shared_reply_binding_changed';END IF;
 END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER validate_shared_work BEFORE INSERT ON hagency_agent_v1.management_shared_work FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_work();
CREATE TRIGGER validate_shared_work_receipt BEFORE INSERT ON hagency_agent_v1.management_shared_work_receipts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_work();
CREATE TRIGGER validate_shared_reply BEFORE INSERT ON hagency_agent_v1.management_shared_replies FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_shared_work();
CREATE FUNCTION hagency_agent_v1.fence_shared_work() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.permanent_fence THEN
  UPDATE hagency_agent_v1.management_shared_work SET state=CASE WHEN started_at_ms IS NULL THEN 'cancelled' ELSE 'unknown' END WHERE runtime_id=NEW.id AND state IN('queued','acknowledged','started');
  UPDATE hagency_agent_v1.management_shared_replies SET state=CASE WHEN state='issued' THEN 'unknown' ELSE 'blocked' END WHERE work_id IN(SELECT id FROM hagency_agent_v1.management_shared_work WHERE runtime_id=NEW.id) AND state IN('pending','issued');
 END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER fence_shared_work AFTER UPDATE OF permanent_fence ON hagency_agent_v1.management_shared_runtimes FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_shared_work();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=13;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=13);
