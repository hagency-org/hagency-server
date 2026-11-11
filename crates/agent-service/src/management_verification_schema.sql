-- A device acknowledgement proves the admitted connection, not an allocation,
-- provider execution, or a reservation. Old receipts never renew its freshness.
CREATE TABLE hagency_agent_v1.management_verifications (
 contribution_id text PRIMARY KEY REFERENCES hagency_agent_v1.management_contributions(id),
 contribution_revision bigint NOT NULL CHECK(contribution_revision>0),
 executor_generation bigint NOT NULL CHECK(executor_generation>0),
 resource_revision text NOT NULL CHECK(length(resource_revision)=64),
 verified_at_ms bigint NOT NULL,
 fresh_until_ms bigint NOT NULL CHECK(fresh_until_ms>verified_at_ms)
);
ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;
UPDATE hagency_agent_v1.management_deployment SET version=2;
ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=2);
