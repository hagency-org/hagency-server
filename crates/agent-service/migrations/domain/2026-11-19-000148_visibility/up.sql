-- Migrate only rows without an audience. Preserve existing shared services;
-- never publish an owner-only Agent just because the server was upgraded.
ALTER TABLE hagency_agent_v1.agents ADD COLUMN IF NOT EXISTS visibility text CHECK(visibility IN ('public','private'));
UPDATE hagency_agent_v1.agents a
SET visibility=CASE WHEN EXISTS(
 SELECT 1 FROM hagency_agent_v1.bindings b
 WHERE b.agent_id=a.id AND b.scope_kind='project' AND b.state<>'left'
) THEN 'public' ELSE 'private' END
WHERE visibility IS NULL;
ALTER TABLE hagency_agent_v1.agents ALTER COLUMN visibility SET NOT NULL;
ALTER TABLE hagency_agent_v1.agents ALTER COLUMN visibility SET DEFAULT 'private';
CREATE INDEX IF NOT EXISTS agents_public_idx ON hagency_agent_v1.agents(id) WHERE visibility='public';
