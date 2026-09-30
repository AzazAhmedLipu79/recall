# 3. System Design

This document describes how Recall is built. It is the bridge between
the requirements (what) and the code (where).

## 3.1 High-level architecture

```
                  ┌─────────────────────────────────────┐
                  │          recallctl (CLI)            │  ← user-facing
                  └────────────┬──────────────┬─────────┘
                               │              │
            ┌──────────────────┘              └────────────────────┐
            ▼                                                       ▼
  ┌──────────────────┐                              ┌──────────────────────┐
  │ recall-importer  │                              │  recall-backup       │
  │                  │                              │  recall-verify       │
  │ folder / takeout │                              └──────────┬───────────┘
  │ takeout-zip      │                                         │
  └────┬───────┬─────┘                                         │
       │       │                                               │
       ▼       ▼                                               │
  ┌─────────┐ ┌──────────────┐   read bytes by sha256           │
  │recall-  │ │  recall-     │ ◀────────────────────────────────┤
  │metadata │ │  storage     │                                  │
  └────┬────┘ └──────┬───────┘                                  │
       │             │                                          │
       ▼             ▼                                          │
  ┌─────────────────────────┐                                   │
  │       recall-db         │  SQLite (WAL, FK on)              │
  └────────────┬────────────┘                                   │
               │                                                │
               ▼                                                │
       archive.db (catalog)        store/ (content-addressed)   │
                                                          ──────┘
```

**Key principle:** the catalog (SQLite) and the bytes (`store/`) are
both **derived** from the user's source material. Either one alone is
recoverable: the catalog can be rebuilt by re-hashing `store/`, and
the store can be rebuilt by re-importing source. There is no hidden
state.

## 3.2 Crate layout

A Cargo workspace at the repo root. Each crate has a single,
narrow responsibility.

| Crate             | Purpose                                                                       |
| ----------------- | ----------------------------------------------------------------------------- |
| `recall-core`     | Shared types: `Error`, `Result`, ID wrappers (`MediaId`, `ImportId`, …), `MediaKind`, `ArchivePath`, `RelativePath`. |
| `recall-hash`     | Streaming SHA-256 over `Read` and `File`.                                     |
| `recall-storage`  | Content-addressable `ObjectStore`: `import`, `import_reader`, `verify`, `iter_objects`. |
| `recall-db`       | SQLite migrations, `Catalog` (typed query layer), all schema.                  |
| `recall-metadata` | EXIF / video / QuickTime metadata extraction. Currently minimal; expanded per roadmap. |
| `recall-importer` | Source-specific ingest: `import_folder`, `import_takeout`, `import_takeout_zip`. |
| `recall-verify`   | Re-hash every object, write `verification_runs` + `verification_findings`.     |
| `recall-backup`   | Copy `objects/` + `metadata/` + `archive.db` to a secondary destination.       |
| `recallctl`       | The headless CLI binary. Subcommands map 1:1 onto the engine crates.          |

Dependency direction is strictly downward:

```
recallctl → importer / verify / backup / db / storage / core
recall-importer → metadata / storage / db / core
recall-verify → db / storage / hash / core
recall-backup → db / storage / core
recall-db → core
recall-storage → hash / core
recall-metadata → core
recall-hash → (no internal deps)
recall-core → (no internal deps)
```

No circular deps. Every crate is independently testable.

## 3.3 Storage layout

A Recall archive is a single directory the user picks:

```
archive-root/
├── archive.db            # SQLite catalog (WAL files alongside)
├── archive.db-wal
├── archive.db-shm
└── store/
    ├── objects/
    │   ├── .import-<pid>-<nanos>-<seq>.partial   # transient, cleaned up
    │   └── aa/bb/<full-sha256>                  # content-addressed files
    └── metadata/
        ├── <sha256>.json.partial                # transient
        └── <sha256>.json                        # Google sidecars etc.
```

### Why `objects/<aa>/<bb>/<sha256>`?

The first 4 hex chars (2 bytes → `aa/`) and next 4 hex chars (2 bytes
→ `bb/`) form a 2-level fan-out that keeps any single directory from
growing past ~65k files on a filesystem that struggles past 10k. The
full hash remains in the final component so two archives can be diffed
or merged with `rsync` trivially.

### Crash safety on writes

Every write goes:

1. Write to `*.partial` next to the destination.
2. `fsync` the partial file.
3. `rename(partial → final)`. POSIX rename is atomic on a single
   filesystem.

A crash between step 1 and step 3 leaves at most one orphan
`*.partial`, which is detected and discarded on the next import attempt
(`recall-storage::ObjectStore::cleanup_partial`).

### Originals never modified

The object store **reads** from the source. The source path is
recorded in `media_files.original_path` but the source bytes are
untouched. There is no in-place normalization, no rename, no
transcoding.

## 3.4 Catalog schema (current)

Migrations live in `crates/recall-db/src/migrations/`. They run
in-order on `recall_db::open`.

### `media`

One row per unique piece of content, keyed by sha256.

