-- One shared default of each kind per Project, with scope-safe references.
ALTER TABLE hagency_project_v1.boards ADD CONSTRAINT boards_project_scope UNIQUE(project_id,owner_project_id);
CREATE TABLE hagency_project_v1.navigation_defaults (
 project_id text PRIMARY KEY REFERENCES hagency_agent_v1.projects(id),
 board_id text, room_id text, revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0),
 FOREIGN KEY(board_id,project_id) REFERENCES hagency_project_v1.boards(project_id,owner_project_id),
 FOREIGN KEY(room_id,project_id) REFERENCES hagency_agent_v1.rooms(room_id,project_id)
);
INSERT INTO hagency_project_v1.migrations(version) VALUES(6);
