CREATE SCHEMA IF NOT EXISTS hagency_project_v1;
CREATE TABLE IF NOT EXISTS hagency_project_v1.migrations(version integer PRIMARY KEY);
CREATE TABLE IF NOT EXISTS hagency_project_v1.boards (
 project_id text PRIMARY KEY REFERENCES hagency_agent_v1.projects(id),
 change_seq bigint NOT NULL DEFAULT 0 CHECK(change_seq>=0),
 next_number bigint NOT NULL DEFAULT 1 CHECK(next_number>0)
);
CREATE TABLE IF NOT EXISTS hagency_project_v1.tasks (
 id text PRIMARY KEY, project_id text NOT NULL REFERENCES hagency_project_v1.boards(project_id),
 number bigint NOT NULL CHECK(number>0), title text NOT NULL CHECK(length(title) BETWEEN 1 AND 240),
 description text NOT NULL CHECK(octet_length(description)<=32768),
 state_key text NOT NULL CHECK(state_key IN ('todo','in_progress','blocked','in_review','done')),
 rank bigint NOT NULL CHECK(rank>0), priority text NOT NULL CHECK(priority IN ('low','normal','high','urgent')),
 assignee_mxid text, labels jsonb NOT NULL CHECK(jsonb_typeof(labels)='array'), due_date date,
 version bigint NOT NULL DEFAULT 1 CHECK(version>0), created_by text NOT NULL,
 created_at bigint NOT NULL, updated_at bigint NOT NULL, archived_at bigint,
 UNIQUE(project_id,number), UNIQUE(id,project_id)
);
CREATE INDEX IF NOT EXISTS tasks_lane ON hagency_project_v1.tasks(project_id,state_key,rank,id) WHERE archived_at IS NULL;
CREATE INDEX IF NOT EXISTS tasks_archive ON hagency_project_v1.tasks(project_id,archived_at,id);
CREATE INDEX IF NOT EXISTS tasks_assignee ON hagency_project_v1.tasks(project_id,assignee_mxid);
CREATE TABLE IF NOT EXISTS hagency_project_v1.comments (
 id text PRIMARY KEY, project_id text NOT NULL, task_id text NOT NULL,
 author text NOT NULL, body text NOT NULL CHECK(length(body)>0 AND octet_length(body)<=8192),
 created_at bigint NOT NULL,
 FOREIGN KEY(task_id,project_id) REFERENCES hagency_project_v1.tasks(id,project_id)
);
ALTER TABLE hagency_project_v1.comments ADD COLUMN IF NOT EXISTS seq bigint GENERATED ALWAYS AS IDENTITY;
CREATE INDEX IF NOT EXISTS comments_task_seq ON hagency_project_v1.comments(project_id,task_id,seq);
CREATE TABLE IF NOT EXISTS hagency_project_v1.changes (
 project_id text NOT NULL REFERENCES hagency_project_v1.boards(project_id),
 seq bigint NOT NULL CHECK(seq>0), task_id text NOT NULL, actor text NOT NULL,
 kind text NOT NULL, occurred_at bigint NOT NULL, PRIMARY KEY(project_id,seq),
 FOREIGN KEY(task_id,project_id) REFERENCES hagency_project_v1.tasks(id,project_id)
);
CREATE INDEX IF NOT EXISTS changes_task ON hagency_project_v1.changes(project_id,task_id,seq);
CREATE TABLE IF NOT EXISTS hagency_project_v1.receipts (
 project_id text NOT NULL REFERENCES hagency_project_v1.boards(project_id),
 actor text NOT NULL, intent_key text NOT NULL, digest text NOT NULL, result jsonb NOT NULL,
 PRIMARY KEY(project_id,actor,intent_key)
);
INSERT INTO hagency_project_v1.migrations(version) VALUES(1) ON CONFLICT DO NOTHING;
