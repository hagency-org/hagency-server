-- Runs on every start (idempotent).
-- One registration per owner project (ADR 0001): rooms created with the older
-- single-column key move to the (room, project) key; no foreign key uses the old one.
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid='hagency_agent_v1.rooms'::regclass AND contype='p' AND array_length(conkey,1)=1) THEN
  ALTER TABLE hagency_agent_v1.rooms DROP CONSTRAINT rooms_pkey;
  ALTER TABLE hagency_agent_v1.rooms ADD CONSTRAINT rooms_pkey PRIMARY KEY(room_id,project_id);
 END IF;
END $$;
