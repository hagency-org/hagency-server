-- A fence withdraws an Agent's reservation for good, and it left no trace of
-- when or why: the reservation only says `fenced`. Every fence now leaves one
-- row. It is written where the state changes, so it covers every path that
-- fences, in this service and in the database, and no path has to remember it.
--
-- `cause` is `dependency` when the fence came from a change to a row the
-- reservation depends on (a revoked session or device, a closed grant, a
-- withdrawn pool, ...), and `service` when the service decided it (a start
-- that ran out of time, a cancelled request, a reservation its executor no
-- longer holds). `facts` is what those rows looked like at that moment: the
-- one that is not as it should be is the reason.
--
-- The record refers to its reservation by ID and holds no reference to it:
-- it is evidence, and must not decide what may happen to the reservation.
-- Every statement here can run again on a database that already has it.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.management_agent_fences (
 intent_id text PRIMARY KEY,
 fenced_at_ms bigint NOT NULL CHECK(fenced_at_ms>0),
 cause text NOT NULL CHECK(cause IN('dependency','service')),
 facts text NOT NULL
);
CREATE OR REPLACE FUNCTION hagency_agent_v1.record_agent_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE at_ms bigint := (extract(epoch from clock_timestamp())*1000)::bigint;
BEGIN
 -- The record never stands in the way of the fence it records.
 BEGIN
  INSERT INTO hagency_agent_v1.management_agent_fences(intent_id,fenced_at_ms,cause,facts)
  SELECT NEW.id,at_ms,
   CASE WHEN pg_trigger_depth()>1 THEN 'dependency' ELSE 'service' END,
   jsonb_strip_nulls(jsonb_build_object(
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
      WHERE sa.request_id=NEW.request_id AND c.operation='agent.runtime.provision' AND c.expires_at_ms<=at_ms)))::text
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
DROP TRIGGER IF EXISTS record_agent_fence ON hagency_agent_v1.management_agent_intents;
CREATE TRIGGER record_agent_fence AFTER INSERT OR UPDATE OF state ON hagency_agent_v1.management_agent_intents
 FOR EACH ROW WHEN (NEW.state='fenced') EXECUTE FUNCTION hagency_agent_v1.record_agent_fence();

-- The same for a process. A fenced process is not used again: its started
-- work becomes unknown and its unstarted work is cancelled. Until now nothing
-- said whether its lease ran out, its start did, someone stopped the Agent,
-- the time bound gave it up, or something it stands on ended. `facts` has
-- what the process and its Agent looked like at that moment.
-- `leaseEndedMsAgo` is negative when the lease still had time, which means
-- the lease was not the reason.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.management_shared_runtime_fences (
 runtime_id text PRIMARY KEY,
 fenced_at_ms bigint NOT NULL CHECK(fenced_at_ms>0),
 cause text NOT NULL CHECK(cause IN('dependency','service')),
 facts text NOT NULL
);
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
DROP TRIGGER IF EXISTS record_shared_runtime_fence ON hagency_agent_v1.management_shared_runtimes;
CREATE TRIGGER record_shared_runtime_fence AFTER UPDATE OF permanent_fence ON hagency_agent_v1.management_shared_runtimes
 FOR EACH ROW WHEN (NEW.permanent_fence AND NOT OLD.permanent_fence) EXECUTE FUNCTION hagency_agent_v1.record_shared_runtime_fence();
