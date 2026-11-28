-- Puts back the record rules of migration `2026-11-27-000158` (for a
-- reservation) and `2026-11-26-000156` (for a process): an error in writing a
-- record is again dropped without a warning. The fences and the records are
-- unchanged. The up step runs again over them.
CREATE OR REPLACE FUNCTION hagency_agent_v1.record_agent_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE at_ms bigint := (extract(epoch from clock_timestamp())*1000)::bigint;
 naming text;
 named_source text;
 reserve_ends_ms bigint;
BEGIN
 -- The record never stands in the way of the fence it records.
 BEGIN
  -- Named by this statement, and so not by a rule of the database.
  naming := current_setting('hagency.fence_source',true);
  IF pg_trigger_depth()<=1 AND split_part(naming,'@',2)=statement_timestamp()::text THEN
   named_source := split_part(naming,'@',1);
  END IF;
  -- The reserve command is the reservation's payload. One that does not
  -- read as expected leaves this fact out, not the record.
  BEGIN
   reserve_ends_ms := (NEW.payload::jsonb->>'expiresAtMs')::bigint;
  EXCEPTION WHEN OTHERS THEN reserve_ends_ms := NULL;
  END;
  INSERT INTO hagency_agent_v1.management_agent_fences(intent_id,fenced_at_ms,cause,facts)
  SELECT NEW.id,at_ms,
   CASE WHEN pg_trigger_depth()>1 THEN 'dependency' ELSE 'service' END,
   jsonb_strip_nulls(jsonb_build_object(
    'source',named_source,
    'requestState',r.state,
    'allocationState',a.state,
    'grantState',g.state,
    'poolGrantState',pg.state,
    'poolState',pc.state,
    'setupState',su.state,
    'planState',pl.state,
    'projectActive',pr.active,
    'roomActive',ro.active,
    'deviceRevoked',d.revoked,
    'deviceGenerationChanged',d.generation<>NEW.executor_generation,
    'sessionRevoked',s.revoked,
    'sessionEnded',s.valid_until_ms<=at_ms,
    'ownerActive',u.active,
    'startEnded',EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_runtime_commands c
      JOIN hagency_agent_v1.management_shared_assignments sa ON sa.id=c.assignment_id
      WHERE sa.request_id=NEW.request_id AND c.operation='agent.runtime.provision' AND c.expires_at_ms<=at_ms),
    'reserveEndedMsAgo',at_ms-reserve_ends_ms,
    'allocationEndedMsAgo',at_ms-a.expires_at_ms,
    'grantEndedMsAgo',at_ms-g.expires_at_ms,
    'poolGrantEndedMsAgo',at_ms-pg.expires_at_ms))::text
  FROM (SELECT 1) one
  LEFT JOIN hagency_agent_v1.management_agent_requests r ON r.id=NEW.request_id
  LEFT JOIN hagency_agent_v1.management_allocations a ON a.id=r.allocation_id
  LEFT JOIN hagency_agent_v1.management_resource_grants g ON g.id=a.grant_id
  LEFT JOIN hagency_agent_v1.management_resource_grants pg ON pg.id=r.pool_grant_id
  LEFT JOIN hagency_agent_v1.management_contributions pc ON pc.id=r.pool_contribution_id
  LEFT JOIN hagency_agent_v1.management_project_setups su ON su.id=r.setup_id
  LEFT JOIN hagency_agent_v1.management_projects pl ON pl.id=r.plan_id
  LEFT JOIN hagency_agent_v1.projects pr ON pr.id=r.domain_project_id
  LEFT JOIN hagency_agent_v1.rooms ro ON ro.room_id=r.room_id
  LEFT JOIN hagency_agent_v1.devices d ON d.id=NEW.executor_device_id AND d.user_id=NEW.executor_owner_user_id
  LEFT JOIN hagency_agent_v1.sessions s ON s.id=d.session_id
  LEFT JOIN hagency_agent_v1.users u ON u.id=NEW.executor_owner_user_id
  ON CONFLICT(intent_id) DO NOTHING;
 EXCEPTION WHEN OTHERS THEN NULL;
 END;
 RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION hagency_agent_v1.record_shared_runtime_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE at_ms bigint := (extract(epoch from clock_timestamp())*1000)::bigint;
BEGIN
 BEGIN
  INSERT INTO hagency_agent_v1.management_shared_runtime_fences(runtime_id,fenced_at_ms,cause,facts)
  SELECT NEW.id,at_ms,
   CASE WHEN pg_trigger_depth()>1 THEN 'dependency' ELSE 'service' END,
   jsonb_strip_nulls(jsonb_build_object(
    'stateBefore',OLD.state,
    'stopped',NEW.state='stopped',
    'leaseEndedMsAgo',CASE WHEN OLD.activity_until_ms>0 THEN at_ms-OLD.activity_until_ms END,
    'startWindowEndedMsAgo',CASE WHEN OLD.state IN('pending','issued') THEN at_ms-OLD.expires_at_ms END,
    'controlMode',c.mode,
    'assignmentState',a.state,
    'reservationFenced',i.state='fenced',
    'workNeverStarted',(SELECT count(*) FROM hagency_agent_v1.management_shared_work w
      WHERE w.runtime_id=NEW.id AND w.state='cancelled' AND w.started_at_ms IS NULL),
    'workUnknown',(SELECT count(*) FROM hagency_agent_v1.management_shared_work w
      WHERE w.runtime_id=NEW.id AND w.state='unknown')))::text
  FROM (SELECT 1) one
  LEFT JOIN hagency_agent_v1.management_shared_assignments a ON a.id=NEW.assignment_id
  LEFT JOIN hagency_agent_v1.management_shared_controls c ON c.assignment_id=NEW.assignment_id
  LEFT JOIN hagency_agent_v1.management_agent_intents i ON i.request_id=a.request_id
  ON CONFLICT(runtime_id) DO NOTHING;
 EXCEPTION WHEN OTHERS THEN NULL;
 END;
 RETURN NEW;
END;
$$;
