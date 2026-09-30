-- 0002_verification.sql: history of archive verification runs.

CREATE TABLE verification_runs (
    id                TEXT PRIMARY KEY,
    started_at        INTEGER NOT NULL,
    finished_at       INTEGER,
    files_checked     INTEGER NOT NULL DEFAULT 0,
    files_ok          INTEGER NOT NULL DEFAULT 0,
    files_missing     INTEGER NOT NULL DEFAULT 0,
    files_mismatched  INTEGER NOT NULL DEFAULT 0,
    status            TEXT NOT NULL      -- running|ok|problem
);

-- Each verification run produces one row per checked file.
-- We keep these around so the user can drill into a previous problem.
CREATE TABLE verification_findings (
    run_id         TEXT NOT NULL REFERENCES verification_runs(id) ON DELETE CASCADE,
    media_id       TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    finding        TEXT NOT NULL,      -- ok|missing|mismatched
    expected_hash  TEXT NOT NULL,
    actual_hash    TEXT,
    PRIMARY KEY (run_id, media_id)
);

-- History of secondary backup syncs.
CREATE TABLE backup_runs (
    id                TEXT PRIMARY KEY,
    destination_path  TEXT NOT NULL,
    started_at        INTEGER NOT NULL,
    finished_at       INTEGER,
    bytes_copied      INTEGER NOT NULL DEFAULT 0,
    files_copied      INTEGER NOT NULL DEFAULT 0,
    files_skipped     INTEGER NOT NULL DEFAULT 0,
    files_failed      INTEGER NOT NULL DEFAULT 0,
    status            TEXT NOT NULL     -- running|success|partial|failed
);
