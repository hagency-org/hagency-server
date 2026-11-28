-- The first activity lease of a runtime keeps its end as every renewal does:
-- in a row of this table, from epoch 0 to epoch 1. A fence clears the
-- runtime's current end, and the sweep reads the ends kept here. Until now
-- the first end was not kept, and the sweep took thirty seconds after the
-- start window for it; a lease that follows a longer authorization window
-- does not fit that. Rows from epoch 1 on are unchanged, and an earlier
-- server reads this table only for its latest end.
-- Every statement can run again over its own result.
DO $$
DECLARE c record;
BEGIN
 FOR c IN SELECT conname FROM pg_constraint
  WHERE contype='c' AND conrelid='hagency_agent_v1.management_shared_activity_renewals'::regclass
   AND pg_get_constraintdef(oid) ~ 'from_epoch >= '
 LOOP
  EXECUTE format('ALTER TABLE hagency_agent_v1.management_shared_activity_renewals DROP CONSTRAINT %I',c.conname);
 END LOOP;
END;
$$;
ALTER TABLE hagency_agent_v1.management_shared_activity_renewals ADD CONSTRAINT management_shared_activity_renewals_from_epoch_check CHECK(from_epoch BETWEEN 0 AND 9007199254740990);
