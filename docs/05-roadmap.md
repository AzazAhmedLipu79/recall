# 5. Roadmap

What still needs to be built, in the serial order I recommend tackling
it. Each item lists the requirements it closes, the rough scope, and
the risks.

## Phase R1 — Search & query CLI

**Closes:** FR-7.4, FR-7.5 (and unblocks all UI work).

**Why first.** Every column the UI will need already exists in
`media`, `media_metadata`, `media_timestamps`, `locations`, and
`album_items`. Search via CLI is the cheapest way to validate the
data model and the highest leverage for the user — "show me all
photos from Tokyo in 2024" should work *before* there's a GUI.

**Scope.**
- New subcommand `recallctl search` with filters:
  - `--kind photo|video|document|audio|other`
  - `--from <ts>` / `--to <ts>` (against `canonical_taken_at`)
  - `--album <name>`
  - `--source <label>`
  - `--location <lat>,<lon> [--radius-km N]` (uses `locations`)
  - `--limit N`
- Output format: a one-line per row, similar to existing CLI.
  Optional `--json` for machine consumption (also lays groundwork for
  the GUI).
- Add a `Catalog::search` method returning a typed iterator.

**Risks.**
- None new; everything sits on top of existing schema and indexes.
- Need a small bbox / haversine helper for "near a point" queries.

**Dependencies.** None — purely additive.

---

## Phase R2 — Live progress events

**Closes:** FR-8.2 partially.

**Why second.** The Tauri UI (Phase R5) needs to show per-file
progress. Designing the event stream now, before the GUI exists,
locks in a stable wire format we won't have to break later.

**Scope.**
- Add a `Progress` trait / channel to `import_file` so callers
  receive `Event::Discovered | Imported(media_id) | Duplicate |
  Failed(reason)`.
- `recallctl import-*` gains `--json` (newline-delimited JSON on
  stdout) and `--progress` (TTY-friendly text).
- Same plumbing feeds the future Tauri UI via a `tauri::Channel` or
  equivalent.

**Risks.**
- Plumb a callback through `import_file` carefully — must not break
  the existing 13 importer tests.
- Avoid making `recall-importer` depend on `tauri`; the channel
  type should live in `recall-core` or a new tiny crate.

---

## Phase R3 — Rich metadata extraction

**Closes:** FR-3.3 (full coverage).

**Why third.** Today `recall-metadata` reads a thin slice of EXIF.
The schema supports more (camera, lens, ISO, aperture, dimensions,
duration, GPS). Filling this in makes search meaningful (especially
location), enables future "by camera" / "by lens" filters, and is a
hard prerequisite for any timeline UI worth using.

**Scope.**
- **Photos.** Replace the hand-rolled EXIF walker in
  `recall-metadata::exif` with `kamadak-exif` (battle-tested,
  supports every standard tag + MakerNotes). Keep our conflict-
  resolution rules (EXIF beats filesystem).
- **Videos.** Add container-level metadata extraction for MP4/MOV
  (duration, dimensions, rotation) using `mp4` or `matroska` crates,
  keyed off `kamadak-exif`'s QuickTime branch.
- **Audio.** ID3 / Vorbis comments for `kind=audio` items — title,
  artist, album.
- **GPS unification.** When EXIF GPS exists, also write a
  `locations` row tagged `exif` so search can filter by location
  uniformly across photo and video sources.
- Tests covering each new extractor against fixture files.

**Risks.**
- New crates mean new supply-chain surface. Pin versions.
- MakerNote handling varies wildly between vendors; test against
  a few real iPhone / Samsung / Canon JPEGs.

---

## Phase R4 — Remote backup

**Closes:** FR-5.x expansion.

**Why fourth.** Local secondary drive handles the "house fire"
scenario on its own. Real offsite redundancy needs cloud storage,
and once we have search + remote backup, the archive is genuinely
useful as a personal vault even without a GUI.

**Scope.**
- Introduce a `BackupDestination` trait in `recall-backup` so the
  existing local copy logic is reusable.
- Implement an `S3CompatibleDestination` using the `object_store`
  crate (works for AWS S3, Backblaze B2, Cloudflare R2, MinIO).
- Add `recallctl backup --destination s3://bucket/prefix` (env vars
  for credentials: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, etc.).
- Same atomic guarantees: write to a key like `prefix/objects/...tmp`
  with content-md5, then copy to the final key.
- Local destination stays the default.

**Risks.**
- Cloud providers have differing semantics for "rename" / "copy";
  work within each provider's primitives.
- Cost: uploads should be dedup-aware (the existing sha256 check
  already gives us this).
- Credentials: never log them, never store them in the catalog.

---

## Phase R5 — Tauri UI

**Closes:** FR-8.1, FR-8.2.

**Why fifth.** Everything below the UI is in place: the engine has
a stable API, the schema has columns the UI needs, and there's a
JSON progress stream from R2. Building the UI last lets us focus
entirely on UX rather than racing the engine.

**Scope (high level — spec out before coding).**
- **Timeline view.** Group by month, drill to day, infinite scroll.
  Backend = `search --from --to --kind --limit`.
- **Map view.** Map of all locations, clustered. Backend = a new
  `recallctl search --location-bbox` (or a tiny built-in HTTP
  endpoint for the UI).
- **Albums.** Browse, create user albums (kind=`user`), add / remove
  items.
- **Inspector.** Side panel with all metadata + sidecar JSON +
  "show in Finder" button.
- **Import progress.** Subscribe to the R2 progress channel while
  `recallctl import-*` runs (probably via `tauri::Command` spawning
  the binary).
- **Settings.** Archive root, backup destination, verify schedule.

**Risks.**
- The Tauri app needs to know where the archive root lives.
  Probably stored in `~/.config/recall/config.toml` (separate from
  the archive itself).
- Any logic in the UI that duplicates engine logic is a bug.
  Treat the engine as the only source of truth.

---

## Phase R6 — Future

Things explicitly out of scope of this roadmap but worth mentioning:

- **Smart albums** (`kind=smart`) — SQL-like saved queries.
  Trivially enabled by the schema; needs a UI surface in R5.
- **GPX / track import** — feed GPS tracks to back-fill locations
  on photos taken near them.
- **Apple Photos library import** — same pipeline, different
  source format.
- **Face / object recognition** — heavy ML, no commitments.
- **Multi-machine sync** — CRDTs over the catalog DB, last writer
  wins on metadata. Hard. Optional.
- **Encryption at rest** — optional key wrapping for the `objects/`
  tree. Real value-add but heavy.

---

## Recommended execution order

| Order | Phase | Approx. size | Hard pre-reqs |
| ----- | ----- | ------------ | ------------- |
| 1     | R1 — Search CLI        | Small  | none |
| 2     | R2 — Progress events   | Small  | none |
| 3     | R3 — Rich metadata     | Medium | none |
| 4     | R4 — Remote backup     | Medium | none |
| 5     | R5 — Tauri UI          | Large  | R1, R2 |

R1 → R2 → R3 → R4 can be done in any order after R1/R2 are
established; R5 should come last so the GUI isn't chasing an API
that's still moving.
