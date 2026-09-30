-- 0004_indexes.sql: indexes supporting search, timeline and sync.

-- Timeline: month-grouped scans and per-day drilldowns.
CREATE INDEX idx_media_taken_at ON media(canonical_taken_at);

-- Source filtering (every UI section has a "by source" view).
CREATE INDEX idx_media_source ON media(source_label);

-- Album membership lookups.
CREATE INDEX idx_album_items_media ON album_items(media_id);

-- Import drilldowns.
CREATE INDEX idx_import_files_outcome ON import_files(outcome);
CREATE INDEX idx_import_files_media ON import_files(media_id);

-- Verification drilldowns.
CREATE INDEX idx_verif_findings_finding ON verification_findings(finding);

-- Backup history.
CREATE INDEX idx_backup_runs_started ON backup_runs(started_at);
