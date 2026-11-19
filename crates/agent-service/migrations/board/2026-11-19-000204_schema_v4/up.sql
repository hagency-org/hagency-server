-- Adopt pre-Diesel installations at their existing schema version.
DO $hagency_migration$
BEGIN
IF (SELECT max(version) FROM hagency_project_v1.migrations) < 4 THEN
-- Archived lists keep their tasks, ranks and individual archive states for restoration.
ALTER TABLE hagency_project_v1.columns ADD COLUMN archived boolean NOT NULL DEFAULT false;
INSERT INTO hagency_project_v1.migrations(version) VALUES (4);
END IF;
END $hagency_migration$;
