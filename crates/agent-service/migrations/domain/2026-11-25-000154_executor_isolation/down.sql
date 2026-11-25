CREATE OR REPLACE FUNCTION hagency_agent_v1.shared_executor_protocol_reason(
 owner_id text, executor_id text, executor_generation bigint, at_ms bigint
) RETURNS text LANGUAGE sql STABLE AS $$
 SELECT nullif(coalesce((
  SELECT CASE
   WHEN p.qualified AND p.generation=d.generation AND p.session_id=d.session_id THEN
    CASE WHEN p.expires_at_ms>at_ms AND NOT d.revoked AND NOT s.revoked AND s.valid_until_ms>at_ms AND u.active
     THEN '' ELSE 'owner_daemon_unavailable' END
   ELSE 'owner_daemon_update_required' END
  FROM hagency_agent_v1.devices d
  JOIN hagency_agent_v1.sessions s ON s.id=d.session_id AND s.user_id=d.user_id
  JOIN hagency_agent_v1.users u ON u.id=d.user_id
  LEFT JOIN hagency_agent_v1.management_shared_executor_protocols p ON p.device_id=d.id AND p.owner_user_id=d.user_id
  WHERE d.id=executor_id AND d.user_id=owner_id AND d.generation=executor_generation
 ),'owner_daemon_update_required'),'')
$$;
ALTER TABLE hagency_agent_v1.management_shared_executor_protocols DROP COLUMN IF EXISTS isolation_satisfied;
DROP TABLE IF EXISTS hagency_agent_v1.executor_isolation_attestations;
