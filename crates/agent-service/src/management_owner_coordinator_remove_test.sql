-- Test-only reconstruction of pre-merge schemas.
DROP TRIGGER IF EXISTS require_owner_coordinated_grant ON hagency_agent_v1.management_resource_grants;
DROP FUNCTION IF EXISTS hagency_agent_v1.require_owner_coordinated_grant();
ALTER TABLE hagency_agent_v1.management_contributions DROP CONSTRAINT IF EXISTS resource_owner_is_coordinator;
ALTER TABLE hagency_agent_v1.users DROP CONSTRAINT IF EXISTS management_owner_mxid_key;
DROP TABLE IF EXISTS hagency_agent_v1.management_owner_coordination_merges;
