-- Owner-approved calendar terms are immutable. An executor command is the
-- durable issuance attempt; only its native receipt proves period authority.
CREATE TABLE hagency_agent_v1.management_agent_recurring_terms (
 request_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_agent_requests(id),
 calendar text NOT NULL,
 renewal_mode text NOT NULL CHECK(renewal_mode IN ('automatic','owner'))
);
ALTER TABLE hagency_agent_v1.management_agent_executor_commands
 DROP CONSTRAINT management_agent_executor_commands_operation_check,
 DROP CONSTRAINT management_agent_executor_commands_intent_id_operation_key,
 ADD COLUMN command_payload text,
 ADD COLUMN period_key text,
 ADD CONSTRAINT management_agent_executor_operation CHECK(operation IN ('agent.reserve','agent.fence','agent.period.renew')),
 ADD CONSTRAINT management_agent_executor_period CHECK((operation='agent.period.renew' AND command_payload IS NOT NULL AND period_key IS NOT NULL) OR (operation<>'agent.period.renew' AND command_payload IS NULL AND period_key IS NULL));
CREATE UNIQUE INDEX management_agent_executor_initial ON hagency_agent_v1.management_agent_executor_commands(intent_id,operation) WHERE operation<>'agent.period.renew';
CREATE UNIQUE INDEX management_agent_executor_period ON hagency_agent_v1.management_agent_executor_commands(intent_id,period_key) WHERE operation='agent.period.renew';
CREATE FUNCTION hagency_agent_v1.retain_agent_recurring_terms() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'immutable_agent_recurring_terms'; END;
$$;
CREATE TRIGGER retain_agent_recurring_terms BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_recurring_terms FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_recurring_terms();
-- Existing trigger used ON CONFLICT with the replaced unique constraint.
CREATE OR REPLACE FUNCTION hagency_agent_v1.queue_agent_executor() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 INSERT INTO hagency_agent_v1.management_agent_executor_commands(id,intent_id,operation,created_at_ms)
 VALUES(NEW.id,NEW.id,'agent.reserve',(NEW.payload::jsonb->>'issuedAtMs')::bigint) ON CONFLICT DO NOTHING;
 IF NEW.state='fenced' THEN
  INSERT INTO hagency_agent_v1.management_agent_executor_commands(id,intent_id,operation,created_at_ms)
  VALUES('fence_'||NEW.id,NEW.id,'agent.fence',(extract(epoch from clock_timestamp())*1000)::bigint) ON CONFLICT DO NOTHING;
  UPDATE hagency_agent_v1.management_shared_assignments SET state='fenced' WHERE request_id=NEW.request_id;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TABLE hagency_agent_v1.management_grant_publication_tombstones (
 owner_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 executor_device_id text NOT NULL REFERENCES hagency_agent_v1.devices(id),
 executor_generation bigint NOT NULL CHECK(executor_generation>0),
 command_id text NOT NULL,
 operation text NOT NULL CHECK(operation IN ('grant.publish','grant.succeed')),
 payload_digest text NOT NULL,publication_digest text NOT NULL,receipt_digest text NOT NULL,
 observed_at_ms bigint NOT NULL,
 PRIMARY KEY(owner_user_id,executor_device_id,command_id)
);
CREATE TRIGGER retain_grant_publication_tombstone BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_grant_publication_tombstones FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_recurring_terms();
-- An allocation generation may end permanently while its Agent identity lives.
-- Each request and each native child remains unique and immutable.
ALTER TABLE hagency_agent_v1.management_agent_requests DROP CONSTRAINT management_agent_requests_proposed_agent_id_key;
DROP INDEX hagency_agent_v1.pool_agent_room_unique;
ALTER TABLE hagency_agent_v1.management_shared_assignments DROP CONSTRAINT management_shared_assignments_agent_id_key;
CREATE UNIQUE INDEX management_shared_assignment_live ON hagency_agent_v1.management_shared_assignments(agent_id) WHERE state='pending_native';
CREATE TABLE hagency_agent_v1.management_agent_reallocations (
 request_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_agent_requests(id),
 previous_request_id text NOT NULL UNIQUE REFERENCES hagency_agent_v1.management_agent_requests(id),
 allocation_generation bigint NOT NULL CHECK(allocation_generation>1)
);
CREATE TRIGGER retain_agent_reallocation BEFORE UPDATE OR DELETE ON hagency_agent_v1.management_agent_reallocations FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_agent_recurring_terms();
-- Reusing an identity is allowed only by the explicit successor transaction.
-- Deferred validation sees the recovery link inserted after its new request.
CREATE FUNCTION hagency_agent_v1.require_agent_allocation_generation() RETURNS trigger LANGUAGE plpgsql AS $$
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
 AND i.state='fenced' AND w.receipt::jsonb->>'state'='fenced'
 AND x.allocation_generation=COALESCE((i.payload::jsonb->>'allocationGeneration')::bigint,1)+1)
 THEN RAISE EXCEPTION 'agent_allocation_generation_required'; END IF;
 RETURN NEW;
