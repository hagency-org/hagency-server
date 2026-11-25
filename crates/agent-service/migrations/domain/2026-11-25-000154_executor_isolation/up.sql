-- Administrator-attested microVM isolation per executor device generation.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.executor_isolation_attestations (
 device_id text PRIMARY KEY,
 owner_user_id text NOT NULL,
 generation bigint NOT NULL CHECK(generation>0),
 kind text NOT NULL CHECK(kind IN ('kata','firecracker')),
 host_label text NOT NULL CHECK(host_label ~ '^[A-Za-z0-9._-]{1,64}$'),
 attested_by_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 attested_at_ms bigint NOT NULL CHECK(attested_at_ms>0),
 FOREIGN KEY(device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id)
);
ALTER TABLE hagency_agent_v1.management_shared_executor_protocols
    ADD COLUMN IF NOT EXISTS isolation_satisfied boolean NOT NULL DEFAULT true;
CREATE OR REPLACE FUNCTION hagency_agent_v1.shared_executor_protocol_reason(
 owner_id text, executor_id text, executor_generation bigint, at_ms bigint
) RETURNS text LANGUAGE sql STABLE AS $$
 SELECT nullif(coalesce((
  SELECT CASE
   WHEN p.qualified AND p.generation=d.generation AND p.session_id=d.session_id THEN
    CASE WHEN NOT (p.expires_at_ms>at_ms AND NOT d.revoked AND NOT s.revoked AND s.valid_until_ms>at_ms AND u.active)
     THEN 'owner_daemon_unavailable'
     WHEN NOT p.isolation_satisfied THEN 'executor_isolation_required'
     ELSE '' END
   ELSE 'owner_daemon_update_required' END
  FROM hagency_agent_v1.devices d
  JOIN hagency_agent_v1.sessions s ON s.id=d.session_id AND s.user_id=d.user_id
  JOIN hagency_agent_v1.users u ON u.id=d.user_id
  LEFT JOIN hagency_agent_v1.management_shared_executor_protocols p ON p.device_id=d.id AND p.owner_user_id=d.user_id
  WHERE d.id=executor_id AND d.user_id=owner_id AND d.generation=executor_generation
 ),'owner_daemon_update_required'),'')
$$;
