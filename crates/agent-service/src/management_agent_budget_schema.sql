-- Additive request and native budget-delta evidence; no original amount is overwritten.
CREATE TABLE hagency_agent_v1.management_agent_topups (
 id text PRIMARY KEY, request_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_requests(id),
 assignment_id text NOT NULL REFERENCES hagency_agent_v1.management_shared_assignments(id),
 manager_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 payload text NOT NULL CHECK(octet_length(payload)<=16384),digest text NOT NULL CHECK(length(digest)=64),
 expected_budget_revision bigint NOT NULL CHECK(expected_budget_revision>0),expected_budget_digest text NOT NULL CHECK(length(expected_budget_digest)=64),
 expected_ceiling_tokens bigint NOT NULL CHECK(expected_ceiling_tokens BETWEEN 1 AND 9007199254740991),
 add_tokens bigint NOT NULL CHECK(add_tokens BETWEEN 1 AND 9007199254740991),
 purpose text NOT NULL CHECK(octet_length(purpose) BETWEEN 1 AND 2048),
 state text NOT NULL CHECK(state IN('pending','approved','declined','cancelled','cancellation_requested')),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),created_at_ms bigint NOT NULL,expires_at_ms bigint NOT NULL CHECK(expires_at_ms>created_at_ms)
);
CREATE TABLE hagency_agent_v1.management_agent_budget_commands (
 id text PRIMARY KEY,topup_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_agent_topups(id),
 request_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_requests(id),
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),executor_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id),executor_generation bigint NOT NULL,
 payload text NOT NULL CHECK(octet_length(payload)<=16384),digest text NOT NULL CHECK(length(digest)=64),created_at_ms bigint NOT NULL
);
CREATE TABLE hagency_agent_v1.management_agent_budget_results (
 command_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_agent_budget_commands(id),
 payload text NOT NULL CHECK(octet_length(payload)<=16384),digest text NOT NULL CHECK(length(digest)=64),
 ledger_revision bigint NOT NULL CHECK(ledger_revision BETWEEN 1 AND 9007199254740991),state text NOT NULL CHECK(state IN('applied','refused','cancelled_before_apply')),
 budget_revision bigint NOT NULL CHECK(budget_revision>0)
);
CREATE TABLE hagency_agent_v1.management_agent_budget_issuance (
 command_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_agent_budget_commands(id),
 executor_generation bigint NOT NULL,issued_at_ms bigint NOT NULL,expires_at_ms bigint NOT NULL CHECK(expires_at_ms>issued_at_ms)
);
CREATE TABLE hagency_agent_v1.management_agent_budget_tickets (
 command_id text NOT NULL REFERENCES hagency_agent_v1.management_agent_budget_commands(id),token_hash text NOT NULL,
 executor_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id),executor_generation bigint NOT NULL,
 purpose text NOT NULL CHECK(purpose IN('apply','observe','cancel')),issued_at_ms bigint NOT NULL,expires_at_ms bigint NOT NULL,
 PRIMARY KEY(command_id,token_hash),CHECK(expires_at_ms>issued_at_ms AND expires_at_ms-issued_at_ms<=30000)
);
CREATE TABLE hagency_agent_v1.management_agent_budget_cancellations (
 command_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_agent_budget_commands(id),
 human_command_id text NOT NULL,payload text NOT NULL,digest text NOT NULL CHECK(length(digest)=64)
);
CREATE FUNCTION hagency_agent_v1.retain_agent_budget() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME<>'management_agent_topups' THEN RAISE EXCEPTION 'immutable_agent_budget';END IF;
 IF (to_jsonb(NEW)-'state'-'revision') IS DISTINCT FROM (to_jsonb(OLD)-'state'-'revision') OR NEW.revision<>OLD.revision+1
 OR NOT((OLD.state='pending' AND NEW.state IN('approved','declined','cancelled')) OR (OLD.state='approved' AND NEW.state='cancellation_requested')) THEN RAISE EXCEPTION 'agent_budget_transition';END IF;
 RETURN NEW;
