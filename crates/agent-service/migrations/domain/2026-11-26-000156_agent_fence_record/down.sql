-- Stops recording fences and keeps the records already written: they are the
-- only account of why an earlier Agent or process was withdrawn. The up step
-- runs again over the kept tables and puts the rules back.
DROP TRIGGER IF EXISTS record_agent_fence ON hagency_agent_v1.management_agent_intents;
DROP FUNCTION IF EXISTS hagency_agent_v1.record_agent_fence();
DROP TRIGGER IF EXISTS record_shared_runtime_fence ON hagency_agent_v1.management_shared_runtimes;
DROP FUNCTION IF EXISTS hagency_agent_v1.record_shared_runtime_fence();
