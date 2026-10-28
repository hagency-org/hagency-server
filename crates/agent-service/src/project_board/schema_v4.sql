-- Archived lists keep their tasks, ranks and individual archive states for restoration.
ALTER TABLE hagency_project_v1.columns ADD COLUMN archived boolean NOT NULL DEFAULT false;
INSERT INTO hagency_project_v1.migrations(version) VALUES (4);
