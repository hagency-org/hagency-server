-- The record of a fence the service made said `service` and no more: not
-- which part of the service fenced, and nothing about time, since an
-- allocation or a grant keeps its state when its time is over. So the record
-- of a reservation whose time was over looked like that of any other fence
-- the service made. It now says both.
--
-- `source` names the part of the service that fenced. The statement that
-- fences names it in the setting `hagency.fence_source`, as
-- `<source>@<its statement_timestamp()>`, so only the records that statement
-- writes read it, and a name never passes to a later fence in the same
-- transaction. A fence by a rule of the database (`dependency`), or by a
-- service that names nothing (one from before this migration), has none.
--
-- `reserveEndedMsAgo`, `allocationEndedMsAgo`, `grantEndedMsAgo` and
-- `poolGrantEndedMsAgo` are how long before the fence the reserve command,
-- the allocation, the allocation's grant and the pool grant had ended. A
-- negative value means that time was still ahead, so it was not the reason.
--
-- The record still never stands in the way of the fence it records, and the
-- records already written stay as they are. Every statement here can run
-- again on a database that already has it.
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
