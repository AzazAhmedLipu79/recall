# 2. System Requirements Specification (SRS)

## 2.1 Scope

Recall ingests media files from one or more sources, deduplicates them
by content, preserves original bytes and sidecar metadata, indexes them
in SQLite, and lets the user browse, search, verify, and back them up
— all from a single machine, with no required network access.

### 2.1.1 In scope

- Folder-based import (phone dumps, manual folders)
- Google Takeout import (extracted directory **or** raw `.zip`)
- Content-addressable storage with SHA-256 verification
- SQLite catalog of media, sidecars, albums, devices, locations
- Secondary-drive backup (local filesystem target)
- Per-import and per-file audit trail
- Headless CLI for all of the above
- (Future) Tauri-based GUI on top of the same engine

### 2.1.2 Out of scope

- Cloud sync, sharing, social features
- Photo / video editing
- Re-encoding, transcoding, "smart" compression
- Telemetry, analytics, or any required network call
- Multi-user access control (single-user, single-machine)
- Server-side rendering or remote processing

## 2.2 Actors

- **User** — owns the archive, runs the CLI or (later) GUI.
- **Source device / service** — produces the original files: a phone,
  a camera SD card, a Google Takeout export.
- **Destination drive** — receives the secondary backup copy. Same
  machine or an attached external drive.

## 2.3 Functional requirements

### FR-1 Ingest

- **FR-1.1 Folder import.** Given a directory tree, walk it, identify
  media files by extension, import each one. *(Implemented.)*
- **FR-1.2 Google Takeout folder import.** Given an extracted Takeout
  directory, pair each media file with its `<basename>.json` sidecar,
  extract structured metadata (timestamps, location, description),
  and preserve the sidecar verbatim. Derive albums from
  `Albums/<name>/...`. *(Implemented.)*
- **FR-1.3 Google Takeout ZIP import.** Given a `.zip` file, perform
  the same flow as FR-1.2 without requiring manual extraction.
  *(Implemented.)*
- **FR-1.4 Idempotent ingest.** Re-importing the same source must
  not duplicate `media` rows. Same sha256 → recorded as duplicate.
  *(Implemented.)*
- **FR-1.5 Crash-safe writes.** Writes go to a sibling `*.partial`
  file and are atomically renamed into place. *(Implemented.)*

### FR-2 Storage

- **FR-2.1 Content-addressable layout.** Stored at
  `objects/<aa>/<bb>/<full-sha256>` where `<aa>` and `<bb>` are the
  first 4 hex chars of the hash. *(Implemented.)*
- **FR-2.2 Sidecar retention.** Sidecar JSONs stored under
  `metadata/<sha256>.json`, linked via `metadata_sidecars`.
  *(Implemented.)*
- **FR-2.3 Original preservation.** Bytes on disk are byte-for-byte
  identical to source. *(Implemented — no re-encoding anywhere.)*

### FR-3 Catalog

- **FR-3.1 Per-media row.** Each unique piece of content gets one
  `media` row keyed by `sha256`. *(Implemented.)*
- **FR-3.2 Provenance.** A media item may be discovered at many source
  paths; each path is a `media_files` row. *(Implemented.)*
- **FR-3.3 Structured metadata.** Camera, lens, ISO, aperture, shutter,
  orientation, dimensions, duration, description extracted into
  `media_metadata`. *(Implemented for EXIF basics; broad coverage
  listed in roadmap.)*
- **FR-3.4 Source-of-truth timestamps.** Multiple timestamp sources
  (EXIF, filesystem, Google, QuickTime) stored separately in
  `media_timestamps` and resolved into `media.canonical_taken_at`.
  *(Implemented.)*
- **FR-3.5 Locations.** Multiple GPS readings per item, tagged by
  source, in `locations`. *(Schema implemented; ingestion partial.)*
- **FR-3.6 Albums.** Albums and membership in `albums` /
  `album_items`. Album kind: `user | google_takeout | smart`.
  *(Implemented.)*
