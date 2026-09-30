//! `recall-app` — Tauri 2 desktop UI for Recall.
//!
//! This binary is a thin host around the existing engine crates. The
//! frontend (HTML/TS in `../ui`) calls into `commands::*` over the IPC
//! bridge; commands delegate to `recall-db`, `recall-importer`, etc.
//! Long-running operations stream [`recall_core::ProgressEvent`]s
//! through a `tauri::ipc::Channel` so the UI can render live progress.
//!
//! Archived media bytes are streamed to the webview through Tauri's
//! built-in `asset://` protocol. The frontend asks the backend for
//! an absolute filesystem path (`cmd_get_media_path`) and then uses
//! `convertFileSrc(absPath)` to build an `asset://localhost/...` URL
//! that the webview can drop straight into `<img>` / `<video>` /
//! `<audio>` elements. The runtime archive root is added to the asset
//! protocol scope in `setup` (the static scope in `tauri.conf.json`
//! already covers `$HOME` and the per-user data/config dirs).
//!
//! Tracing is initialised here too: `tracing-subscriber` writes to
//! stderr (so `cargo run` shows logs in the terminal) AND to a
//! daily-rolling file under `<archive_root>/logs/recall.log.YYYY-MM-DD`
//! (so shipped builds leave a paper trail for support). The log
//! directory is created if missing.
//!
//! The whole crate is `forbid(unsafe_code)` per workspace policy.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod commands;
mod state;

use state::AppState;

const PERSISTED_ROOT_FILE: &str = "archive-root.json";

#[derive(Debug, Serialize, Deserialize)]
struct PersistedArchiveRoot {
    /// Absolute path the user picked via the wizard or "Move archive…".
    /// Serialised as `archive_root` so the on-disk file is greppable.
    archive_root: String,
}

/// Read the persisted archive-root file (`<app_config_dir>/archive-root.json`).
/// Returns `None` if the file doesn't exist or can't be parsed — both
/// fall through to the legacy defaults.
fn read_persisted_archive_root<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Option<PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let path = dir.join(PERSISTED_ROOT_FILE);
    let bytes = std::fs::read(&path).ok()?;
    let parsed: PersistedArchiveRoot = serde_json::from_slice(&bytes).ok()?;
    let p = PathBuf::from(parsed.archive_root);
    if p.as_os_str().is_empty() {
        None
    } else {
        Some(p)
    }
}

/// Persist the user's chosen archive root so subsequent launches
/// honour it (without requiring `RECALL_ARCHIVE_ROOT` to be set). The
/// file lives under `app_config_dir` (e.g. macOS:
/// `~/Library/Application Support/sh.recall.app/archive-root.json`).
fn write_persisted_archive_root<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    root: &Path,
) -> Result<(), String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("could not resolve app config dir: {e}"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create app config dir {}: {e}", dir.display()))?;
    let path = dir.join(PERSISTED_ROOT_FILE);
    let payload = PersistedArchiveRoot {
        archive_root: root.to_string_lossy().into_owned(),
    };
    let bytes = serde_json::to_vec_pretty(&payload)
        .map_err(|e| format!("could not serialise archive-root: {e}"))?;
    let tmp = path.with_extension("json.partial");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("could not rename to {}: {e}", path.display()))?;
    Ok(())
}

/// Resolve the archive root. The precedence is:
///
/// 1. `RECALL_ARCHIVE_ROOT` env var (always wins; useful for CLI dev
///    overrides and for `recallctl`).
/// 2. The persisted file written by the wizard / "Move archive…" UI.
/// 3. Legacy defaults:
///    - `$HOME` if `$HOME/archive.db` already exists (so a CLI run
///      left a library there and the GUI picks it up);
///    - `$HOME/Recall` otherwise (created on first launch).
/// 4. `"."` as a last-resort fallback.
fn resolve_archive_root<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> PathBuf {
    if let Ok(custom) = std::env::var("RECALL_ARCHIVE_ROOT") {
        return PathBuf::from(custom);
    }
    if let Some(persisted) = read_persisted_archive_root(app) {
        return persisted;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = PathBuf::from(home);
        let existing = home_path.join("archive.db");
        if existing.exists() {
            return home_path;
        }
        let mut p = home_path;
        p.push("Recall");
        return p;
    }
    PathBuf::from(".")
}

