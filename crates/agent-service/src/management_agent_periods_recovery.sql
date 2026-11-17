-- Idempotent schema-30 hardening. No receipt, allocation, or term is rewritten.
-- A refused attempt proves no credit. Pending/lost receipts and authorized
-- periods remain mutually exclusive, including direct database writes.
DROP INDEX IF EXISTS hagency_agent_v1.management_agent_executor_period;
CREATE UNIQUE INDEX IF NOT EXISTS management_agent_executor_period_active
 ON hagency_agent_v1.management_agent_executor_commands(intent_id,period_key)
 WHERE operation='agent.period.renew' AND receipt::jsonb->>'state' IS DISTINCT FROM 'refused';
CREATE INDEX IF NOT EXISTS management_agent_executor_period_history
 ON hagency_agent_v1.management_agent_executor_commands(intent_id,period_key,created_at_ms DESC,id DESC)
 WHERE operation='agent.period.renew';
CREATE INDEX IF NOT EXISTS management_agent_intent_identity
 ON hagency_agent_v1.management_agent_intents((payload::jsonb->>'proposedAgentId'));
CREATE INDEX IF NOT EXISTS management_agent_request_identity
 ON hagency_agent_v1.management_agent_requests(proposed_agent_id);
CREATE TABLE IF NOT EXISTS hagency_agent_v1.management_agent_renewal_cursors (
 executor_device_id text PRIMARY KEY REFERENCES hagency_agent_v1.devices(id),
 after_intent_id text NOT NULL
);
CREATE OR REPLACE FUNCTION hagency_agent_v1.require_agent_allocation_generation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.pool_contribution_id IS NOT NULL AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_requests r WHERE r.room_id=NEW.room_id AND r.id<>NEW.id AND r.proposed_agent_id<>NEW.proposed_agent_id AND r.pool_contribution_id IS NOT NULL) THEN RAISE EXCEPTION 'pool_agent_room_used'; END IF;
 IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_requests r WHERE r.proposed_agent_id=NEW.proposed_agent_id AND r.id<>NEW.id)
 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_reallocations x
 JOIN hagency_agent_v1.management_agent_requests previous ON previous.id=x.previous_request_id
 JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=previous.id
 JOIN hagency_agent_v1.management_agent_executor_commands w ON w.intent_id=i.id AND w.operation='agent.fence'
 WHERE x.request_id=NEW.id AND previous.proposed_agent_id=NEW.proposed_agent_id
 AND previous.manager_user_id=NEW.manager_user_id AND previous.administrator_user_id=NEW.administrator_user_id
 AND previous.room_id=NEW.room_id AND previous.pool_contribution_id=NEW.pool_contribution_id
 AND previous.definition_digest=NEW.definition_digest AND previous.requested_tokens=NEW.requested_tokens
 AND i.state='fenced' AND w.receipt::jsonb->>'state' IN ('fenced','absent_fenced')
 AND x.allocation_generation=COALESCE((i.payload::jsonb->>'allocationGeneration')::bigint,1)+1)
 THEN RAISE EXCEPTION 'agent_allocation_generation_required'; END IF;
 RETURN NEW;
END;
$$;

-- Failed unreserved generations have no room assignment. Recover from the
-- nearest prior allocation that did reserve, preserving its membership rules.
CREATE OR REPLACE FUNCTION hagency_agent_v1.previous_agent_assignment(request text)
RETURNS text LANGUAGE sql STABLE AS $$
 WITH RECURSIVE prior(request_id,depth) AS (
  SELECT previous_request_id,1 FROM hagency_agent_v1.management_agent_reallocations WHERE request_id=request
  UNION ALL
  SELECT x.previous_request_id,p.depth+1 FROM hagency_agent_v1.management_agent_reallocations x JOIN prior p ON x.request_id=p.request_id WHERE p.depth<4096
 ) SELECT a.id FROM prior p JOIN hagency_agent_v1.management_shared_assignments a ON a.request_id=p.request_id ORDER BY p.depth LIMIT 1;
$$;