END;
$$;
CREATE CONSTRAINT TRIGGER require_agent_allocation_generation AFTER INSERT ON hagency_agent_v1.management_agent_requests DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.require_agent_allocation_generation();
-- A V4 original execution command can be replaced only after the native
-- executor durably proves that this exact command never issued a process.
ALTER TABLE hagency_agent_v1.management_shared_runtimes ADD COLUMN IF NOT EXISTS issuance_void boolean NOT NULL DEFAULT false;
DROP INDEX hagency_agent_v1.original_shared_runtime;
CREATE UNIQUE INDEX original_shared_runtime ON hagency_agent_v1.management_shared_runtimes(assignment_id) WHERE payload::jsonb->>'version'='1' AND NOT issuance_void;
CREATE OR REPLACE FUNCTION hagency_agent_v1.retain_shared_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME<>'management_shared_runtimes' THEN RAISE EXCEPTION 'permanent_shared_runtime_history'; END IF;
 IF (to_jsonb(NEW)-'state'-'permanent_fence'-'process_digest'-'lease_hash'-'lease_mode'-'lease_until_ms'-'activity_epoch'-'activity_hash'-'activity_until_ms'-'last_renewal_id'-'last_renewal_epoch'-'issuance_void') IS DISTINCT FROM
    (to_jsonb(OLD)-'state'-'permanent_fence'-'process_digest'-'lease_hash'-'lease_mode'-'lease_until_ms'-'activity_epoch'-'activity_hash'-'activity_until_ms'-'last_renewal_id'-'last_renewal_epoch'-'issuance_void')
 OR (NEW.issuance_void IS DISTINCT FROM OLD.issuance_void AND (OLD.issuance_void OR NOT NEW.permanent_fence OR NEW.state<>'stopped' OR NEW.process_digest IS NOT NULL OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts p WHERE p.runtime_id=NEW.id AND p.payload::jsonb->>'version'='1' AND p.payload::jsonb->>'state'='not_started')))
 OR (OLD.permanent_fence AND NOT NEW.permanent_fence)
 OR (OLD.process_digest IS NOT NULL AND NEW.process_digest IS DISTINCT FROM OLD.process_digest)
 OR NEW.activity_epoch<OLD.activity_epoch
 OR (OLD.state='stopped' AND NEW.state<>'stopped')
 OR (OLD.state<>'pending' AND NEW.state='pending')
 OR (NEW.permanent_fence AND NEW.activity_until_ms<>0)
 THEN RAISE EXCEPTION 'immutable_shared_runtime_scope'; END IF;
 RETURN NEW;
