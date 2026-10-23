-- Collaboration data is committed with the same board revision as task changes.
ALTER TABLE hagency_project_v1.changes ADD COLUMN details jsonb NOT NULL DEFAULT '{}';
CREATE TABLE hagency_project_v1.notifications (
 project_id text NOT NULL, recipient text NOT NULL, seq bigint NOT NULL,
 task_id text NOT NULL, actor text NOT NULL, kind text NOT NULL,
 title text NOT NULL, details jsonb NOT NULL, occurred_at bigint NOT NULL, read_at bigint,
 PRIMARY KEY(project_id,recipient,seq),
 FOREIGN KEY(project_id,seq) REFERENCES hagency_project_v1.changes(project_id,seq),
 FOREIGN KEY(task_id,project_id) REFERENCES hagency_project_v1.tasks(id,project_id)
);
CREATE INDEX notifications_unread ON hagency_project_v1.notifications(project_id,recipient,seq DESC) WHERE read_at IS NULL;
CREATE TABLE hagency_project_v1.attachments (
 id text PRIMARY KEY, project_id text NOT NULL, task_id text NOT NULL,
 filename text NOT NULL, content_type text NOT NULL, data bytea NOT NULL CHECK(octet_length(data) BETWEEN 1 AND 2097152),
 created_by text NOT NULL, created_at bigint NOT NULL, removed_at bigint,
 FOREIGN KEY(task_id,project_id) REFERENCES hagency_project_v1.tasks(id,project_id)
);
CREATE INDEX attachments_task ON hagency_project_v1.attachments(project_id,task_id,created_at);
INSERT INTO hagency_project_v1.migrations(version) VALUES(3);
