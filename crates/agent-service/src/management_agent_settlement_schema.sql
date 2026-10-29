-- Additive cleanup observations. Existing fences and receipts are never backfilled.
CREATE TABLE hagency_agent_v1.management_agent_settlement_commands (
 id text PRIMARY KEY,
 request_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_requests(id),
 fence_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_executor_commands(id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 executor_device_id text NOT NULL,
 executor_generation bigint NOT NULL CHECK(executor_generation>0),
 FOREIGN KEY(executor_device_id,owner_user_id) REFERENCES hagency_agent_v1.devices(id,user_id),
 payload text NOT NULL CHECK(octet_length(payload)<=20480),
 digest text NOT NULL CHECK(length(digest)=64),
 created_at_ms bigint NOT NULL CHECK(created_at_ms>0)
);
CREATE INDEX agent_settlement_request ON hagency_agent_v1.management_agent_settlement_commands(request_id,id);
CREATE INDEX agent_settlement_executor ON hagency_agent_v1.management_agent_settlement_commands(owner_user_id,executor_device_id,id);
CREATE TABLE hagency_agent_v1.management_agent_settlement_results (
 command_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_agent_settlement_commands(id),
 payload text NOT NULL CHECK(octet_length(payload)<=8192),
 digest text NOT NULL CHECK(length(digest)=64),
 ledger_revision bigint NOT NULL CHECK(ledger_revision BETWEEN 1 AND 9007199254740991)
);
CREATE TABLE hagency_agent_v1.management_agent_settlement_issuance (
 command_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_agent_settlement_commands(id),
 issued_at_ms bigint NOT NULL CHECK(issued_at_ms>0)
);
CREATE TABLE hagency_agent_v1.management_agent_settlement_tickets (
 command_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_settlement_commands(id),
 token_hash text NOT NULL, executor_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id),
 executor_generation bigint NOT NULL CHECK(executor_generation>0),
 issued_at_ms bigint NOT NULL, expires_at_ms bigint NOT NULL,
 PRIMARY KEY(command_id,token_hash),
 CHECK(expires_at_ms>issued_at_ms AND expires_at_ms-issued_at_ms<=30000)
);
CREATE TRIGGER retain_agent_settlement_command BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_settlement_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_budget();
CREATE TRIGGER retain_agent_settlement_result BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_settlement_results FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_budget();
CREATE TRIGGER retain_agent_settlement_issuance BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_settlement_issuance FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_budget();
CREATE FUNCTION hagency_agent_v1.validate_agent_settlement() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE c record; f record; i record; p jsonb := NEW.payload::jsonb;
BEGIN
 IF TG_TABLE_NAME='management_agent_settlement_commands' THEN
  SELECT * INTO STRICT f FROM hagency_agent_v1.management_agent_executor_commands WHERE id=NEW.fence_id;
  SELECT * INTO STRICT i FROM hagency_agent_v1.management_agent_intents WHERE id=f.intent_id;
  IF f.operation<>'agent.fence' OR f.receipt IS NULL OR i.state<>'fenced'
    OR NEW.request_id<>i.request_id OR NEW.owner_user_id<>i.executor_owner_user_id
    OR NEW.executor_device_id<>i.executor_device_id OR NEW.executor_generation<>i.executor_generation
    OR p->>'commandId' IS DISTINCT FROM NEW.id OR p->>'operation' IS DISTINCT FROM 'agent.settle'
    OR p->>'fenceReceiptDigest' IS DISTINCT FROM f.receipt_digest
    OR p->'fenceCommand'->>'commandId' IS DISTINCT FROM f.id
    OR p->'fenceCommand'->'reserveCommand' IS DISTINCT FROM i.payload::jsonb
  THEN RAISE EXCEPTION 'agent_settlement_fence_required';END IF;
 ELSE
  SELECT * INTO STRICT c FROM hagency_agent_v1.management_agent_settlement_commands WHERE id=NEW.command_id;
  IF p->>'commandId' IS DISTINCT FROM c.id OR p->>'commandDigest' IS DISTINCT FROM c.digest
    OR p->>'requestId' IS DISTINCT FROM c.request_id
    OR p->>'fenceReceiptDigest' IS DISTINCT FROM c.payload::jsonb->>'fenceReceiptDigest'
    OR (p->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision
    OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_settlement_issuance WHERE command_id=c.id)
  THEN RAISE EXCEPTION 'agent_settlement_result_binding';END IF;
 END IF;
 RETURN NEW;
END;$$;
CREATE TRIGGER validate_agent_settlement_command BEFORE INSERT ON hagency_agent_v1.management_agent_settlement_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_agent_settlement();
CREATE TRIGGER validate_agent_settlement_result BEFORE INSERT ON hagency_agent_v1.management_agent_settlement_results FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_agent_settlement();
CREATE VIEW hagency_agent_v1.management_agent_settlement_evidence AS
SELECT request_id,max(ledger_revision) AS ledger_revision FROM (
 SELECT i.request_id,(w.receipt::jsonb->>'ledgerRevision')::bigint AS ledger_revision FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE w.receipt IS NOT NULL
 UNION ALL SELECT c.request_id,r.ledger_revision FROM hagency_agent_v1.management_agent_budget_results r JOIN hagency_agent_v1.management_agent_budget_commands c ON c.id=r.command_id
 UNION ALL SELECT a.request_id,o.ledger_revision FROM hagency_agent_v1.management_shared_process_receipts o JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=o.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id
 UNION ALL SELECT a.request_id,o.ledger_revision FROM hagency_agent_v1.management_shared_work_receipts o JOIN hagency_agent_v1.management_shared_work w ON w.id=o.work_id JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id
 UNION ALL SELECT a.request_id,o.ledger_revision FROM hagency_agent_v1.management_agent_runtime_observations o JOIN hagency_agent_v1.management_agent_runtime_commands c ON c.id=o.command_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=c.assignment_id
) observations GROUP BY request_id;
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=21;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=21);