END;
$$;
CREATE OR REPLACE FUNCTION hagency_agent_v1.validate_shared_runtime() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_shared_runtimes' THEN
  IF NEW.issuance_void THEN RAISE EXCEPTION 'shared_original_not_unstarted'; END IF;
 END IF;
 IF TG_TABLE_NAME='management_shared_process_receipts' THEN
  IF NEW.payload::jsonb->>'observationId' IS DISTINCT FROM NEW.id OR NEW.payload::jsonb->>'commandId' IS DISTINCT FROM NEW.runtime_id
   OR (NEW.payload::jsonb->>'ledgerRevision')::bigint IS DISTINCT FROM NEW.ledger_revision THEN RAISE EXCEPTION 'shared_process_receipt_mismatch'; END IF;
  -- Only an explicit native no-issuance proof may retire a never-started
  -- recurring original command. No absence of evidence grants a retry.
  IF NEW.payload::jsonb->>'version'='1' AND NEW.payload::jsonb->>'state'='not_started' THEN
   IF NEW.payload::jsonb->>'processDigest' IS NOT NULL OR NEW.payload::jsonb->>'policyDigest' IS NOT NULL
    OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r WHERE r.id=NEW.runtime_id AND r.payload::jsonb->>'version'='1' AND r.payload::jsonb->'reserveCommand'->'poolGrant'->>'version'='4' AND r.permanent_fence AND r.state IN('pending','issued') AND r.activity_epoch=0 AND r.process_digest IS NULL)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w WHERE w.runtime_id=NEW.runtime_id)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts p WHERE p.runtime_id=NEW.runtime_id)
   THEN RAISE EXCEPTION 'shared_original_not_unstarted'; END IF;
   RETURN NEW;
  END IF;
  IF NEW.payload::jsonb->>'state'='not_started' AND (
    NEW.payload::jsonb->>'version' IS DISTINCT FROM '3'
    OR NEW.payload::jsonb->>'processDigest' IS NOT NULL OR NEW.payload::jsonb->>'policyDigest' IS NOT NULL
    OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_runtimes r WHERE r.id=NEW.runtime_id AND r.payload::jsonb->>'version'='3' AND r.permanent_fence AND r.state IN('pending','issued') AND r.activity_epoch=0 AND r.process_digest IS NULL)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w WHERE w.runtime_id=NEW.runtime_id)
    OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts p WHERE p.runtime_id=NEW.runtime_id)
  ) THEN RAISE EXCEPTION 'shared_attempt_not_unstarted'; END IF;
  IF EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts p WHERE p.runtime_id=NEW.runtime_id AND p.payload::jsonb->>'state'='not_started') THEN RAISE EXCEPTION 'shared_attempt_permanently_unstarted';END IF;
 ELSIF NEW.payload::jsonb->>'version' IN('2','3') THEN
  IF NEW.payload::jsonb->>'capability' IS DISTINCT FROM (CASE NEW.payload::jsonb->>'version' WHEN '3' THEN 'hagency.managed_agents.execution.v3' ELSE 'hagency.managed_agents.execution.v2' END)
   OR NEW.payload::jsonb->>'operation' IS DISTINCT FROM 'agent.execution.provision'
   OR NEW.payload::jsonb->>'commandId' IS DISTINCT FROM NEW.id
   OR NEW.payload::jsonb->>'assignmentId' IS DISTINCT FROM NEW.assignment_id
   OR (NEW.payload::jsonb->>'issuedAtMs')::bigint IS DISTINCT FROM NEW.created_at_ms
   OR (NEW.payload::jsonb->>'expiresAtMs')::bigint IS DISTINCT FROM NEW.expires_at_ms
   OR (NEW.payload::jsonb->>'version'='3' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_budget_attempts bp WHERE bp.runtime_id=NEW.id AND bp.assignment_id=NEW.assignment_id AND bp.pin_payload::jsonb=NEW.payload::jsonb->'executionBudget'))
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_attempts x
     JOIN hagency_agent_v1.management_shared_runtimes prior ON prior.id=x.predecessor_id
     JOIN hagency_agent_v1.management_shared_runtimes original ON original.id=x.original_runtime_id
     JOIN hagency_agent_v1.management_shared_assignments a ON a.id=x.assignment_id
     JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id
     JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id
     WHERE x.runtime_id=NEW.id AND x.assignment_id=NEW.assignment_id
       AND a.state='pending_native' AND i.state<>'fenced' AND m.state='matrix_verified'
       AND prior.assignment_id=a.id AND prior.state='stopped' AND prior.permanent_fence
       AND original.assignment_id=a.id AND original.payload::jsonb->>'version'='1'
       AND NEW.payload::jsonb->'originalRuntimeCommand'=original.payload::jsonb
       AND NEW.payload::jsonb->>'originalRuntimeDigest'=original.digest
       AND NEW.payload::jsonb->>'agentReservationId'=a.agent_reservation_id
       AND (NEW.payload::jsonb->>'assignmentGeneration')::bigint=a.generation
       AND NEW.payload::jsonb->>'startIntentId'=x.id
       AND (NEW.payload::jsonb->>'processAttempt')::bigint=x.process_attempt
       AND NEW.payload::jsonb->'predecessor'->>'commandId'=prior.id
       AND NEW.payload::jsonb->'predecessor'->>'commandDigest'=prior.digest
       AND (NEW.payload::jsonb->'predecessor'->>'processDigest'=prior.process_digest OR (NEW.payload::jsonb->>'version'='3' AND NEW.payload::jsonb->'predecessor'->>'processDigest'='' AND prior.process_digest IS NULL))
       AND (NEW.payload::jsonb->'predecessor'->>'processAttempt')::bigint=x.process_attempt-1
       AND coalesce((prior.payload::jsonb->>'processAttempt')::bigint,1)=x.process_attempt-1
       AND EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_process_receipts o
         WHERE o.runtime_id=prior.id AND o.digest=NEW.payload::jsonb->'predecessor'->>'stoppedReceiptDigest'
          AND ((o.payload::jsonb->>'state'='stopped' AND o.payload::jsonb->>'processDigest'=prior.process_digest) OR (NEW.payload::jsonb->>'version'='3' AND o.payload::jsonb->>'version'='3' AND o.payload::jsonb->>'state'='not_started' AND o.payload::jsonb->>'processDigest' IS NULL AND prior.process_digest IS NULL AND NEW.payload::jsonb->'predecessor'->>'processDigest'=''))))
  THEN RAISE EXCEPTION 'fresh_shared_attempt_required';END IF;
 ELSE
  IF NEW.payload::jsonb->>'capability' IS DISTINCT FROM 'hagency.managed_agents.execution.v1'
   OR NEW.payload::jsonb->>'operation' IS DISTINCT FROM 'agent.execution.provision'
   OR NEW.payload::jsonb->>'policy' IS DISTINCT FROM 'codex_text_v1'
   OR (NEW.payload::jsonb->>'version')::integer IS DISTINCT FROM 1
   OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_runtime_commands WHERE assignment_id=NEW.assignment_id)
   OR NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments a JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=a.id
    WHERE a.id=NEW.assignment_id AND a.state='pending_native' AND i.state<>'fenced' AND m.state='matrix_verified'
    AND NEW.payload::jsonb->>'commandId'=NEW.id AND NEW.payload::jsonb->>'assignmentId'=a.id
    AND NEW.payload::jsonb->>'agentReservationId'=a.agent_reservation_id
    AND NEW.payload::jsonb->>'reserveReceiptDigest'=a.reserve_receipt_digest
    AND NEW.payload::jsonb->>'reserveCommandDigest'=i.digest AND NEW.payload::jsonb->'reserveCommand'=i.payload::jsonb
    AND (NEW.payload::jsonb->>'assignmentGeneration')::bigint=a.generation
    AND (NEW.payload::jsonb->>'issuedAtMs')::bigint=NEW.created_at_ms
    AND (NEW.payload::jsonb->>'expiresAtMs')::bigint=NEW.expires_at_ms)
  THEN RAISE EXCEPTION 'fresh_shared_assignment_required'; END IF;
 END IF; RETURN NEW;
END;
$$;


ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=30 WHERE singleton AND version=29;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=30);
