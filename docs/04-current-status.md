# 4. Current Status

What is actually built and tested today.

## 4.1 Crate inventory

All 9 crates in the workspace compile and pass tests:

```
$ cargo test --workspace
test result: ok. 6 passed   # recall-backup
test result: ok. 15 passed  # recall-db
test result: ok. 4 passed   # recall-hash
test result: ok. 15 passed  # recall-importer
test result: ok. 8 passed   # recall-metadata
test result: ok. 7 passed   # recall-storage
test result: ok. 4 passed   # recall-verify
test result: ok. 0 passed   # recallctl (binary, no tests)
test result: ok. 0 passed   # recall-core (types only — 1 doc ignored)
```

**Total: 60 tests passing.** Build is clean under
`cargo build --workspace`. No unsafe code (`#![forbid(unsafe_code)]` in
every crate).

## 4.2 Implemented features (with command + tests)

### ✅ Init / archive creation

```
$ recallctl init
```

Creates `archive.db` (runs all migrations) and `store/objects/` +
`store/metadata/`. Idempotent — running on an existing archive is a
no-op.

### ✅ Folder import

```
$ recallctl import-folder --source ~/PhoneDump/DCIM [--label "iPhone trip"]
```

Walks the tree, imports by extension allowlist
(`MEDIA_EXTS` in `recall-importer`), preserves directory structure in
`media_files.original_path`.

**Tests:** `import_folder_discovers_media`, `import_folder_dedupes_by_content`, etc.

### ✅ Google Takeout folder import

```
$ recallctl import-takeout --source ~/Takeout
```

Pairs each media file with `<basename>.json`, extracts structured
metadata (title, description, photoTakenTime.timestamp, etc.) into
`media_metadata` and `media_timestamps`, derives album from
`Albums/<name>/...`.

**Tests:** `import_takeout_pairs_sidecar`, `import_takeout_derives_album_from_path`, etc.

### ✅ Google Takeout ZIP import

```
$ recallctl import-takeout-zip --source ~/Takeout.zip
```

Same flow as folder import, but reads from a `.zip` without requiring
manual extraction. Auto-strips a common top-level wrapper directory
(e.g. `Takeout/`). Each entry is extracted to a temp file with its
original extension preserved.

**Tests (4 new):**
- `zip_import_streams_entries_and_records_paths`
- `zip_import_parses_google_sidecar_in_zip`
- `zip_import_strips_common_top_level_prefix`
- `zip_import_is_idempotent`

### ✅ Verify

```
$ recallctl verify
verify: checked=N ok=N missing=0 mismatched=0
```

Re-hashes every object, writes per-file findings + a run summary row.
Exit code `2` if any problems found.

**Tests:** `verify_clean_archive_reports_ok`, `verify_detects_missing_file`, `verify_detects_hash_mismatch`.

### ✅ Backup

```
$ recallctl backup --destination /Volumes/External/recall-backup
backup: copied=N skipped=N failed=0 bytes=N
```

Replicates `objects/` + `metadata/` + `archive.db` to a destination.
Idempotent: re-running with no changes copies zero files.

**Tests:** `backup_copies_all_objects`, `backup_is_idempotent`, `copy_catalog_db_round_trips`.

### ✅ Dedup

```
$ recallctl dedup              # list media with multiple source paths
$ recallctl dedup --prune      # also remove media_files rows pointing at missing paths
```

**Tests:** in `recall-db`.

### ✅ Status

```
$ recallctl status
Total media: N
Total bytes: N
By kind: photos=N videos=N documents=N audio=N other=N
```

### ✅ Imports history

```
$ recallctl imports [--limit N]
```

Lists recent `imports` rows.

### ✅ Search & query

```
$ recallctl search \
    --kind photo \
    --from 2024-01-01 --to 2024-12-31 \
    --album Vacation \
    --has-location \
    --near-lat 37.0 --near-lon -122.0 --radius-km 50 \
    --text "sunset" \
    --limit 50
```

Composite filter (kind + source + album + bbox + text + date range)
backed by a SQL builder in `recall-db::SearchFilter::search_media`.

**Tests (6 new):** `search_by_kind`, `search_by_source`,
`search_by_text`, `search_by_date_range`, `search_by_album`,
`search_by_location_bbox`.

### ✅ Live progress events

Every long-running command (`import-folder`, `import-takeout`,
`import-takeout-zip`, `verify`, `backup`) threads a `&dyn Progress`
through and emits structured `ProgressEvent`s. With `--json` they are
written as NDJSON to stdout so UIs can stream:

```json
{"started":{"kind":"import","total":42}}
{"file_imported":{"path":"…","sha256":"…"}}
{"finished":{"kind":"import","ok":40,"failed":1,"skipped":1}}
```

The `Progress` trait is dyn-compatible (`fn on_event(&self,
&ProgressEvent)`) so the CLI and any future Tauri/IPC listener can
swap implementations.

### ✅ Rich metadata extraction (expanded EXIF slice)