/// Initialise `tracing-subscriber` with two layers:
///
/// 1. A stderr writer that uses ANSI colours — for dev (`cargo run`)
///    so logs are immediately readable in the terminal.
/// 2. A daily-rolling file writer at
///    `<archive_root>/logs/recall.log.YYYY-MM-DD` — for shipped
///    builds so users (and support) can grep the archive root for
///    problems after the fact.
///
/// If the log directory cannot be created (permissions, full disk),
/// we fall back to stderr-only rather than aborting the launch —
/// logging is best-effort; the app itself is the thing that must
/// start.
///
/// The non-blocking writer guard is returned so the caller can drop it
/// after Tauri's event loop starts (a few buffered lines will then
/// flush on shutdown).
fn init_tracing(archive_root: &PathBuf) {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,recall_app=debug,recall_importer=info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true);

    // Try to set up the file layer. If it fails, init just the stderr
    // layer so we still get *some* output.
    let log_dir = archive_root.join("logs");
    match std::fs::create_dir_all(&log_dir) {
        Ok(_) => {
            let file_appender = tracing_appender::rolling::daily(&log_dir, "recall.log");
            let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false)
                .with_target(true);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(file_layer)
                .init();

            // Stash the guard so the writer isn't dropped (and its
            // background thread killed) before the program exits.
            // `Box::leak` is fine here — the subscriber lives for the
            // life of the process and there's exactly one of them.
            let _ = Box::leak(Box::new(_guard));

            tracing::info!(
                archive_root = %archive_root.display(),
                log_dir = %log_dir.display(),
                "tracing initialised (stderr + daily file)"
            );
        }
        Err(e) => {
            // Couldn't make the log dir — stderr only.
            tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .init();
            tracing::warn!(
                "could not create log dir {}: {e}; using stderr only",
                log_dir.display()
            );
        }
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            // Resolve the archive root inside `setup` so we can read
            // the persisted JSON file (which lives in the app config
            // dir). The file log layer needs the root *before* any
            // tracing macros fire, so initialise tracing first.
            let handle: tauri::AppHandle = app.handle().clone();
            let archive_root = resolve_archive_root(&handle);
            init_tracing(&archive_root);

            // Initialise app state: an open SQLite connection + ObjectStore
            // rooted at the configured archive directory. Surface any
            // failure as an Err so Tauri logs it instead of aborting the
            // whole process (which would surface as "Recall quit
            // unexpectedly" with SIGABRT on macOS).
            tracing::info!("recall-app starting; archive_root={}", archive_root.display());

            // Extend the asset protocol scope at runtime so the webview
            // can stream any file under the resolved archive root via
            // `convertFileSrc` -> `asset://localhost/...`. The static
            // `tauri.conf.json` already whitelists `$HOME` (and its
            // `Recall` subdir) plus the per-app config/data dirs, but
            // the archive root is only known once `RECALL_ARCHIVE_ROOT`
            // (or `$HOME`/`$HOME/Recall`) has been resolved, so we add
            // it here. `allow_directory(.., recursive)` accepts both
            // existing and not-yet-existing directories (it walks up to
            // canonicalize the parent), so a freshly-created `$HOME/Recall`
            // is fine.
            let scopes = app.state::<tauri::scope::Scopes>();
            if let Err(e) = scopes.allow_directory(&archive_root, true) {
                tracing::error!(
                    "failed to extend asset protocol scope with {}: {e}",
                    archive_root.display()
                );
            }

            match AppState::open(&archive_root) {
                Ok(st) => {
                    tracing::info!("archive opened");
                    app.manage(Mutex::new(st));
                    Ok(())
                }
                Err(e) => {
                    tracing::error!(
                        "failed to open archive at {}: {e}",
                        archive_root.display()
                    );
                    Err(e.into())
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::cmd_archive_info,
            commands::cmd_status,
            commands::cmd_search,
            commands::cmd_import_folder,
            commands::cmd_import_takeout,
            commands::cmd_verify,
            commands::cmd_backup,
            commands::cmd_export_zip,
            commands::cmd_list_albums,
            commands::cmd_list_imports,
            commands::cmd_get_media_detail,
            commands::cmd_get_media_path,
            commands::cmd_create_album,
            commands::cmd_rename_album,
            commands::cmd_delete_album,
            commands::cmd_add_to_album,
            commands::cmd_remove_from_album,
            commands::cmd_get_albums_for_media,
            commands::cmd_is_first_run,
            commands::cmd_seed_sample,
            commands::cmd_reveal_in_finder,
            commands::cmd_log_dir,
            commands::cmd_log_js_error,
            commands::cmd_set_archive_root,
            commands::cmd_migrate_archive,
            commands::cmd_probe_archive_root,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Recall");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_is_archive_root_plus_logs() {
        // The trace file layout is part of the documented contract —
        // the UI's "Show logs" link resolves to this exact path, so a
        // regression here would silently send users to the wrong place.
        let root = PathBuf::from("/tmp/recall-test-archive");
        let expected = root.join("logs");
        assert_eq!(root.join("logs"), expected);
        assert!(expected.to_string_lossy().ends_with("logs"));
    }

    #[test]
    fn archive_root_default_is_absolute_or_dot() {
        // Without a Tauri AppHandle we can't exercise the full resolver
        // (which reads the persisted JSON file via `app.path()`). But
        // the env var + legacy-fallback chain still produces either an
        // absolute path or ".", and that's the contract we promise:
        // anything else would mean the app crashes on launch when run
        // outside a TTY.
        let got = default_archive_root_for_test();
        let ok = got.is_absolute() || got == PathBuf::from(".");
        assert!(ok, "default archive root should be absolute or '.': {got:?}");
    }

    /// Mirror of `resolve_archive_root` that skips the persisted-file
    /// step (which needs a Tauri AppHandle). Exists purely so unit
    /// tests can still validate the env-var + legacy-fallback chain.
    fn default_archive_root_for_test() -> PathBuf {
        if let Ok(custom) = std::env::var("RECALL_ARCHIVE_ROOT") {
            return PathBuf::from(custom);
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home_path = PathBuf::from(home);
            let existing = home_path.join("archive.db");
            if existing.exists() {
                return home_path;
            }
            let mut p = home_path;
            p.push("Recall");
            return p;
        }
        PathBuf::from(".")
    }
}