- **FR-3.7 Devices.** Devices recorded in `devices` table for
  provenance. *(Schema implemented; ingestion pending.)*

### FR-4 Verification

- **FR-4.1 Re-hash.** Walk every `media` row, re-hash the object on
  disk, compare to recorded sha256. *(Implemented.)*
- **FR-4.2 Per-file findings.** Each checked file produces a
  `verification_findings` row tagged `ok | missing | mismatched`.
  *(Implemented.)*
- **FR-4.3 Run summary.** Run produces one `verification_runs` row
  with counts and overall status. *(Implemented.)*

### FR-5 Backup

- **FR-5.1 Replicate objects.** Copy every `objects/` file to a
  destination root, using sha256 to skip files already there.
  *(Implemented for local destination.)*
- **FR-5.2 Replicate sidecars.** Copy every `metadata/` file alongside.
  *(Implemented.)*
- **FR-5.3 Crash-safe destination.** Writes use temp + rename on the
  destination side as well. *(Implemented.)*
- **FR-5.4 Replicate catalog.** Copy `archive.db` to the destination.
  *(Implemented.)*
- **FR-5.5 Run history.** Each backup writes a `backup_runs` row.
  *(Implemented.)*

### FR-6 Audit trail

- **FR-6.1 Per-import row.** Every import run writes an `imports` row.
  *(Implemented.)*
- **FR-6.2 Per-file outcome.** Each file encountered during an import
  writes an `import_files` row with `imported | duplicate | failed`.
  *(Implemented.)*

### FR-7 Search & browse (CLI)

- **FR-7.1 Status.** Print total media, total bytes, and counts by
  kind. *(Implemented.)*
- **FR-7.2 Duplicates.** List media with multiple source paths (true
  duplicates by content). *(Implemented.)*
- **FR-7.3 Imports history.** List recent import runs.
  *(Implemented.)*
- **FR-7.4 Filtered search.** Search by date range, location,
  album, kind, source. *(Roadmap.)*
- **FR-7.5 Free-text search.** Search description, filename, sidecar
  fields. *(Roadmap.)*

### FR-8 GUI *(future)*

- **FR-8.1 Browse.** Timeline, albums, map, search. *(Roadmap —
  Tauri UI deferred.)*
- **FR-8.2 Live import progress.** Stream per-file events to the UI.
  *(Roadmap.)*

## 2.4 Non-functional requirements

- **NFR-1 Performance.** A multi-GB import must stream (constant RAM)
  and hash with at least 200 MB/s on a modern SSD.
  *(Implemented — 256 KB buffers, no full-file materialization.)*
- **NFR-2 Crash safety.** A power loss mid-import must leave the
  catalog in a consistent state: either the file is fully recorded
  and the bytes are in place, or neither. *(Implemented — temp +
  rename + per-import `imports` row finalised even on partial.)*
- **NFR-3 Storage efficiency.** Two copies of the same photo from a
  phone dump and a Takeout export must occupy one object's worth of
  disk. *(Implemented — content-addressed storage + UNIQUE on
  `media.sha256`.)*
- **NFR-4 No network dependency.** Core features must work offline.
  *(Implemented.)*
- **NFR-5 Portability.** Archive root is a normal directory: db file
  + `objects/` + `metadata/`. Trivial to copy, back up, or inspect
  with `sqlite3`. *(Implemented.)*
- **NFR-6 Test coverage.** Every public engine function has unit
  tests; the verification / backup / import pipelines have
  integration tests that use real files. *(Currently: 46 tests
  passing across 9 crates.)*
- **NFR-7 No unsafe.** `#![forbid(unsafe_code)]` in every crate.
  *(Implemented.)*

## 2.5 Constraints

- **C-1 Stack.** Rust 2021+ workspace, SQLite (rusqlite), Tauri (later)
  for UI. No exotic runtimes.
- **C-2 Targets.** macOS first, Linux second. (No Windows-specific
  work yet.)
- **C-3 No external services.** All storage is local. Future cloud
  backup is a remote destination on the same engine.