Beyond `DateTimeOriginal` and GPS, we now read:
- `DateTime` (tag 0x0132) — fallback for non-PHOTO files
- `LensMake` (0xA433), `LensModel` (0xA434)
- `UserComment` (0x9286) — used to populate `description`

The GPS IFD parser was rewritten to read the IFD value field correctly
(was reading the count field — silent bug for all v1 files).

### ✅ Backup destination abstraction

`recall_backup::BackupDestination` is a small, dyn-compatible trait
(`same_content`, `put`, `get`). `LocalDestination` is the first impl.
The CLI accepts `file://` URLs (mapped to `LocalDestination`) and a
plain path; remote destinations (S3/B2/R2) parse the URL but
gracefully refuse with a clear message until Phase R4-remote ships.

## 4.3 Schema status

All four migrations implemented:

| Migration                | Status |
| ------------------------ | ------ |
| `0001_init.sql`          | ✅ — media, media_files, media_metadata, media_timestamps, locations, albums, album_items, devices |
| `0002_verification.sql`  | ✅ — verification_runs, verification_findings, backup_runs |
| `0003_import_files.sql`  | ✅ — imports, import_files, metadata_sidecars |
| `0004_indexes.sql`       | ✅ — timeline, source, album, outcome, finding, backup indexes |

## 4.4 Storage guarantees verified

- Content-addressable layout (`objects/<aa>/<bb>/<sha256>`) — tests in
  `recall-storage::tests`.
- Idempotent import — `import_is_idempotent`.
- Crash-safe writes — implicit in `import_large_streams` and
  `cleanup_partial` tests; `.partial` orphan cleanup.
- Original bytes preserved byte-for-byte — `import_places_file_by_hash`
  reads back the exact bytes.
- Large files stream — `import_large_streams` (8 MB at 256 KB
  buffers).

## 4.5 Verified CLI flows (end-to-end smoke)

A reproducible end-to-end run was executed during the last phase:

1. `recallctl init` — created archive.
2. `recallctl import-takeout-zip` — imported 3 photos from a synthetic
   Takeout ZIP (2 with sidecars + 1 plain).
3. `recallctl status` — 3 photos, 74 bytes.
4. `recallctl verify` — 3/3 ok.
5. Album "Vacation" (kind=google_takeout) created with 2 members —
   correctly derived from `Albums/Vacation/...`.
6. Re-import of same ZIP — `imported=0, duplicates=3, failed=0`. The
   catalog shows `Total media: 3` (no growth).

Smoke artifacts under `scripts/smoke_make_zip.py`.

## 4.6 Tauri UI scaffold

A new crate `recall-app` exists and compiles cleanly. It is a Tauri
2 host process wrapping the existing engine:

- **Engine commands as IPC**: every UI action maps to a
  `#[tauri::command]` in `crates/recall-app/src/commands.rs`.
  Status / search / list operations are simple; long-running ops
  (`import_folder`, `import_takeout`, `verify`, `backup`) take a
  `tauri::Channel<ProgressEvent>` and stream per-file progress to
  the frontend.
- **AppState**: a single open SQLite connection + ObjectStore, held
  behind a `Mutex`. Created on `setup` from
  `RECALL_ARCHIVE_ROOT`.
- **Frontend**: a single self-contained `ui/index.html` with inlined
  CSS + JS. No build step, no bundler — Tauri exposes its API as
  `window.__TAURI__` (via `withGlobalTauri: true`).
- **Three views**:
  - **Library** — kind + album filters in a left sidebar, search
    bar, year-grouped photo grid with lazy-loaded thumbnails via
    `cmd_get_thumbnail_bytes`, inspector panel that slides in from
    the right on tile click.
  - **Import** — drag-and-drop zone + path input + takeout / plain
    folder mode toggle + live progress bar (imported / duplicates /
    failed / ETA) + scrolling event log.
  - **Settings** — archive info, verify, backup, recent imports.
- **Toast notifications** confirm every action and surface errors.
- **Inspector** uses `cmd_get_media_detail` (single round-trip
  returning media + metadata + timestamps + locations + albums +
  source files) plus `cmd_get_thumbnail_bytes` (base64-encoded
  preview).
- **To run**: `RECALL_ARCHIVE_ROOT=~/recall-archive cargo run
  --release -p recall-app`. (Skip `cargo tauri dev` for day-to-day
  work — its file-watcher rebuild loop is noisy for a no-bundler
  UI.)

## 4.7 What's NOT yet built

See `05-roadmap.md` for the full list. The big-ticket items:

- **Map view + clustering** — search engine supports
  `near_lat` / `near_lon` / `radius_km`; no frontend map yet.
- **User-album editor** — can list albums, can't create / rename /
  add items yet.
- **Remote backup (S3/B2/R2)** — `BackupDestination` trait is in
  place; a concrete `S3Destination` impl is the next step.
- **Video metadata** — duration / dimensions / GPS-in-MP4 not yet
  extracted.
- **Native folder-picker dialog** — users currently paste a path
  or drag a folder onto the dropzone. tauri-plugin-dialog not
  wired.
- **Watch-folder / auto-import** — manual `import_folder` only.
