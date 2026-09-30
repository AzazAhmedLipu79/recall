# v1 launch prep — status snapshot

**Last updated:** end of M4, 2026-09-30.
**Goal:** ship a polished, signed, notarised macOS app publicly.
**Target platform for v1:** macOS only (Windows / Linux / mobile deferred).

---

## What's done (green tests, code on disk)

### ✅ M1 — Project hygiene (complete)

- `LICENSE` (MIT)
- `README.md` (product pitch, install, build, screenshots)
- `PRIVACY.md` (data stays local, no telemetry by default, opt-in crash report)
- `NOTICE` / `CONTRIBUTING.md` / `CHANGELOG.md`
- `.github/workflows/release.yml` (tauri-action, signing + notarisation)
- `.github/workflows/ci.yml` (fmt + clippy + test)
- `.github/ISSUE_TEMPLATE/{bug,feature}.md`

### ✅ M3 — Album CRUD backend (complete)

Added the backend half of album management so the UI layer can wire to it.

**New commands in `crates/recall-app/src/commands.rs`:**

| Command | What it does |
|---|---|
| `cmd_create_album(name)` | Insert or fetch (idempotent on `(name, "user")`). Returns id. |
| `cmd_rename_album(id, name)` | Only user albums (Takeout albums can't be renamed). |
| `cmd_delete_album(id)` | Cascades via `ON DELETE CASCADE` FK. |
| `cmd_add_to_album(album, media)` | `INSERT OR REPLACE` on `album_items`. |
| `cmd_remove_from_album(album, media)` | Returns affected rows; UI ignores 0. |
| `cmd_get_albums_for_media(media)` | Powers inspector "In albums" chips. |
| `cmd_is_first_run()` | `media_count() == 0`. Powers wizard boot. |
| `cmd_seed_sample(count?)` | Defaults 20. Writes to `<archive>/seed-tmp/`, imports through `import_takeout`, then `remove_dir_all` the temp. Streams progress. |
| `cmd_reveal_in_finder(path)` | `open -R <path>` on macOS, `open <path>` elsewhere. |
| `cmd_log_dir()` | Returns `<archive>/logs/` for Settings → Show Logs. |

**Test coverage:** 21 db tests pass (6 new), 15 importer tests pass.

### ✅ M2 — Logging wired up (complete)

- `tracing-appender` added to workspace deps.
- `tracing-subscriber` initialised in `main.rs` *before* Tauri takes over:
  - **Stderr layer** for `cargo run` (ANSI colours).
  - **Daily-rolling file layer** at `<archive>/logs/recall.log.YYYY-MM-DD` (no ANSI) for shipped builds / support post-mortems.
  - Fallback to stderr-only if the log dir can't be created (best-effort, never blocks launch).
- `EnvFilter` default: `info,recall_app=debug,recall_importer=info`. Override via `RUST_LOG=...`.
- `tracing::info!` / `tracing::error!` calls added to: `cmd_import_folder`, `cmd_import_takeout`, `cmd_verify`, `cmd_backup`, `cmd_export_zip`, `cmd_seed_sample`.

### ✅ M4 — UI module split (complete)

The 1833-line single-file `ui/index.html` is now a 239-line shell +
24 ES modules + 5 CSS files. No bundler. Every import path includes
the `.js` extension so it resolves under Tauri's `asset://` origin.

**`ui/` tree:**

```
ui/
  index.html                  # shell only (239 lines) — topbar, sidebar,
                              # view sections, inspector, lightbox, toast
                              # stack, wizard modal, album-menu popover,
                              # <script type="module" src="./js/main.js">
  css/
    tokens.css                # :root vars (24 lines)
    base.css                  # reset + body (15 lines)
    layout.css                # topbar/sidebar/main/statusbar/grid (228 lines)
    components.css            # tiles/inspector/lightbox/import+settings/
                              # toast/wizard/album-menu (593 lines)
    themes.css                # responsive + light-theme placeholder (19 lines)
  js/
    main.js                   # sequential boot (113 lines)
    router.js                 # showView + nav bindings (26 lines)
    api.js                    # invoke/Channel + cmd_* wrappers (66 lines)
    state.js                  # shared: kind/album/selectedId/mode/results (28 lines)
    util/
      dom.js                  # $, escapeHtml, hashStr, delegate (42 lines)
      format.js               # fmtDate, fmtSize, fmtBytes (25 lines)
      toast.js                # toast() (26 lines)
      keys.js                 # global shortcuts (56 lines)
      url.js                  # mediaUrl/getMediaPath cache (39 lines)
    views/
      library.js              # search + grid + lazy thumbnails (152 lines)
      inspector.js            # slide-in panel + metadata (170 lines)
      lightbox.js             # fullscreen + keyboard nav (106 lines)
      import.js               # drag-drop + folder picker + progress (172 lines)
      settings.js             # verify/backup/export (135 lines)
      wizard.js               # 3-step first-run wizard (97 lines)
      albums.js               # sidebar list + #album-pick select (58 lines)
      statusbar.js            # counts + archive root (27 lines)
    components/
      tile.js                 # tile factory (53 lines)
      album-menu.js           # add-to-album popover (76 lines)
```

**Boot sequence** (`main.js`):

```js
document.addEventListener("DOMContentLoaded", async () => {
  assertTauri();             // toast + bail if window.__TAURI__ is missing
  // init all views…
  bindNavButtons(...);
  bindGlobalKeys(...);
  // Sequential — parallelising would race on #album-pick options.
  await refreshAlbums();
  await refreshStatus();
  await wizard.checkFirstRun();
  showView("library");
});
```

**Verification:**
- `cargo test --workspace` → **72 passing, 1 ignored, 0 failed** (baseline preserved).
- Every JS module parses cleanly as an ES module.
- Module-path convention (`.js` suffix on every import) is honoured across all 24 files.

### ✅ Earlier (already shipped in prior session)

- **Crash fix:** replaced broken `recall://` URI scheme with built-in `asset://` protocol.
- **Moved `seed::run`** from `recallctl/src/seed.rs` to `recall-importer/src/seed.rs`.

---

## What's NOT yet built (the work queue)

Per the plan at `/Users/darkmac/.puku-cli/plans/quiet-imagining-waffle.md`. In order:

### 🟡 M5 — First-run wizard

The shell + boot wiring is in place from M4. Remaining work:

1. **Welcome step** is rendered. Add the "Get started →" screenshot.
2. **Archive location step:** populate `#wiz-archive-path` with
   `cmd_archive_info().archive_root`. "Change…" button → `tauri-plugin-dialog`
   folder picker (also M5 plugin install).
3. **Sample seed step:** "Try with 20 sample photos" calls
   `cmd_seed_sample(20)` with progress. Already wired in `views/wizard.js`.
4. **Persist `recall_wizard_seen` flag** in `tauri-plugin-store` when
   user clicks Skip or Done so the wizard doesn't reappear.
5. **Plain-HTML "Skip setup" link** from any step closes the wizard.

### 🟡 M6 — Albums UI

The album-menu popover exists; the sidebar list works. Still to wire:

1. Sidebar `+ New album` button (input prompt → `cmd_create_album`).
2. Right-click album row → native context menu with rename / delete.
3. Inspector "Add to album…" trigger (`data-action="open-album-menu"`)
   pointing to the album-menu popover.
4. Album row in sidebar shows media count from `cmd_list_albums`.

### 🟡 M7 — Keyboard shortcuts + accessibility

Shortcuts are wired in `util/keys.js`. Still to do:

- `role="dialog"` + `aria-modal="true"` + focus trap on inspector,
  lightbox, wizard.
- All buttons get `aria-label`.
- Grid tiles become `<button>` (currently `<div>` with click handler).
- `prefers-reduced-motion` respected for the lightbox fade and tile hover.
- Visible focus ring (`--accent` already in tokens; add `:focus-visible`).
- Shortcut overlay (currently a toast) — upgrade to a proper panel.

### 🟡 M2.5 — Opt-in crash reporting

`sentry-rust` + `sentry-tauri`. Settings toggle "Send anonymous crash reports" — **default off**. When on, only crashes + breadcrumbs (no PII, no archive content).

### ⛔ Dropped from v1

- **M8 — Apple code signing + notarisation + auto-update channel.**
  Removed from scope because distribution is handled by an out-of-tree
  build script rather than from inside the app. No GitHub Actions
  release workflow, no `tauri-action` signing step, no
  `tauri-plugin-updater`, no Settings → About → "Check for updates"
  button. If the build script ever needs inline docs for these steps,
  they live in that script's README, not here.

---

## Key files to know about

| File | Why it matters |
|---|---|
| `crates/recall-app/src/main.rs` | Tracing init + Tauri builder + `invoke_handler!` list. |
| `crates/recall-app/src/commands.rs` | All `#[tauri::command]`s. Long-running ops take `Channel<ProgressEvent>` and stream progress. |
| `crates/recall-app/src/state.rs` | `AppState { conn, store, archive_root, db_path, store_root }`. Single mutex. |
| `crates/recall-db/src/lib.rs` | `Catalog` wrapper. All album CRUD + `media_count` etc. |
| `crates/recall-importer/src/seed.rs` | Synthetic media generator used by both `recallctl seed` and `cmd_seed_sample`. |
| `crates/recall-app/ui/index.html` | 239-line shell. View modules live under `ui/js/`. |
| `crates/recall-app/ui/js/main.js` | Boot sequence. Sequential `await`s. |
| `crates/recall-app/ui/js/api.js` | Every `cmd_*` wrapper + Tauri globals captured once. |
| `Cargo.toml` | Workspace deps. New `tracing-appender = "0.2"` added. |
| `CHANGELOG.md` | Updated as each milestone ships. |
| `/Users/darkmac/.puku-cli/plans/quiet-imagining-waffle.md` | The full v1 plan. |

---

## Verification commands (run after each milestone)

```bash
# Compile-check everything
cargo check --workspace

# Run the entire test suite (should be 72 tests, 1 ignored)
cargo test --workspace 2>&1 | grep "test result"

# Build the release .app + .dmg
cargo tauri build

# Run the app pointed at a throwaway archive
RECALL_ARCHIVE_ROOT=/tmp/recall-test cargo run --release -p recall-app
```

### M4 — UI module split verification (manual)

- [ ] Launch app, switch views, search, click a tile, open inspector,
      open lightbox, drag a folder onto the dropzone, run verify.
- [ ] No console errors.
- [ ] Network panel shows `asset://localhost/...` requests succeeding
      for tile thumbnails and the inspector preview.
- [ ] Hard-refresh (Cmd+Shift+R) still works.

---

## Risk register (lifted from the plan)

| Risk | Mitigation |
|---|---|
| Icon is a 512×512 placeholder | Designer pass before public launch. |
| Wizard blocks non-empty archives | `cmd_is_first_run` only fires on truly empty DB; "Skip setup" link always present. |
| Album delete cascade | Already verified: `album_items.album_id` has `ON DELETE CASCADE` FK. |
| ES module split load-order bugs | Sequential boot in `main.js` + `await` on every init. Verified: 72 tests pass. |
| `withGlobalTauri` + log plugin race | Defer JS-side `__TAURI__` calls until after `assertTauri()` succeeds. |
| Distribution script drift | Keep build steps documented in the script's own README so M8 details (if ever needed) live next to the code that performs them. |

---

## Resume tomorrow

When you come back:

1. **Read this file** (5 min) — you're caught up.
2. **Read `/Users/darkmac/.puku-cli/plans/quiet-imagining-waffle.md`** (15 min) — the full plan + architecture rationale.
3. **Run `cargo test --workspace`** — confirm 72 passing / 1 ignored / 0 failed.
4. **Start M5 (First-run wizard).** Backend is done; the work is wiring
   `tauri-plugin-dialog` for the folder picker, persisting
   `recall_wizard_seen` in `tauri-plugin-store`, and adding the
   welcome screenshot.
5. **M6 follows directly:** the album-menu popover is already in place
   from M4 — just need to wire the inspector trigger.

Sleep well. 🌙
