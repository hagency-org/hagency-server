-- Preserve a legacy staged prefix in file_outbox.content. New bytes append to
-- immutable chunks, so old partially staged/queued/unknown deliveries survive.
CREATE TABLE hagency_agent_v1.file_chunks (
 file_id text NOT NULL REFERENCES hagency_agent_v1.file_outbox(id),
 byte_offset integer NOT NULL CHECK(byte_offset>=0),
 content bytea NOT NULL CHECK(octet_length(content) BETWEEN 1 AND 65536),
 PRIMARY KEY(file_id,byte_offset)
);
CREATE FUNCTION hagency_agent_v1.guard_file_chunk() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE parent hagency_agent_v1.file_outbox%ROWTYPE;
BEGIN
 SELECT * INTO parent FROM hagency_agent_v1.file_outbox WHERE id=coalesce(NEW.file_id,OLD.file_id) FOR UPDATE;
 IF TG_OP='DELETE' THEN
  IF parent.state NOT IN ('sent','cancelled') THEN RAISE EXCEPTION 'permanent_file_chunk'; END IF;
  RETURN OLD;
 END IF;
 IF TG_OP='UPDATE' OR parent.state<>'staging' OR NEW.byte_offset<>parent.received_bytes
  OR NEW.byte_offset+octet_length(NEW.content)>(parent.manifest->>'bytes')::bigint
 THEN RAISE EXCEPTION 'invalid_file_chunk'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_file_chunk BEFORE INSERT OR UPDATE OR DELETE ON hagency_agent_v1.file_chunks
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.guard_file_chunk();
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_file_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'permanent_file_delivery'; END IF;
 IF TG_OP='INSERT' THEN
  IF NOT EXISTS(SELECT 1 FROM hagency_agent_v1.owner_events e JOIN hagency_agent_v1.bindings b ON b.id=e.binding_id JOIN hagency_agent_v1.agents a ON a.id=e.agent_id
    WHERE e.id=NEW.owner_event_id AND e.state='running' AND e.execution_id=NEW.execution_id
    AND e.agent_id=NEW.agent_id AND e.binding_id=NEW.binding_id AND e.owner_user_id=NEW.owner_user_id
    AND e.room_id=NEW.room_id AND a.puppet_mxid=NEW.puppet_mxid AND e.requester_mxid=NEW.requester_mxid
    AND e.thread_root=NEW.thread_root AND e.binding_generation=NEW.binding_generation AND e.dispatch_epoch=NEW.dispatch_epoch
    AND e.dispatch_device_id=NEW.dispatch_device_id) OR NEW.state<>'staging' OR NEW.received_bytes<>0 OR octet_length(NEW.content)<>0
    OR NEW.mxc_url IS NOT NULL OR NEW.worker_phase IS NOT NULL OR NEW.worker_token IS NOT NULL OR NEW.worker_until_ms<>0
  THEN RAISE EXCEPTION 'invalid_file_delivery_scope'; END IF;
  RETURN NEW;
 END IF;
 IF (NEW.id,NEW.request_id,NEW.owner_event_id,NEW.execution_id,NEW.agent_id,NEW.binding_id,NEW.owner_user_id,NEW.room_id,NEW.puppet_mxid,NEW.requester_mxid,NEW.thread_root,NEW.binding_generation,NEW.dispatch_epoch,NEW.dispatch_device_id,NEW.manifest,NEW.manifest_digest,NEW.created_at_ms)
  IS DISTINCT FROM (OLD.id,OLD.request_id,OLD.owner_event_id,OLD.execution_id,OLD.agent_id,OLD.binding_id,OLD.owner_user_id,OLD.room_id,OLD.puppet_mxid,OLD.requester_mxid,OLD.thread_root,OLD.binding_generation,OLD.dispatch_epoch,OLD.dispatch_device_id,OLD.manifest,OLD.manifest_digest,OLD.created_at_ms)
 THEN RAISE EXCEPTION 'immutable_file_delivery'; END IF;
 IF NOT (
  (OLD.state='staging' AND NEW.state='staging' AND NEW.content=OLD.content AND NEW.received_bytes>OLD.received_bytes AND EXISTS(SELECT 1 FROM hagency_agent_v1.file_chunks c WHERE c.file_id=OLD.id AND c.byte_offset=OLD.received_bytes AND c.byte_offset+octet_length(c.content)=NEW.received_bytes))
  OR (OLD.state='staging' AND NEW.state='queued' AND NEW.content=OLD.content AND NEW.received_bytes=(NEW.manifest->>'bytes')::bigint)
  OR (OLD.state='queued' AND NEW.state='uploading' AND NEW.content=OLD.content AND NEW.received_bytes=OLD.received_bytes AND NEW.worker_phase='upload' AND NEW.worker_token IS NOT NULL)
  OR (OLD.state='uploading' AND NEW.state='uploaded' AND NEW.mxc_url IS NOT NULL AND NEW.worker_token=OLD.worker_token AND NEW.worker_phase='upload' AND NEW.content=OLD.content)
  OR (OLD.state='uploaded' AND NEW.state='sending' AND NEW.worker_phase='send' AND NEW.worker_token IS NOT NULL AND NEW.content=OLD.content)
  OR (OLD.state IN ('uploading','sending') AND NEW.state='unknown' AND NEW.worker_token=OLD.worker_token AND NEW.worker_phase=OLD.worker_phase AND NEW.content=OLD.content)
  OR (OLD.state='unknown' AND NEW.state='unknown' AND OLD.worker_phase='upload' AND NEW.worker_token=OLD.worker_token AND OLD.mxc_url IS NULL AND NEW.mxc_url IS NOT NULL AND NEW.content=OLD.content)
  OR (OLD.state IN ('sending','unknown') AND OLD.worker_phase='send' AND NEW.state='sent' AND NEW.matrix_event_id IS NOT NULL AND NEW.worker_token=OLD.worker_token AND octet_length(NEW.content)=0)
  OR (OLD.state IN ('staging','queued','uploaded') AND NEW.state='cancelled' AND octet_length(NEW.content)=0)
 ) THEN RAISE EXCEPTION 'invalid_file_delivery_transition'; END IF;
 IF NOT ((OLD.state='queued' AND NEW.state='uploading') OR (OLD.state='uploaded' AND NEW.state='sending'))
  AND (NEW.worker_phase,NEW.worker_token,NEW.worker_until_ms) IS DISTINCT FROM (OLD.worker_phase,OLD.worker_token,OLD.worker_until_ms)
 THEN RAISE EXCEPTION 'immutable_file_worker'; END IF;
 IF NEW.mxc_url IS DISTINCT FROM OLD.mxc_url AND NOT (OLD.worker_phase='upload' AND OLD.state IN ('uploading','unknown') AND NEW.state IN ('uploaded','unknown') AND OLD.mxc_url IS NULL AND NEW.mxc_url IS NOT NULL)
 THEN RAISE EXCEPTION 'invalid_file_media'; END IF;
 IF NEW.state NOT IN ('staging','cancelled') AND NEW.received_bytes<>OLD.received_bytes THEN RAISE EXCEPTION 'immutable_file_length'; END IF;
 IF OLD.mxc_url IS NOT NULL AND NEW.mxc_url IS DISTINCT FROM OLD.mxc_url THEN RAISE EXCEPTION 'immutable_file_media'; END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS retain_file_outbox ON hagency_agent_v1.file_outbox;
CREATE TRIGGER retain_file_outbox BEFORE INSERT OR UPDATE OR DELETE ON hagency_agent_v1.file_outbox
 FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_file_outbox();
CREATE FUNCTION hagency_agent_v1.clear_terminal_file_chunks() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.state IN ('sent','cancelled') THEN DELETE FROM hagency_agent_v1.file_chunks WHERE file_id=NEW.id; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER clear_terminal_file_chunks AFTER UPDATE OF state ON hagency_agent_v1.file_outbox
 FOR EACH ROW WHEN (NEW.state IN ('sent','cancelled')) EXECUTE FUNCTION hagency_agent_v1.clear_terminal_file_chunks();
CREATE INDEX file_outbox_owner_active ON hagency_agent_v1.file_outbox(owner_user_id) WHERE state NOT IN ('sent','cancelled');
