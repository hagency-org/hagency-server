CREATE TABLE hagency_agent_v1.file_outbox (
 id text PRIMARY KEY, request_id text NOT NULL, owner_event_id text NOT NULL REFERENCES hagency_agent_v1.owner_events(id),
 execution_id text NOT NULL, agent_id text NOT NULL, binding_id text NOT NULL, owner_user_id text NOT NULL,
 room_id text NOT NULL, puppet_mxid text NOT NULL, requester_mxid text NOT NULL, thread_root text NOT NULL,
 binding_generation bigint NOT NULL, dispatch_epoch bigint NOT NULL, dispatch_device_id text NOT NULL,
 manifest jsonb NOT NULL, manifest_digest text NOT NULL CHECK(length(manifest_digest)=64),
 received_bytes bigint NOT NULL DEFAULT 0 CHECK(received_bytes>=0 AND received_bytes<=8388608),
 content bytea NOT NULL DEFAULT ''::bytea,
 state text NOT NULL CHECK(state IN ('staging','queued','uploading','uploaded','sending','sent','unknown','cancelled')),
 mxc_url text, worker_phase text CHECK(worker_phase IN ('upload','send')), worker_token text, worker_until_ms bigint NOT NULL DEFAULT 0,
 matrix_event_id text, created_at_ms bigint NOT NULL,
 UNIQUE(owner_event_id,request_id),
 CHECK((state='sent')=(matrix_event_id IS NOT NULL)),
 CHECK(state NOT IN ('uploaded','sending','sent') OR mxc_url IS NOT NULL)
);
CREATE INDEX file_outbox_pending ON hagency_agent_v1.file_outbox(state,id);
CREATE FUNCTION hagency_agent_v1.retain_file_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
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
  (OLD.state='staging' AND NEW.state='staging' AND NEW.received_bytes>OLD.received_bytes AND substring(NEW.content FROM 1 FOR OLD.received_bytes::integer)=OLD.content AND octet_length(NEW.content)=NEW.received_bytes)
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
CREATE TRIGGER retain_file_outbox BEFORE INSERT OR UPDATE OR DELETE ON hagency_agent_v1.file_outbox FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_file_outbox();
CREATE VIEW hagency_agent_v1.file_sendable AS
 SELECT o.id FROM hagency_agent_v1.file_outbox o
 JOIN hagency_agent_v1.owner_events e ON e.id=o.owner_event_id
 JOIN hagency_agent_v1.bindings b ON b.id=o.binding_id JOIN hagency_agent_v1.agents a ON a.id=o.agent_id
 JOIN hagency_agent_v1.users u ON u.id=o.owner_user_id
 LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id
 JOIN hagency_agent_v1.execution_leases l ON l.agent_id=o.agent_id JOIN hagency_agent_v1.devices d ON d.id=l.device_id JOIN hagency_agent_v1.sessions s ON s.id=d.session_id
 WHERE e.state IN ('running','completed') AND e.execution_id=o.execution_id AND e.dispatch_epoch=o.dispatch_epoch AND e.dispatch_device_id=o.dispatch_device_id
 AND e.agent_id=o.agent_id AND e.binding_id=o.binding_id AND e.owner_user_id=o.owner_user_id AND e.room_id=o.room_id
 AND e.thread_root=o.thread_root AND e.requester_mxid=o.requester_mxid AND b.agent_id=o.agent_id AND b.room_id=o.room_id AND a.puppet_mxid=o.puppet_mxid
 AND b.generation=o.binding_generation AND b.state='active' AND NOT b.admin_project_paused AND NOT b.admin_room_paused
 AND a.state='active' AND u.active AND a.owner_user_id=o.owner_user_id
 AND ((b.scope_kind='project' AND a.visibility='public' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))
 AND l.owner_user_id=o.owner_user_id AND l.epoch=o.dispatch_epoch AND l.device_id=o.dispatch_device_id AND a.execution_device_id=l.device_id
 AND l.device_generation=d.generation AND d.user_id=o.owner_user_id AND s.user_id=o.owner_user_id AND NOT d.revoked AND NOT s.revoked
 AND l.expires_at_ms>(extract(epoch FROM clock_timestamp())*1000)::bigint AND s.valid_until_ms>(extract(epoch FROM clock_timestamp())*1000)::bigint;