| Column              | Notes |
| ------------------- | ----- |
| `id`                | ULID-style id (TEXT) |
| `kind`              | `photo|video|document|audio|other` |
| `sha256`            | UNIQUE. The dedup key. |
| `size_bytes`        | size of the archived file |
| `mime_type`         | best-effort |
| `original_filename` | first filename seen |
| `archive_path`      | `objects/<aa>/<bb>/<sha256>` |
| `canonical_taken_at`| resolved timestamp (priority: EXIF > Google > filesystem) |
| `created_at`        | filesystem ctime |
| `modified_at`       | filesystem mtime |
| `source_label`      | `google_takeout|iphone|mac|...` |
| `inserted_at`       | when this row was created |

### `media_files`

Many-to-many of media to source paths. Provenance.

```sql
(media_id, original_path) PRIMARY KEY
```

### `media_metadata`

One row per media item. EXIF / container fields.

### `media_timestamps`

Multiple rows per media — one per source. We never silently overwrite
one source's timestamp with another.

```sql
(media_id, source) PRIMARY KEY
-- source ∈ exif|filesystem|google|quicktime|gpx|...
```

### `locations`

```sql
(media_id, source) PRIMARY KEY
```

### `albums` + `album_items`

Albums with `kind` ∈ `user|google_takeout|smart`. Membership preserves
order via `position`.

### `devices`

Per-source provenance (iPhone, iPad, Mac, Android).

### `imports` + `import_files` (added in `0003`)

Per-run summary + per-file outcomes. Lets the user answer "what
happened to this file?" forever.

### `verification_runs` + `verification_findings` (added in `0002`)

Run-level summary + per-file findings (`ok|missing|mismatched`).

### `backup_runs` (added in `0002`)

One row per backup sync with counts and status.

### `metadata_sidecars` (added in `0003`)

Pointer from a media row to its retained sidecar JSON.

### Indexes (`0004`)

`canonical_taken_at` for timeline, `source_label` for source
filtering, plus drilldown indexes on `import_files.outcome`,
`import_files.media_id`, `verification_findings.finding`,
`backup_runs.started_at`.

## 3.5 Ingest pipeline

The three import flows (folder, takeout, takeout-zip) all converge on
the same per-file pipeline:

```
discover files (walk tree or zip entries)
   ↓
pair_sidecars (if takeout) — match *.json with its media
   ↓
for each file:
   open → stream-hash → tee-write to store/objects/.import-*.partial
   ↓
   on success: rename → objects/aa/bb/<sha256>
   ↓
   catalog: upsert media row (sha256 UNIQUE → no double-insert)
   ↓
   catalog: insert media_files row (provenance)
   ↓
   catalog: insert media_metadata / media_timestamps / locations
   ↓
   catalog: insert metadata_sidecars row (if sidecar present)
   ↓
   catalog: if path under Albums/<name>/, upsert album + membership
   ↓
finalize: write imports row + per-file import_files rows
```

The `import_takeout_zip` variant extracts each zip entry to a temp
file in `TMPDIR` (preserving extension so `meta::extract` can infer
kind), runs the same pipeline, and cleans up the temp dir at the
end. The temp dir is process-id + timestamped to avoid collisions.

## 3.6 Verification pipeline

```
snapshot all media rows (avoid holding a borrow across long work)
   ↓
begin_verification: insert verification_runs row, status=running
   ↓
for each media:
   if object file missing → record Missing
   else: re-hash → if matches expected: Ok, else Mismatched
   ↓
update_verification: finalise run with counts, status=ok|problem
```

A crash mid-run still produces a finished `verification_runs` row
(the loop is linear and the final update is unconditional).

## 3.7 Backup pipeline

```
record_backup_run: row with status=running
   ↓
for each (archive_path, abs) in store.iter_objects():
   if dest already exists with same sha256: skip
   else: write to .partial, fsync, rename → Copied
   ↓
for each sidecar in store/metadata/:
   same logic
   ↓
update_backup_run: counts + final status=ok|partial
   ↓
copy_catalog_db: write archive.db to destination via temp+rename
```

Idempotent: re-running the same backup to the same destination copies
zero new files.

## 3.8 CLI surface (`recallctl`)

```
recallctl init
recallctl import-folder   --source <path> [--label <name>]
recallctl import-takeout  --source <path>
recallctl import-takeout-zip --source <path>
recallctl verify
recallctl backup          --destination <path>
recallctl dedup           [--prune]
recallctl status
recallctl imports         [--limit <n>]
```

Each subcommand opens the DB and store fresh, calls the corresponding
engine function, prints a one-line summary, and exits. No global
state, no daemon.

## 3.9 What's intentionally not here

- **No async runtime.** SQLite + filesystem I/O are fast enough that
  threads aren't required for the engine. If a future remote backup
  needs async, that's the only place we'll add `tokio`.
- **No ORMs.** All DB access is hand-written SQL in `recall-db::Catalog`
  with typed row structs.
- **No plugin system.** If a new source type is needed (Apple Photos
  library, LightRoom catalog), it's a new function in `recall-importer`
  with the same per-file pipeline underneath.
- **No background daemon.** Every operation is a discrete CLI
  invocation. The Tauri UI (future) will run those invocations on
  behalf of the user, but the engine remains stateless.
