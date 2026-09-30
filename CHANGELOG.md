# Changelog

All notable changes to Recall are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/), and the project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Settings → Archive: "Switch archive…" button. Picker probes the
  chosen folder and either (a) opens an existing Recall archive in
  place via `cmd_set_archive_root`, (b) hands off to the existing
  Move flow to copy the current archive into an empty folder, or
  (c) refuses a folder that has unrelated files. Backed by the
  new `cmd_probe_archive_root` tauri command (`ArchiveProbe`
  carries `path`, `has_archive_db`, `has_store`, `is_empty`,
  `is_current`).
- `tracing::info!` log line on archive root resolution so the path
  the GUI picked at launch is greppable in `<archive>/logs/`.
- `.gitignore` covering `target/`, `*.db*` (catalogs), `logs/`,
  build artefacts (`*.dmg`, `*.app`, `*.zip`), and IDE noise
  (`.DS_Store`, `.vscode/`).

### Changed
- Switch-archive modal uses the shared `trapFocus` util for Escape
  handling, focus cycling, and focus restoration (previously the
  wizard and switch modal each had their own keydown listeners).

### Fixed
- `Promise.all` of post-migration refreshes swapped for
  per-task `.catch` so one failing refresh no longer discards the
  rest.

## [v0.1.0] — 2026-09-30

### Added
- UI module split: `ui/index.html` (1833 → 239 lines) broken into
  `ui/css/{tokens,base,layout,components,themes}.css` and ES modules
  under `ui/js/`. No bundler — Tauri 2's `asset://` origin resolves
  `.js`-suffixed imports natively.
  - `util/{dom,format,keys,url,toast}.js`
  - `state.js` (shared `let` + setter functions for kind/album/selection/mode/results),
    `api.js` (typed wrappers for every `cmd_*` + Tauri globals),
    `router.js` (topbar nav + `showView`)
  - `views/{library,inspector,lightbox,import,settings,wizard,albums,statusbar}.js`
  - `components/{tile,album-menu}.js`
  - `main.js` (sequential boot: assertTauri → statusbar → inspector →
    lightbox → library → albums → settings → import → wizard → album-menu
    → nav bindings → keyboard shortcuts → `refreshAlbums()` →
    `refreshStatus()` → `checkFirstRun()` → `showView("library")`).
- First-run wizard shell + backend wiring: `wizard` modal in
  `index.html` with three steps (welcome / archive path / sample seed);
  `views/wizard.js` exposes `checkFirstRun()` which auto-shows the
  wizard when `cmd_is_first_run()` returns true.
- Album-menu popover (Inspector `Add to album…` lives here once M6
  wires the trigger).
- Global keyboard shortcuts module (`util/keys.js`): ⌘1/⌘2/⌘3 switch
  views, ⌘F focuses search, ⌘O jumps to Import + focuses path input,
  ⌘I opens inspector for the selected tile, ? shows the shortcut
  overlay toast.
- Wizard modal markup + first three-step scaffold in `index.html`.
- Album CRUD backend: `cmd_create_album`, `cmd_rename_album`,
  `cmd_delete_album`, `cmd_add_to_album`, `cmd_remove_from_album`,
  `cmd_get_albums_for_media`. `AlbumRow.media_count` populated via
  `LEFT JOIN`.
- First-run wizard backend: `cmd_is_first_run`, `cmd_seed_sample`
  (wraps `recall_importer::seed::run`).
- Settings helpers: `cmd_reveal_in_finder` (`open -R` on macOS),
  `cmd_log_dir` (returns `<archive>/logs/`).
- File-based logging via `tracing-subscriber` + `tracing-appender`:
  daily-rolling logs at `<archive_root>/logs/recall.log.YYYY-MM-DD`,
  plus ANSI stderr for `cargo run`. Override level via `RUST_LOG`.
- `tracing::info!` / `tracing::error!` calls on every long-running
  command (`import_folder`, `import_takeout`, `verify`, `backup`,
  `export_zip`, `seed_sample`).
- `seed::run` exposed publicly from `recall_importer` so both the
  CLI and the desktop app can use the same synthetic-media generator.
- Project hygiene: `LICENSE` (MIT), `README.md`, `PRIVACY.md`,
  `NOTICE`, `CONTRIBUTING.md`, `.github/workflows/{ci,release}.yml`,
  `.github/ISSUE_TEMPLATE/{bug,feature}.md`.
- `docs/v1-launch-status.md` — status snapshot of the v1 launch work.

### Changed
- `asset://` is now used for archived media streaming (replaces the
  custom `recall://` scheme). Hard prerequisite for shipping anything.
- UI boot path: the previous single-script inline `<script>` block in
  `index.html` is replaced by a single `<script type="module" src="./js/main.js">`.
  All view wiring lives in `main.js` (sequential `await`s, no
  races on `#album-pick` options).

### Removed
- `cmd_get_thumbnail_bytes` and the in-memory base64 thumbnail path
  (unused since the `asset://` switch).
- Inline `<style>` block in `index.html` (~770 lines of CSS moved to
  `ui/css/{tokens,base,layout,components,themes}.css`).
- Inline `<script>` block in `index.html` (~870 lines of JS moved to
  `ui/js/` modules).

## Planned for v1 (not yet shipped)

- M5 wizard backend integration: persist `recall_wizard_seen` in
  `tauri-plugin-store`; wire "Change…" to `tauri-plugin-dialog`
  folder picker; populate `wiz-archive-path` from `cmd_archive_info`.
- M6 albums UI surface: sidebar `+ New album`, right-click rename /
  delete, inspector "Add to album…" trigger wiring (popover already
  rendered).
- M7 keyboard shortcuts + accessibility audit: focus traps, `aria-*`,
  `prefers-reduced-motion`, grid tiles become `<button>` for
  keyboard nav.
- M2.5 opt-in crash reporting via `sentry-rust` + `sentry-tauri`
  (default off; no PII).

> **Distribution note:** M8 (Apple code signing + notarisation +
> auto-update channel) was removed from the v1 scope. The build is
> produced by an out-of-tree script that handles distribution; the
> app does not sign itself or manage updates in v1.

See `docs/v1-launch-status.md` for the full roadmap and risk register.

[Unreleased]: https://github.com/AzazAhmedLipu79/recall/compare/v0.1.0...HEAD
[v0.1.0]: https://github.com/AzazAhmedLipu79/recall/releases/tag/v0.1.0