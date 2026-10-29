-- project_id is the original board scope key, retained for task/receipt compatibility.
ALTER TABLE hagency_project_v1.boards ADD COLUMN owner_project_id text REFERENCES hagency_agent_v1.projects(id);
UPDATE hagency_project_v1.boards SET owner_project_id=project_id;
ALTER TABLE hagency_project_v1.boards ALTER COLUMN owner_project_id SET NOT NULL;
ALTER TABLE hagency_project_v1.boards DROP CONSTRAINT boards_project_id_fkey;
ALTER TABLE hagency_project_v1.boards ADD COLUMN name text NOT NULL DEFAULT 'Main Board' CHECK(length(name) BETWEEN 1 AND 80);
ALTER TABLE hagency_project_v1.boards ADD COLUMN creation_key text;
ALTER TABLE hagency_project_v1.boards ADD COLUMN created_by text;
CREATE UNIQUE INDEX boards_creation ON hagency_project_v1.boards(owner_project_id,created_by,creation_key);
ALTER TABLE hagency_agent_v1.projects ADD COLUMN archived boolean NOT NULL DEFAULT false;
INSERT INTO hagency_project_v1.migrations(version) VALUES(5);
