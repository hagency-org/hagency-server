-- Retain immutable attachment identity/history without retaining deleted blobs.
ALTER TABLE hagency_project_v1.attachments
 ADD COLUMN byte_size bigint,
 ADD COLUMN content_sha256 text;
UPDATE hagency_project_v1.attachments
 SET byte_size=octet_length(data), content_sha256=encode(sha256(data),'hex');
ALTER TABLE hagency_project_v1.attachments
 ALTER COLUMN byte_size SET NOT NULL,
 ALTER COLUMN content_sha256 SET NOT NULL,
 ADD CONSTRAINT attachments_byte_size_valid CHECK(byte_size BETWEEN 1 AND 2097152),
 ADD CONSTRAINT attachments_digest_valid CHECK(content_sha256 ~ '^[0-9a-f]{64}$'),
 DROP CONSTRAINT attachments_data_check,
 ALTER COLUMN data DROP NOT NULL;
UPDATE hagency_project_v1.attachments SET data=NULL WHERE removed_at IS NOT NULL;
ALTER TABLE hagency_project_v1.attachments
 ADD CONSTRAINT attachments_payload_valid CHECK(
  (removed_at IS NULL AND data IS NOT NULL AND octet_length(data)=byte_size)
  OR (removed_at IS NOT NULL AND data IS NULL)
 );
CREATE INDEX attachments_active_task
 ON hagency_project_v1.attachments(project_id,task_id,created_at,id) WHERE removed_at IS NULL;
INSERT INTO hagency_project_v1.migrations(version) VALUES(7);
