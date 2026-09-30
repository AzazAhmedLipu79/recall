-- 0003_import_files.sql: per-file outcome for each import.
--
-- One row per (import, source_path). Records the outcome and, when the file
-- was archived, links back to the media row. This makes the import report
-- truly auditable: "what happened to this file?" is always answerable.

CREATE TABLE imports (
    id            TEXT PRIMARY KEY,
    source_kind   TEXT NOT NULL,    -- google_takeout|folder|iphone|manual
    source_path   TEXT NOT NULL,
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER,
    discovered    INTEGER NOT NULL DEFAULT 0,
    imported      INTEGER NOT NULL DEFAULT 0,
    duplicates    INTEGER NOT NULL DEFAULT 0,
    failed        INTEGER NOT NULL DEFAULT 0,
    status        TEXT NOT NULL     -- running|partial|success|failed
);

CREATE TABLE import_files (
    import_id     TEXT NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    source_path   TEXT NOT NULL,
    media_id      TEXT REFERENCES media(id) ON DELETE SET NULL,
    outcome       TEXT NOT NULL,    -- imported|duplicate|failed
    error         TEXT,
    PRIMARY KEY (import_id, source_path)
);

-- Sidecars: pointer from a media row to a retained JSON sidecar (Google etc.).
CREATE TABLE metadata_sidecars (
    media_id   TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    sidecar_path TEXT NOT NULL,     -- path under archive/metadata/<hash>.json
    kind       TEXT NOT NULL,       -- google_photos|...
    PRIMARY KEY (media_id, sidecar_path)
);
