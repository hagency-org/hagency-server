-- Current worker protocol is an authenticated, short-lived declaration. It is
-- not execution authority and never changes existing work or accounting.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.management_shared_executor_protocols (
 device_id text PRIMARY KEY,
 owner_user_id text NOT NULL,
 generation bigint NOT NULL CHECK(generation>0),
 session_id text NOT NULL,
 qualified boolean NOT NULL,
 observed_at_ms bigint NOT NULL CHECK(observed_at_ms>0),
 expires_at_ms bigint NOT NULL CHECK(expires_at_ms>=observed_at_ms),
 FOREIGN KEY(device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 FOREIGN KEY(session_id,owner_user_id) REFERENCES hagency_agent_v1.sessions(id,user_id)
);

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
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=34 WHERE singleton AND version=33;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=34);
