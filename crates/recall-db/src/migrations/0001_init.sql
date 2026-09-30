-- 0001_init.sql: initial schema for the archive catalog.
-- The schema is the catalog, not the source of truth for bytes.
-- Bytes live in the object store keyed by sha256.

CREATE TABLE media (
    id                  TEXT PRIMARY KEY,
    kind                TEXT NOT NULL,           -- photo|video|document|audio|other
    sha256              TEXT NOT NULL UNIQUE,    -- content hash; dupes collapse here
    size_bytes          INTEGER NOT NULL,
    mime_type           TEXT,
    original_filename   TEXT,
    archive_path        TEXT NOT NULL,           -- objects/aa/bb/<sha256>
    canonical_taken_at  INTEGER,                 -- unix seconds, used for timeline
    created_at          INTEGER,                 -- filesystem ctime
    modified_at         INTEGER,                 -- filesystem mtime
    source_label        TEXT NOT NULL,           -- google_takeout|iphone|mac|...
    inserted_at         INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

-- A media item can be discovered multiple times from different source paths.
-- This preserves provenance without duplicating the actual file.
CREATE TABLE media_files (
    media_id        TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    original_path   TEXT NOT NULL,
    device_id       TEXT,
    import_id       TEXT,
    size_bytes      INTEGER NOT NULL,
    modified_at     INTEGER,
    PRIMARY KEY (media_id, original_path)
);

-- Structured metadata extracted from the file (EXIF, container, etc.).
CREATE TABLE media_metadata (
    media_id          TEXT PRIMARY KEY REFERENCES media(id) ON DELETE CASCADE,
    camera_make       TEXT,
    camera_model      TEXT,
    lens              TEXT,
    focal_length      REAL,
    iso               INTEGER,
    aperture          REAL,
    shutter_speed     TEXT,           -- stored as text because of "1/200" format
    orientation       INTEGER,
    width             INTEGER,
    height            INTEGER,
    duration_seconds  REAL,
    description       TEXT
);

-- Source-specific timestamps so we never silently overwrite one with another.
-- See PRD §12 — metadata conflict resolution.
CREATE TABLE media_timestamps (
    media_id  TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    source    TEXT NOT NULL,           -- exif|filesystem|google|quicktime|...
    value     INTEGER NOT NULL,        -- unix seconds
    label     TEXT,                    -- human-readable label (optional)
    PRIMARY KEY (media_id, source)
);

-- A media item may have multiple GPS coordinates (different sources).
CREATE TABLE locations (
    media_id   TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    latitude   REAL NOT NULL,
    longitude  REAL NOT NULL,
    altitude   REAL,
    source     TEXT NOT NULL,           -- exif|google|gpx|...
    PRIMARY KEY (media_id, source)
);

CREATE TABLE albums (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,          -- user|google_takeout|smart
    created_at  INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_albums_name_kind ON albums(name, kind);

CREATE TABLE album_items (
    album_id  TEXT NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    media_id  TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    position  INTEGER NOT NULL,
    PRIMARY KEY (album_id, media_id)
);

CREATE TABLE devices (
    id           TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    kind         TEXT NOT NULL,         -- iphone|ipad|mac|android|...
    first_seen   INTEGER NOT NULL,
    last_seen    INTEGER NOT NULL
);
