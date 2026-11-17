-- Test-only reconstruction of pre-merge schemas.
DROP TRIGGER IF EXISTS require_owner_coordinated_grant ON hagency_agent_v1.management_resource_grants;
DROP FUNCTION IF EXISTS hagency_agent_v1.require_owner_coordinated_grant();
ALTER TABLE hagency_agent_v1.management_contributions DROP CONSTRAINT IF EXISTS resource_owner_is_coordinator;
ALTER TABLE hagency_agent_v1.users DROP CONSTRAINT IF EXISTS management_owner_mxid_key;
DROP TABLE IF EXISTS hagency_agent_v1.management_owner_coordination_merges;

-- Test-only reconstruction of version 21 for migration coverage.
DROP TRIGGER IF EXISTS establish_project_manager_authority ON hagency_agent_v1.management_projects;
DROP FUNCTION IF EXISTS hagency_agent_v1.establish_project_manager_authority();
ALTER TABLE hagency_agent_v1.management_project_authorities DROP CONSTRAINT IF EXISTS project_manager_is_authority;
ALTER TABLE hagency_agent_v1.management_projects DROP CONSTRAINT IF EXISTS management_project_manager_key;
DROP TABLE IF EXISTS hagency_agent_v1.management_project_role_merges;
DROP FUNCTION IF EXISTS hagency_agent_v1.retain_project_role_merge();
