# recall-app

Tauri 2 desktop UI for Recall.

## What this is

A thin host process that wires the existing `recall-*` engine crates
to a webview UI. Long-running engine calls stream `ProgressEvent`s to
the frontend over a `tauri::Channel` so the UI can render live
progress without polling.

The frontend in `ui/index.html` is intentionally a single self-
contained file: vanilla HTML + inlined CSS + inlined JS, no build
step, no bundler. Tauri exposes its API on the `window.__TAURI__`
global at runtime (via `withGlobalTauri: true`).

## UI surface

Three top-level views, switchable from the top-right of the window:

* **Library** — left sidebar of kind filters (Photos / Videos /
  Audio / Documents) + albums, search bar (text + album + has-
  location + limit), and a year-grouped photo grid. Click any tile
  to open the inspector.
* **Inspector** — slides in from the right when you click a tile.
  Shows the file blob, full EXIF/container metadata, every
  provenance timestamp, GPS pills, albums pills, and every tracked
  source-path row.
* **Import** — drag-and-drop zone, source-path input, takeout vs.
  plain-folder mode toggle, live progress bar with imported /
  duplicate / failed counts and ETA, plus a scrolling event log.
* **Settings** — archive paths, verify button, backup button,
  recent imports list.

Toast notifications confirm every successful action and surface
errors without blocking.

## Crate layout

```
crates/recall-app/
  Cargo.toml          # tauri 2, recall-* engine deps
  build.rs            # tauri-build
  tauri.conf.json     # window, frontend dist, app identifier
  capabilities/
    default.json      # permissions for the main window
  icons/
    icon.png          # placeholder; replace via `cargo tauri icon`
  src/
    main.rs           # Tauri builder + handler registration
    state.rs          # AppState (open SQLite + ObjectStore)
    commands.rs       # #[tauri::command] fns — one per UI action
  ui/
    index.html        # all CSS + JS inlined
```

## Commands exposed

| Command                     | Engine call                          | Streams progress? |
| --------------------------- | ------------------------------------ | ----------------- |
| `cmd_archive_info`          | reads `AppState` paths               | no                |
| `cmd_status`                | `Catalog::media_counts`              | no                |
| `cmd_search`                | `Catalog::search_media`              | no                |
| `cmd_list_albums`           | `Catalog::list_albums`               | no                |
| `cmd_list_imports`          | `Catalog::list_imports`              | no                |
| `cmd_get_media_detail`      | `get_media` + metadata/timestamps/   | no                |
|                             | locations/albums/source_files        |                   |
| `cmd_get_thumbnail_bytes`   | reads from `ObjectStore`, base64s    | no                |
| `cmd_import_folder`         | `import_folder`                      | yes               |
| `cmd_import_takeout`        | `import_takeout`                     | yes               |
| `cmd_verify`                | `verify_all`                         | yes               |
| `cmd_backup`                | `backup_to` (LocalDestination)       | yes               |

## Running

Install the Tauri CLI once:

```sh
cargo install tauri-cli --version "^2"
```

Then from the workspace root:

```sh
RECALL_ARCHIVE_ROOT=$HOME/recall-archive \
  cargo tauri dev --manifest-path crates/recall-app/Cargo.toml
```

`cargo tauri dev` triggers a file-watcher rebuild loop that's noisy
for a vanilla-JS UI. For day-to-day work, build the binary directly
and run it:

```sh
RECALL_ARCHIVE_ROOT=$HOME/recall-archive \
  cargo run --release -p recall-app
```

The first run takes a while because Tauri pulls in webkit bindings.
After that it's a normal Cargo release build.

To package a release `.app` / `.msi` / `.deb`:

```sh
cargo tauri build --manifest-path crates/recall-app/Cargo.toml
```

You'll need real icons under `icons/` for a packaged build
(`cargo tauri icon path/to/source.png` generates the full set).

## Archive root

The app reads `RECALL_ARCHIVE_ROOT` to know where the archive lives
(the same default the CLI uses). It expects:

```
$RECALL_ARCHIVE_ROOT/
  archive.db
  store/
    objects/<aa>/<bb>/<sha256>
    sidecars/<sha256>.json
```

`AppState::open` will create `archive.db` and `store/` if they're
missing, mirroring `recallctl init`.

## Engine integration

`commands.rs` is the only file that knows about Tauri. Everything
below it is plain engine code. To add a new UI feature:

1. Add a `pub fn cmd_*` to `commands.rs`.
2. Register it in the `invoke_handler!` macro list in `main.rs`.
3. Call it from `ui/index.html` via `invoke('cmd_*', { ... })`.
4. (If it's long-running) thread `Option<&dyn Progress>` through
   the engine call; the `ChannelProgress` adapter already wraps a
   `tauri::Channel<ProgressEvent>` and forwards every event.

## Progress wire format

`ProgressEvent` is serialized with `#[serde(tag = "type",
rename_all = "snake_case")]`, e.g.

```json
{"type":"file_imported","path":"/x.jpg","media_id":"abc…"}
{"type":"verify_ok","path":"/store/objects/aa/bb/…"}
{"type":"finished","kind":"import-folder","ok":42,"failed":0,"skipped":0}
```

The frontend handles `file_imported` / `file_duplicate` /
`file_failed` per-file events for the live counters and progress
bar; the `started` event seeds the `total` for ETA, and `finished`
flips the bar to 100%.

## Known gaps

- Map view + clustering (PRD §8.3) not yet wired; the search engine
  supports `near_lat` / `near_lon` / `radius_km` already.
- No "create user album" UI surface yet.
- No album-item add/remove UI yet.
- Native folder-picker dialog (tauri-plugin-dialog) not yet wired;
  users currently paste a path or drag a folder onto the dropzone.
- Icons are 1×1 placeholders; generate real ones before packaging.
- Thumbnail loading uses base64 data URIs rather than the native
  `asset://` protocol. Fine for personal-archive scale; swap if it
  becomes a bottleneck.
