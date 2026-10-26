CREATE TABLE hagency_agent_v1.file_delivery_denials (
 owner_event_id text PRIMARY KEY REFERENCES hagency_agent_v1.owner_events(id)
);
-- Retained text-delivery denial is historical evidence for this original event.
INSERT INTO hagency_agent_v1.file_delivery_denials(owner_event_id)
 SELECT owner_event_id FROM hagency_agent_v1.reply_outbox WHERE delivery_blocked
 ON CONFLICT DO NOTHING;
CREATE FUNCTION hagency_agent_v1.retain_file_delivery_denial() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'permanent_file_delivery_denial'; END;
$$;
CREATE TRIGGER retain_file_delivery_denial BEFORE UPDATE OR DELETE ON hagency_agent_v1.file_delivery_denials FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_file_delivery_denial();
CREATE OR REPLACE VIEW hagency_agent_v1.file_sendable AS
 SELECT o.id FROM hagency_agent_v1.file_outbox o
 JOIN hagency_agent_v1.owner_events e ON e.id=o.owner_event_id
 JOIN hagency_agent_v1.bindings b ON b.id=o.binding_id JOIN hagency_agent_v1.agents a ON a.id=o.agent_id
 JOIN hagency_agent_v1.users u ON u.id=o.owner_user_id
 LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id
 JOIN hagency_agent_v1.execution_leases l ON l.agent_id=o.agent_id JOIN hagency_agent_v1.devices d ON d.id=l.device_id JOIN hagency_agent_v1.sessions s ON s.id=d.session_id
 WHERE NOT EXISTS(SELECT 1 FROM hagency_agent_v1.file_delivery_denials x WHERE x.owner_event_id=o.owner_event_id) AND e.state IN ('running','completed') AND e.execution_id=o.execution_id AND e.dispatch_epoch=o.dispatch_epoch AND e.dispatch_device_id=o.dispatch_device_id
 AND e.agent_id=o.agent_id AND e.binding_id=o.binding_id AND e.owner_user_id=o.owner_user_id AND e.room_id=o.room_id
 AND e.thread_root=o.thread_root AND e.requester_mxid=o.requester_mxid AND b.agent_id=o.agent_id AND b.room_id=o.room_id AND a.puppet_mxid=o.puppet_mxid
 AND b.generation=o.binding_generation AND b.state='active' AND NOT b.admin_project_paused AND NOT b.admin_room_paused
 AND a.state='active' AND u.active AND a.owner_user_id=o.owner_user_id
 AND ((b.scope_kind='project' AND a.visibility='public' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))
 AND l.owner_user_id=o.owner_user_id AND l.epoch=o.dispatch_epoch AND l.device_id=o.dispatch_device_id AND a.execution_device_id=l.device_id
 AND l.device_generation=d.generation AND d.user_id=o.owner_user_id AND s.user_id=o.owner_user_id AND NOT d.revoked AND NOT s.revoked
 AND l.expires_at_ms>(extract(epoch FROM clock_timestamp())*1000)::bigint AND s.valid_until_ms>(extract(epoch FROM clock_timestamp())*1000)::bigint;