END;$$;
CREATE TRIGGER retain_agent_topup BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_topups FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_budget();
CREATE TRIGGER retain_agent_budget_command BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_budget_commands FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_budget();
CREATE TRIGGER retain_agent_budget_result BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_budget_results FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_budget();
CREATE TRIGGER retain_agent_budget_cancellation BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_budget_cancellations FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_budget();
CREATE TRIGGER retain_agent_budget_issuance BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_budget_issuance FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_budget();
CREATE UNIQUE INDEX agent_budget_applied_head ON hagency_agent_v1.management_agent_budget_results((payload::jsonb->>'assignmentId'),budget_revision) WHERE state='applied';
-- Deferred command check binds the exact assigned-admin acknowledgement committed in this transaction.
CREATE FUNCTION hagency_agent_v1.validate_agent_budget_command() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE c jsonb := NEW.payload::jsonb; t record; a record;
BEGIN
 SELECT * INTO STRICT t FROM hagency_agent_v1.management_agent_topups WHERE id=NEW.topup_id;
 SELECT * INTO STRICT a FROM hagency_agent_v1.management_shared_assignments WHERE id=t.assignment_id;
 IF NEW.request_id<>t.request_id OR NEW.owner_user_id<>a.executor_owner_user_id OR NEW.executor_device_id<>a.executor_device_id OR NEW.executor_generation<>a.executor_generation
 OR t.state NOT IN ('approved','cancellation_requested') OR c->>'commandId'<>NEW.id OR c->>'topUpId'<>t.id OR c->>'assignmentId'<>a.id OR (c->>'assignmentGeneration')::bigint<>a.generation
 OR c->>'topUpRequestDigest'<>t.digest OR c->>'reserveReceiptDigest'<>a.reserve_receipt_digest
 OR (c->>'expectedBudgetRevision')::bigint<>t.expected_budget_revision OR c->>'expectedBudgetDigest'<>t.expected_budget_digest OR (c->>'expectedCeilingTokens')::bigint<>t.expected_ceiling_tokens OR (c->>'addTokens')::bigint<>t.add_tokens
 OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_commands m JOIN hagency_agent_v1.users u ON u.id=m.actor_user_id WHERE m.command_id=c->>'approvalCommandId' AND m.operation='agent.topup.decide' AND m.digest=c->>'approvalDigest' AND u.mxid=c->>'approvingAdministratorMxid' AND m.result::jsonb->>'topUpId'=t.id AND (m.result::jsonb->>'appliedRevision')::bigint=2)
 THEN RAISE EXCEPTION 'agent_budget_command_binding'; END IF;
 RETURN NEW;
END;$$;
CREATE CONSTRAINT TRIGGER validate_agent_budget_command AFTER INSERT ON hagency_agent_v1.management_agent_budget_commands DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_agent_budget_command();
CREATE FUNCTION hagency_agent_v1.validate_agent_budget_result() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE r jsonb := NEW.payload::jsonb; c record;
BEGIN
 SELECT * INTO STRICT c FROM hagency_agent_v1.management_agent_budget_commands WHERE id=NEW.command_id;
 IF r->>'commandId'<>c.id OR r->>'commandDigest'<>c.digest OR r->>'topUpId'<>c.topup_id OR r->>'assignmentId'<>c.payload::jsonb->>'assignmentId'
 OR (r->>'ledgerRevision')::bigint<>NEW.ledger_revision OR r->>'state'<>NEW.state OR (r->>'budgetRevision')::bigint<>NEW.budget_revision
 OR r->>'originalReserveReceiptDigest'<>c.payload::jsonb->>'reserveReceiptDigest'
 OR (NEW.state='applied' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_issuance i WHERE i.command_id=c.id))
 THEN RAISE EXCEPTION 'agent_budget_result_binding';END IF;
 RETURN NEW;
END;$$;
CREATE TRIGGER validate_agent_budget_result BEFORE INSERT ON hagency_agent_v1.management_agent_budget_results FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.validate_agent_budget_result();
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=19;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=19);
