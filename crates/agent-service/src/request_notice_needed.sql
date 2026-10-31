-- Sending delay starts when the answer exists, not when inference began.
CREATE OR REPLACE VIEW hagency_agent_v1.request_notice_needed AS
 SELECT e.id AS owner_event_id, e.binding_id, e.requester_mxid, e.thread_root, e.binding_generation,
 CASE
  WHEN e.state='unknown' AND r.id IS NULL THEN 'interrupted'
  WHEN e.state='cancelled' AND e.outcome='unknown' AND e.execution_id IS NULL THEN 'failed'
  WHEN e.state IN ('completed','cancelled') AND e.outcome IN ('failed','rejected') THEN e.outcome
  WHEN e.state='cancelled' AND e.outcome='expired' THEN 'expired'
  WHEN e.state IN ('pending','offered','acknowledged') AND e.created_at_ms<= (extract(epoch from clock_timestamp())*1000)::bigint-30000 THEN 'waiting'
  WHEN e.state='running' AND e.created_at_ms<= (extract(epoch from clock_timestamp())*1000)::bigint-120000 THEN 'processing'
  WHEN r.id IS NOT NULL AND r.state<>'sent' AND NOT r.delivery_blocked AND r.created_at_ms<= (extract(epoch from clock_timestamp())*1000)::bigint-30000 THEN 'delivery'
 END AS kind
 FROM hagency_agent_v1.owner_events e
 LEFT JOIN hagency_agent_v1.reply_outbox r ON r.owner_event_id=e.id
 JOIN hagency_agent_v1.request_notice_deployment d ON d.singleton AND e.created_at_ms>=d.since_ms
 JOIN hagency_agent_v1.bindings b ON b.id=e.binding_id AND b.generation=e.binding_generation
 JOIN hagency_agent_v1.agents a ON a.id=b.agent_id
 JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id
 LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id
 LEFT JOIN hagency_agent_v1.rooms room ON room.room_id=b.room_id AND room.project_id=b.project_id
 WHERE b.state='active' AND a.state='active' AND u.active AND NOT b.admin_project_paused AND NOT b.admin_room_paused
 AND ((b.scope_kind='project' AND p.active AND room.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id));
