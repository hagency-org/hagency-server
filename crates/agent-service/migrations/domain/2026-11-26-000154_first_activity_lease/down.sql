-- Rows from epoch 0 stay: this table keeps its history. The earlier rule is
-- put back for new rows only.
ALTER TABLE hagency_agent_v1.management_shared_activity_renewals DROP CONSTRAINT IF EXISTS management_shared_activity_renewals_from_epoch_check;
ALTER TABLE hagency_agent_v1.management_shared_activity_renewals ADD CONSTRAINT management_shared_activity_renewals_from_epoch_check CHECK(from_epoch BETWEEN 1 AND 9007199254740990) NOT VALID;
