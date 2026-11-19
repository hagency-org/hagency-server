-- Opt-in working folder of an Agent space: every coding Agent that one
-- hosting computer runs in any Room of the space works in this one folder.
-- A project without a row keeps one folder per Room. Additive only and run on
-- every start without a management_deployment version: an older release
-- ignores the table, so rolling back needs no SQL.
CREATE TABLE IF NOT EXISTS hagency_agent_v1.project_folders (
 project_id text PRIMARY KEY REFERENCES hagency_agent_v1.projects(id),
 folder text NOT NULL CHECK(char_length(folder) BETWEEN 1 AND 64),
 set_by_user_id text NOT NULL REFERENCES hagency_agent_v1.users(id),
 created_at_ms bigint NOT NULL
);
