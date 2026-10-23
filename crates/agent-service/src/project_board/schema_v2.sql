-- Additive upgrade; existing task IDs, versions and immutable receipts survive.
CREATE TABLE hagency_project_v1.columns (
 project_id text NOT NULL REFERENCES hagency_project_v1.boards(project_id),
 column_key text NOT NULL, title text NOT NULL CHECK(length(title) BETWEEN 1 AND 48),
 position integer NOT NULL, wip_limit bigint CHECK(wip_limit BETWEEN 1 AND 10000),
 is_done boolean NOT NULL, PRIMARY KEY(project_id,column_key)
);
INSERT INTO hagency_project_v1.columns(project_id,column_key,title,position,is_done)
 SELECT b.project_id,c.key,c.title,c.pos,c.key='done'
 FROM hagency_project_v1.boards b CROSS JOIN (VALUES
 ('todo','To do',0),('in_progress','In progress',1),('blocked','Blocked',2),
 ('in_review','In review',3),('done','Done',4)) c(key,title,pos);
ALTER TABLE hagency_project_v1.tasks DROP CONSTRAINT tasks_state_key_check;
ALTER TABLE hagency_project_v1.tasks ADD CONSTRAINT tasks_column_fk
 FOREIGN KEY(project_id,state_key) REFERENCES hagency_project_v1.columns(project_id,column_key);
ALTER TABLE hagency_project_v1.tasks ADD COLUMN parent_id text;
ALTER TABLE hagency_project_v1.tasks ADD CONSTRAINT tasks_parent_fk
 FOREIGN KEY(parent_id,project_id) REFERENCES hagency_project_v1.tasks(id,project_id);
ALTER TABLE hagency_project_v1.tasks ADD CONSTRAINT tasks_parent_not_self CHECK(parent_id<>id);
ALTER TABLE hagency_project_v1.tasks ADD COLUMN checklist jsonb NOT NULL DEFAULT '[]'
 CHECK(jsonb_typeof(checklist)='array' AND jsonb_array_length(checklist)<=100);
CREATE INDEX tasks_parent ON hagency_project_v1.tasks(project_id,parent_id);
ALTER TABLE hagency_project_v1.changes ALTER COLUMN task_id DROP NOT NULL;
INSERT INTO hagency_project_v1.migrations(version) VALUES(2);
