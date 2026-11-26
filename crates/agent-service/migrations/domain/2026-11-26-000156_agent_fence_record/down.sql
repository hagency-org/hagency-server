DROP TRIGGER IF EXISTS record_agent_fence ON hagency_agent_v1.management_agent_intents;
DROP FUNCTION IF EXISTS hagency_agent_v1.record_agent_fence();
DROP TABLE IF EXISTS hagency_agent_v1.management_agent_fences;
DROP TRIGGER IF EXISTS record_shared_runtime_fence ON hagency_agent_v1.management_shared_runtimes;
DROP FUNCTION IF EXISTS hagency_agent_v1.record_shared_runtime_fence();
DROP TABLE IF EXISTS hagency_agent_v1.management_shared_runtime_fences;
