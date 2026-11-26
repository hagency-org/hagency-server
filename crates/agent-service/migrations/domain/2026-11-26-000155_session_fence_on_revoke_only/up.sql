-- A session's end is a fact about time, not an instruction. The Agents that a
-- session's devices execute are fenced when the session is revoked and for no
-- other change of its row: a renewal never shortens a session, and an end that
-- is set earlier by hand lets the session run out without withdrawing what a
-- later proof of the same sign-in could still use.
DROP TRIGGER IF EXISTS managed_agent_session_fence ON hagency_agent_v1.sessions;
CREATE TRIGGER managed_agent_session_fence AFTER UPDATE OF revoked ON hagency_agent_v1.sessions
 FOR EACH ROW WHEN (NEW.revoked AND NOT OLD.revoked)
 EXECUTE FUNCTION hagency_agent_v1.fence_agent_dependency();
