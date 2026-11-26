DROP TRIGGER IF EXISTS managed_agent_session_fence ON hagency_agent_v1.sessions;
CREATE TRIGGER managed_agent_session_fence AFTER UPDATE ON hagency_agent_v1.sessions FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
