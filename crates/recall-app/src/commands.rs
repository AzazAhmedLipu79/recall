//! Tauri command surface for the Recall UI.
//!
//! Every public function here is `#[tauri::command]`-annotated in
//! `main.rs`. They wrap engine calls, returning either a typed
//! payload (serialized via serde to the frontend) or `Err(String)` on
//! failure — Tauri expects this signature.
//!
//! Note: we don't name commands after the engine functions because
//! that confuses Tauri's macro (and would shadow `use
//! recall_importer::…`). Each is prefixed `cmd_`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{ipc::Channel, Manager, State};

use recall_backup::{BackupStats, ExportStats, LocalDestination};
use recall_core::{AlbumId, ArchivePath, MediaId, Progress, ProgressEvent, RunKind};
use recall_db::{
    AlbumKind, AlbumRow, ImportRow, LocationRow, MediaCounts, MediaFileRow, MediaMetadataRow,
    MediaRow, SearchFilter, TimestampRow,
};
use recall_importer::{
    import_folder as engine_import_folder, import_takeout as engine_import_takeout, seed,
    ImportStats,
};
use recall_verify::verify_all;

use crate::state::AppState;

// ---------------------------------------------------------------------------
// ChannelProgress: a Progress impl that forwards into a tauri Channel.
// ---------------------------------------------------------------------------

struct ChannelProgress {
    channel: Channel<ProgressEvent>,
}

impl Progress for ChannelProgress {
    fn on_event(&self, event: &ProgressEvent) {
        // Best-effort: if the frontend closed mid-import, the channel
        // is gone and we drop the event silently.
        let _ = self.channel.send(event.clone());
    }
}

fn lock_state<'a>(
    state: &'a State<'_, std::sync::Mutex<AppState>>,
) -> Result<std::sync::MutexGuard<'a, AppState>, String> {
    state.lock().map_err(|e| format!("state poisoned: {e}"))
}

// ---------------------------------------------------------------------------
// DTOs that are stable across the IPC boundary.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ArchiveInfo {
    pub archive_root: String,
    pub db_path: String,
    pub store_root: String,
    /// Sum of object file sizes + archive.db + logs (best-effort;
    /// the walk is memoised on AppState with a 30s TTL so repeated
    /// statusbar ticks don't re-walk the tree).
    pub disk_usage_bytes: u64,
    /// `true` if the path was explicitly chosen by the user (wizard
    /// or "Move archive…") or by an env-var override — i.e. *not* a
    /// legacy fallback the host picked because nothing else was set.
    /// The statusbar badge shows a different hint when this is true.
    pub is_custom: bool,
    /// Media row count (handy for the Settings card's "n items" line
    /// so the UI doesn't have to fire a second `cmd_status` call).
    pub media_count: i64,
}

/// Cheap one-shot: report the archive paths. Always succeeds.
#[tauri::command]
pub fn cmd_archive_info<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, std::sync::Mutex<AppState>>,
) -> Result<ArchiveInfo, String> {
    let mut st = lock_state(&state)?;
    let disk_usage_bytes = st.disk_usage();
    let media_count = st.catalog().media_count().unwrap_or(0);
    let is_custom = archive_root_is_custom(&app, &st.archive_root);
    Ok(ArchiveInfo {
        archive_root: st.archive_root.to_string_lossy().into_owned(),
        db_path: st.db_path.to_string_lossy().into_owned(),
        store_root: st.store_root.to_string_lossy().into_owned(),
        disk_usage_bytes,
        is_custom,
        media_count,
    })
}

fn archive_root_is_custom<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    root: &PathBuf,
) -> bool {
    if std::env::var("RECALL_ARCHIVE_ROOT").is_ok() {
        return true;
    }
    if let Some(s) = super::read_persisted_archive_root(app) {
        return s == *root;
    }
    false
}

/// What we found at a user-picked path. Used by the Settings
/// "Switch archive…" picker to decide whether to switch to an
/// existing archive, copy into the folder, or treat it as an
/// empty target.
#[derive(Debug, Serialize)]
pub struct ArchiveProbe {
    /// Canonicalised absolute path the probe ran against.
    pub path: String,
    /// `true` if `archive.db` is present (alone or with WAL/SHM).
    pub has_archive_db: bool,
    /// `true` if `store/objects/` is present and non-empty.
    pub has_store: bool,
    /// `true` if the folder is missing or has none of the above.
    pub is_empty: bool,
    /// `true` if the picked folder is the user's currently active
    /// archive. Lets the UI short-circuit without a second IPC to
    /// `cmd_archive_info`.
    pub is_current: bool,
}

/// Probe a folder the user picked. Cheap: three `exists()`-class
/// checks plus a canonicalise. Returns a structured verdict so the
/// UI can offer a switch-vs-copy prompt instead of guessing.
#[tauri::command]
pub fn cmd_probe_archive_root(
    state: State<'_, std::sync::Mutex<AppState>>,
    path: String,
) -> Result<ArchiveProbe, String> {
    let p = PathBuf::from(path.trim());
    if p.as_os_str().is_empty() {
        return Err("Path cannot be empty".into());
    }
    // canonicalize errors if the path doesn't exist — treat that as
    // an empty target instead of failing the probe, so the UI can
    // short-circuit into the copy-into flow.
    let canon = match std::fs::canonicalize(&p) {
        Ok(c) => c,
        // Only swallow NotFound; surface permission / IO errors.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ArchiveProbe {
            path: p.to_string_lossy().into_owned(),
            has_archive_db: false,
            has_store: false,
            is_empty: true,
            is_current: false,
        }),
        Err(e) => return Err(format!("could not resolve {}: {e}", p.display())),
    };
    let has_archive_db = canon.join("archive.db").exists();
    let store = canon.join("store");
    let has_store = store.is_dir()
        && std::fs::read_dir(&store)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
    let is_empty = !has_archive_db && !has_store;
    let current = lock_state(&state)?.archive_root.clone();
    Ok(ArchiveProbe {
        path: canon.to_string_lossy().into_owned(),
        has_archive_db,
        has_store,
        is_empty,
        is_current: canon == current,
    })
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct SearchArgs {
    pub kind: Option<String>,
    pub source: Option<String>,
    pub album: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub has_location: Option<bool>,
    pub near_lat: Option<f64>,
    pub near_lon: Option<f64>,
    pub radius_km: Option<f64>,
    pub text: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ImportSummary {
    pub discovered: i64,
    pub imported: i64,
    pub duplicates: i64,
    pub failed: i64,
}

impl From<ImportStats> for ImportSummary {
    fn from(s: ImportStats) -> Self {
        Self {
            discovered: s.discovered,
            imported: s.imported,
            duplicates: s.duplicates,
            failed: s.failed,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BackupSummary {
    pub files_copied: i64,
    pub files_skipped: i64,
    pub files_failed: i64,
    pub bytes_copied: i64,
}

impl From<BackupStats> for BackupSummary {
    fn from(s: BackupStats) -> Self {
        Self {
            files_copied: s.files_copied,
            files_skipped: s.files_skipped,
            files_failed: s.files_failed,
            bytes_copied: s.bytes_copied,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ExportSummary {
    pub files_copied: i64,
    pub files_failed: i64,
    pub bytes_written: i64,
}

impl From<ExportStats> for ExportSummary {
    fn from(s: ExportStats) -> Self {
        Self {
            files_copied: s.files_copied,
            files_failed: s.files_failed,
            bytes_written: s.bytes_written,
        }
    }
}

// ---------------------------------------------------------------------------
// Commands.
// ---------------------------------------------------------------------------

/// Cheap one-shot: report the archive paths. Always succeeds.
#[tauri::command]
pub fn cmd_status(state: State<'_, std::sync::Mutex<AppState>>) -> Result<MediaCounts, String> {
    let mut st = lock_state(&state)?;
    st.catalog().media_counts().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cmd_search(
    state: State<'_, std::sync::Mutex<AppState>>,
    args: SearchArgs,
) -> Result<Vec<MediaRow>, String> {
    let mut st = lock_state(&state)?;
    let filter = SearchFilter {
        kind: args
            .kind
            .as_deref()
            .and_then(|s| recall_core::MediaKind::from_str(s)),
        source: args.source,
        album: args.album,
        from: args.from,
        to: args.to,
        has_location: args.has_location.unwrap_or(false),
        near_latitude: args.near_lat,
        near_longitude: args.near_lon,
        radius_km: args.radius_km,
        text: args.text,
        limit: args.limit,
    };
    st.catalog()
        .search_media(&filter)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cmd_list_albums(
    state: State<'_, std::sync::Mutex<AppState>>,
) -> Result<Vec<AlbumRow>, String> {
    let mut st = lock_state(&state)?;
    st.catalog().list_albums().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cmd_list_imports(
    state: State<'_, std::sync::Mutex<AppState>>,
    limit: Option<i64>,
) -> Result<Vec<ImportRow>, String> {
    let mut st = lock_state(&state)?;
    st.catalog()
        .list_imports(limit.unwrap_or(50))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cmd_import_folder(
    state: State<'_, std::sync::Mutex<AppState>>,
    channel: Channel<ProgressEvent>,
    source: String,
    label: Option<String>,
) -> Result<ImportSummary, String> {
    let src = PathBuf::from(&source);
    let label = label.unwrap_or_else(|| {
        src.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "folder".into())
    });
    tracing::info!(label = %label, source = %src.display(), "import folder: start");

    let AppState { conn, store, .. } = &mut *lock_state(&state)?;
    let (mut conn, store) = (&mut *conn, &*store);
    let progress = ChannelProgress {
        channel: channel.clone(),
    };

    let _ = channel.send(ProgressEvent::Started {
        kind: RunKind::ImportFolder,
        total: None,
    });

    let result = engine_import_folder(&mut conn, &store, &src, &label, Some(&progress));

    match result {
        Ok(stats) => {
            let s: ImportSummary = stats.into();
            tracing::info!(
                label = %label,
                imported = s.imported,
                duplicates = s.duplicates,
                failed = s.failed,
                "import folder: done"
            );
            let _ = channel.send(ProgressEvent::Finished {
                kind: RunKind::ImportFolder,
                ok: (s.imported + s.duplicates) as u64,
                failed: s.failed as u64,
                skipped: 0,
            });
            Ok(s)
        }
        Err(e) => {
            tracing::error!(label = %label, "import folder: failed: {e}");
            let _ = channel.send(ProgressEvent::FileFailed {
                path: src.clone(),
                reason: e.to_string(),
            });
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub fn cmd_import_takeout(
    state: State<'_, std::sync::Mutex<AppState>>,
    channel: Channel<ProgressEvent>,
    source: String,
) -> Result<ImportSummary, String> {
    let src = PathBuf::from(&source);
    tracing::info!(source = %src.display(), "import takeout: start");

    let AppState { conn, store, .. } = &mut *lock_state(&state)?;
    let (mut conn, store) = (&mut *conn, &*store);
    let progress = ChannelProgress {
        channel: channel.clone(),
    };

    let _ = channel.send(ProgressEvent::Started {
        kind: RunKind::ImportTakeout,
        total: None,
    });

    match engine_import_takeout(&mut conn, &store, &src, Some(&progress)) {
        Ok(stats) => {
            let s: ImportSummary = stats.into();
            tracing::info!(
                imported = s.imported,
                duplicates = s.duplicates,
                failed = s.failed,
                "import takeout: done"
            );
            let _ = channel.send(ProgressEvent::Finished {
                kind: RunKind::ImportTakeout,
                ok: (s.imported + s.duplicates) as u64,
                failed: s.failed as u64,
                skipped: 0,
            });
            Ok(s)
        }
        Err(e) => {
            tracing::error!("import takeout: failed: {e}");
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub fn cmd_verify(
    state: State<'_, std::sync::Mutex<AppState>>,
    channel: Channel<ProgressEvent>,
) -> Result<recall_verify::VerifyStats, String> {
    let AppState { conn, store, .. } = &mut *lock_state(&state)?;
    let (mut conn, store) = (&mut *conn, &*store);
    let progress = ChannelProgress {
        channel: channel.clone(),
    };

    let _ = channel.send(ProgressEvent::Started {
        kind: RunKind::Verify,
        total: None,
    });
    tracing::info!("verify: start");

    let res = verify_all(&mut conn, &store, Some(&progress));
    if let Ok(stats) = &res {
        tracing::info!(
            checked = stats.files_checked,
            ok = stats.files_ok,
            missing = stats.files_missing,
            mismatched = stats.files_mismatched,
            "verify: done"
        );
        let _ = channel.send(ProgressEvent::Finished {
            kind: RunKind::Verify,
            ok: stats.files_ok as u64,
            failed: (stats.files_missing + stats.files_mismatched) as u64,
            skipped: 0,
        });
    } else if let Err(e) = &res {
        tracing::error!("verify: failed: {e}");
    }
    res.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cmd_backup(
    state: State<'_, std::sync::Mutex<AppState>>,
    channel: Channel<ProgressEvent>,
    destination: String,
) -> Result<BackupSummary, String> {
    let AppState { conn, store, .. } = &mut *lock_state(&state)?;
    let (mut conn, store) = (&mut *conn, &*store);
    let progress = ChannelProgress {
        channel: channel.clone(),
    };

    let dst_path = PathBuf::from(&destination);
    let label = destination.clone();
    let local = LocalDestination::new(&dst_path);
    let boxed: Box<dyn recall_backup::BackupDestination> = Box::new(local);

    let _ = channel.send(ProgressEvent::Started {
        kind: RunKind::Backup,
        total: None,
    });
    tracing::info!(label = %label, "backup: start");

    let res = recall_backup::backup_to(
        &mut conn,
        &store,
        &label,
        &*boxed,
        Some(&progress),
    );
    if let Ok(stats) = &res {
        tracing::info!(
            copied = stats.files_copied,
            skipped = stats.files_skipped,
            failed = stats.files_failed,
            bytes = stats.bytes_copied,
            "backup: done"
        );
        let _ = channel.send(ProgressEvent::Finished {
            kind: RunKind::Backup,
            ok: (stats.files_copied + stats.files_skipped) as u64,
            failed: stats.files_failed as u64,
            skipped: stats.files_skipped as u64,
        });
    } else if let Err(e) = &res {
        tracing::error!("backup: failed: {e}");
    }
    res.map(|s| s.into()).map_err(|e| e.to_string())
}

/// Bundle every archived object plus `archive.db` into a single
/// deflate-compressed `.zip` at the user-supplied path.
#[tauri::command]
pub fn cmd_export_zip(
    state: State<'_, std::sync::Mutex<AppState>>,
    channel: Channel<ProgressEvent>,
    destination: String,
) -> Result<ExportSummary, String> {
    let dst = PathBuf::from(&destination);

    // Hold the AppState mutex for the duration of the export. Exports
    // are user-initiated and exclusive — there's no value in letting
    // a different command interleave while we're streaming a multi-GB
    // zip.
    let AppState { conn, store, db_path, .. } = &mut *lock_state(&state)?;
    let (conn, store, db_path) = (&mut *conn, &*store, &*db_path);
    let progress = ChannelProgress { channel: channel.clone() };

    let _ = channel.send(ProgressEvent::Started {
        kind: RunKind::ExportZip,
        total: None,
    });
    tracing::info!(destination = %dst.display(), "export zip: start");

    let res = recall_backup::export_zip_to(
        conn,
        store,
        db_path,
        &dst,
        Some(&progress),
    );
    if let Ok(stats) = &res {
        tracing::info!(
            copied = stats.files_copied,
            failed = stats.files_failed,
            bytes = stats.bytes_written,
            "export zip: done"
        );
        let _ = channel.send(ProgressEvent::Finished {
            kind: RunKind::ExportZip,
            ok: stats.files_copied as u64,
            failed: stats.files_failed as u64,
            skipped: 0,
        });
    } else if let Err(e) = &res {
        tracing::error!("export zip: failed: {e}");
    }
    res.map(|s| s.into()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Detail + thumbnail: power the inspector panel and the photo grid.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct MediaDetail {
    pub media: MediaRow,
    pub metadata: Option<MediaMetadataRow>,
    pub timestamps: Vec<TimestampRow>,
    pub locations: Vec<LocationRow>,
    pub albums: Vec<AlbumRow>,
    pub source_files: Vec<MediaFileRow>,
}

/// Fetch everything we know about a single media item for the inspector.
#[tauri::command]
pub fn cmd_get_media_detail(
    state: State<'_, std::sync::Mutex<AppState>>,
    id: String,
) -> Result<Option<MediaDetail>, String> {
    let mut st = lock_state(&state)?;
    let cat = st.catalog();
    let media_id = MediaId::from(id);
    let media = match cat.get_media(&media_id).map_err(|e| e.to_string())? {
        Some(m) => m,
        None => return Ok(None),
    };
    let metadata = cat.get_metadata(&media_id).map_err(|e| e.to_string())?;
    let timestamps = cat.get_timestamps(&media_id).map_err(|e| e.to_string())?;
    let locations = cat.get_locations(&media_id).map_err(|e| e.to_string())?;
    let albums = cat.get_albums_for_media(&media_id).map_err(|e| e.to_string())?;
    let source_files = cat.get_source_files(&media_id).map_err(|e| e.to_string())?;
    Ok(Some(MediaDetail {
        media,
        metadata,
        timestamps,
        locations,
        albums,
        source_files,
    }))
}

// ---------------------------------------------------------------------------
// Media URL: resolve a media id to an absolute filesystem path the
// webview can stream via Tauri's built-in `asset://` protocol.
//
// We deliberately return a plain path (not a `recall://` URL) so we
// don't have to register a custom URL scheme handler — the previous
// handler triggered a race in wry 0.55.1's WKWebView bootstrap that
// crashed cold-launched apps with EXC_CRASH / SIGABRT. The asset
// protocol is built into Tauri and handles Range requests, MIME
// sniffing, and streaming for free.
// ---------------------------------------------------------------------------

/// Return the absolute on-disk path for a media item, if it exists.
///
/// The frontend then calls `convertFileSrc(absPath)` (window-level
/// Tauri helper) to convert the path into an `asset://localhost/...`
/// URL suitable for `<img src>` / `<video src>`. The asset protocol
/// scope in `tauri.conf.json` plus the runtime call in `main.rs` covers
/// the archive root, so the webview can stream any file under it.
#[tauri::command]
pub fn cmd_get_media_path(
    state: State<'_, std::sync::Mutex<AppState>>,
    id: String,
) -> Result<Option<String>, String> {
    let mut st = lock_state(&state)?;
    let cat = st.catalog();
    let media_id = MediaId::from(id);
    let row = match cat.get_media(&media_id).map_err(|e| e.to_string())? {
        Some(r) => r,
        None => return Ok(None),
    };
    let archive_path = ArchivePath(camino::Utf8PathBuf::from(row.archive_path.clone()));
    let abs = st.store.resolve(&archive_path);
    Ok(Some(abs.to_string_lossy().into_owned()))
}

// ---------------------------------------------------------------------------
// Album CRUD: powers the sidebar "+ New album" button, right-click rename /
// delete, and the inspector's "Add to album…" dropdown.
// ---------------------------------------------------------------------------

/// Create a new user album. Returns the new album's id so the frontend
/// can immediately attach media to it (no extra round-trip). Idempotent:
/// if a user album with the same name already exists, returns that
/// album's id instead of failing — friendlier for the sidebar's quick
/// create flow.
#[tauri::command]
pub fn cmd_create_album(
    state: State<'_, std::sync::Mutex<AppState>>,
    name: String,
) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Album name cannot be empty".into());
    }
    let mut st = lock_state(&state)?;
    let cat = st.catalog();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let id = cat
        .upsert_album_by_name(trimmed, AlbumKind::User, now)
        .map_err(|e| e.to_string())?;
    Ok(id.as_str().to_string())
}

/// Rename a user album. Returns `Err` if the album doesn't exist or is
/// not a user album (Takeout-derived albums can't be renamed — their
/// names come from their on-disk folder).
#[tauri::command]
pub fn cmd_rename_album(
    state: State<'_, std::sync::Mutex<AppState>>,
    id: String,
    name: String,
) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Album name cannot be empty".into());
    }
    let mut st = lock_state(&state)?;
    let cat = st.catalog();
    let album_id = AlbumId::from(id);
    let renamed = cat
        .rename_album(&album_id, trimmed)
        .map_err(|e| e.to_string())?;
    if !renamed {
        return Err(format!(
            "Album {} not found or is not a user album",
            album_id.as_str()
        ));
    }
    Ok(())
}

/// Delete an album. Membership rows in `album_items` are removed
/// automatically via the schema's `ON DELETE CASCADE`; media files
/// themselves are untouched.
#[tauri::command]
pub fn cmd_delete_album(
    state: State<'_, std::sync::Mutex<AppState>>,
    id: String,
) -> Result<(), String> {
    let mut st = lock_state(&state)?;
    let cat = st.catalog();
    let album_id = AlbumId::from(id);
    let deleted = cat.delete_album(&album_id).map_err(|e| e.to_string())?;
    if deleted == 0 {
        return Err(format!("Album {} not found", album_id.as_str()));
    }
    Ok(())
}

/// Attach a media item to an album. Idempotent (uses `INSERT OR REPLACE`
/// under the hood); safe to call from the dropdown without checking
/// current membership first.
#[tauri::command]
pub fn cmd_add_to_album(
    state: State<'_, std::sync::Mutex<AppState>>,
    album_id: String,
    media_id: String,
) -> Result<(), String> {
    let mut st = lock_state(&state)?;
    let cat = st.catalog();
    let album = AlbumId::from(album_id);
    let media = MediaId::from(media_id);
    cat.add_to_album(&album, &media, 0)
        .map_err(|e| e.to_string())
}

/// Detach a media item from an album. Silently succeeds when the
/// media wasn't actually in the album — the dropdown removes the row
/// either way.
#[tauri::command]
pub fn cmd_remove_from_album(
    state: State<'_, std::sync::Mutex<AppState>>,
    album_id: String,
    media_id: String,
) -> Result<(), String> {
    let mut st = lock_state(&state)?;
    let cat = st.catalog();
    let album = AlbumId::from(album_id);
    let media = MediaId::from(media_id);
    let _ = cat
        .remove_from_album(&album, &media)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// List the albums currently attached to a media item. Powers the
/// inspector's "In albums" chips and lets the dropdown pre-check the
/// albums the item already belongs to.
#[tauri::command]
pub fn cmd_get_albums_for_media(
    state: State<'_, std::sync::Mutex<AppState>>,
    media_id: String,
) -> Result<Vec<AlbumRow>, String> {
    let mut st = lock_state(&state)?;
    let cat = st.catalog();
    let media = MediaId::from(media_id);
    cat.get_albums_for_media(&media).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// First-run + sample seed.
// ---------------------------------------------------------------------------

/// Returns `true` when the archive contains zero media rows. The wizard
/// uses this on boot to decide whether to show the first-run flow.
#[tauri::command]
pub fn cmd_is_first_run(state: State<'_, std::sync::Mutex<AppState>>) -> Result<bool, String> {
    let mut st = lock_state(&state)?;
    let count = st.catalog().media_count().map_err(|e| e.to_string())?;
    Ok(count == 0)
}

/// Generate synthetic sample media and import it through the normal
/// `import_takeout` pipeline. Used by the first-run wizard to populate
/// an empty archive with ~20 demo photos so the library isn't blank.
///
/// `count` defaults to 20. Output goes under
/// `<archive_root>/seed-tmp/` so the synthetic files don't pollute the
/// user's actual photo folder; the importer pulls them in, then the
/// temp folder is removed on success.
#[tauri::command]
pub fn cmd_seed_sample(
    state: State<'_, std::sync::Mutex<AppState>>,
    channel: Channel<ProgressEvent>,
    count: Option<usize>,
) -> Result<ImportSummary, String> {
    let n = count.unwrap_or(20);
    let mut st = lock_state(&state)?;
    let seed_dir = st.archive_root.join("seed-tmp");
    tracing::info!(count = n, "seed sample: start");

    let _ = channel.send(ProgressEvent::Started {
        kind: RunKind::ImportTakeout,
        total: Some(n as u64),
    });

    let AppState { conn, store, .. } = &mut *st;
    let (mut conn, store) = (&mut *conn, &*store);
    let progress = ChannelProgress {
        channel: channel.clone(),
    };

    let seed_result = seed::run(&mut conn, store, &seed_dir, n, 42, true, Some(&progress));

    // Clean up the temp seed directory regardless of outcome; if the
    // import failed partway, leaving the synthetic files around just
    // fills the archive root with junk.
    let _ = std::fs::remove_dir_all(&seed_dir);

    let stats = seed_result.map_err(|e| {
        tracing::error!("seed sample: failed: {e}");
        e.to_string()
    })?;
    let summary = ImportSummary {
        discovered: n as i64,
        imported: stats.imported,
        duplicates: stats.duplicates,
        failed: stats.failed,
    };
    tracing::info!(
        generated = stats.generated,
        imported = summary.imported,
        duplicates = summary.duplicates,
        failed = summary.failed,
        "seed sample: done"
    );
    let _ = channel.send(ProgressEvent::Finished {
        kind: RunKind::ImportTakeout,
        ok: (summary.imported + summary.duplicates) as u64,
        failed: summary.failed as u64,
        skipped: 0,
    });
    Ok(summary)
}

// ---------------------------------------------------------------------------
// Misc helpers (Reveal in Finder + log path) for Settings.
// ---------------------------------------------------------------------------

/// Reveal a file or directory in the macOS Finder. On macOS uses
/// `open -R <path>`; on other platforms falls back to `open <path>`
/// (which opens the containing folder if given a file). Failures are
/// surfaced to the caller so the UI can toast an error.
#[tauri::command]
pub fn cmd_reveal_in_finder(path: String) -> Result<(), String> {
    use std::process::Command;
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("Path does not exist: {path}"));
    }
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg("-R").arg(&p);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(&p);
        c
    };
    cmd.spawn()
        .map_err(|e| format!("failed to reveal {path}: {e}"))?;
    Ok(())
}

/// Absolute path to the archive's logs directory (one file per day).
/// The UI uses this to render a "Show logs" link in Settings.
#[tauri::command]
pub fn cmd_log_dir(state: State<'_, std::sync::Mutex<AppState>>) -> Result<String, String> {
    let st = lock_state(&state)?;
    let dir = st.archive_root.join("logs");
    Ok(dir.to_string_lossy().into_owned())
}

/// Append a JS error to `<archive>/logs/js-errors.log`. Only called
/// when the user has explicitly opted in via Settings; the UI gates
/// the call so we never write without consent.
#[tauri::command]
pub fn cmd_log_js_error(
    state: State<'_, std::sync::Mutex<AppState>>,
    message: String,
    stack: Option<String>,
    url: Option<String>,
) -> Result<(), String> {
    let st = lock_state(&state)?;
    let dir = st.archive_root.join("logs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create logs dir: {e}"))?;
    let path = dir.join("js-errors.log");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    writeln!(f, "[{ts}] {message}").ok();
    if let Some(u) = url.as_deref() {
        writeln!(f, "    url: {u}").ok();
    }
    if let Some(s) = stack.as_deref() {
        for line in s.lines() {
            writeln!(f, "    {line}").ok();
        }
    }
    writeln!(f).ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// Archive root: switch and migrate in-place (no restart).
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct MigrationSummary {
    pub files_copied: i64,
    pub bytes_copied: i64,
    pub logs_copied: i64,
    pub db_bytes: i64,
    pub deleted_source: bool,
}

/// Move the archive in place: copy every object + the catalog DB + logs
/// to `new_root`, atomically swap the live `AppState` for one opened at
/// the new location, swap the asset-protocol scope, and (if
/// `delete_source`) remove the source directory.
///
/// Designed so the UI can fire-and-forget — the `Channel` streams
/// per-file progress events the same way imports do.
#[tauri::command]
pub fn cmd_migrate_archive<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, std::sync::Mutex<AppState>>,
    channel: Channel<ProgressEvent>,
    new_root: String,
    delete_source: bool,
) -> Result<MigrationSummary, String> {
    let new_root = canonicalize_user_path(&new_root)?;
    {
        let st = lock_state(&state)?;
        if st.archive_root == new_root {
            return Err(format!(
                "New archive location is the same as the current one ({})",
                new_root.display()
            ));
        }
    }

    let _ = channel.send(ProgressEvent::MigrationStarted {
        total_files_hint: None,
    });

    // Copy under a hold of the source AppState so SQLite doesn't shift
    // under our feet. The migration helper writes to a fresh
    // destination — we never touch the source bytes.
    let src_root: PathBuf;
    let summary: recall_backup::migrate::MigrationStats;
    {
        let mut st = lock_state(&state)?;
        src_root = st.archive_root.clone();
        let AppState { conn, store, .. } = &mut *st;
        let (conn, store) = (&mut *conn, &*store);
        summary = recall_backup::migrate::migrate_archive(
            conn,
            store,
            &src_root,
            &new_root,
            Some(&ChannelProgress {
                channel: channel.clone(),
            }),
        )
        .map_err(|e| {
            let _ = channel.send(ProgressEvent::MigrationFailed {
                reason: e.to_string(),
            });
            e.to_string()
        })?;
    }

    let verify_failures = {
        let st = lock_state(&state)?;
        recall_backup::migrate::sample_verify(&st.store, &new_root, 0.1).unwrap_or(0)
    };
    if verify_failures > 0 {
        // Don't trust a corrupt copy — leave the live archive alone so
        // the user can investigate.
        let _ = channel.send(ProgressEvent::MigrationFailed {
            reason: format!(
                "Verification failed: {verify_failures} objects don't match the source. \
                 The source archive is unchanged; pick a different destination."
            ),
        });
        return Err(format!(
            "Migration aborted: {verify_failures} objects failed verification"
        ));
    }

    apply_new_archive_root(&app, &state, &new_root)?;

    if delete_source {
        if let Err(e) = std::fs::remove_dir_all(&src_root) {
            tracing::warn!(
                "could not remove old archive at {}: {e}",
                src_root.display()
            );
        }
    }

    let summary = MigrationSummary {
        files_copied: summary.files_copied,
        bytes_copied: summary.bytes_copied,
        logs_copied: summary.logs_copied,
        db_bytes: summary.db_bytes,
        deleted_source: delete_source,
    };
    let _ = channel.send(ProgressEvent::MigrationFinished {
        files_copied: summary.files_copied as u64,
        bytes_copied: summary.bytes_copied as u64,
        deleted_source: delete_source,
    });
    tracing::info!(
        src = %src_root.display(),
        dst = %new_root.display(),
        files = summary.files_copied,
        bytes = summary.bytes_copied,
        "archive migrated"
    );
    Ok(summary)
}

/// Switch the live archive to `new_root` *without* copying — used by the
/// wizard's "Use this location →" path on first run when the destination
/// is empty. Equivalent to "Move archive…" with `delete_source: false`,
/// minus the copy step.
#[tauri::command]
pub fn cmd_set_archive_root<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, std::sync::Mutex<AppState>>,
    new_root: String,
) -> Result<(), String> {
    let new_root = canonicalize_user_path(&new_root)?;
    {
        let st = lock_state(&state)?;
        if st.archive_root == new_root {
            return Ok(()); // already there — idempotent.
        }
    }
    apply_new_archive_root(&app, &state, &new_root)
}

/// Persist + scope-extend + state-swap, in that order. Shared between
/// `cmd_set_archive_root` (in-place switch with no copy) and
/// `cmd_migrate_archive` (copy + verify, then this).
///
/// Tauri's `Scopes` API is additive-only: there's no `forget` for the
/// asset protocol (the runtime scope is merged with the static one
/// declared in tauri.conf.json). That's fine — allowing both roots is
/// harmless as long as the source directory isn't deleted (and the
/// caller opts into deletion).
fn apply_new_archive_root<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &State<'_, std::sync::Mutex<AppState>>,
    new_root: &Path,
) -> Result<(), String> {
    super::write_persisted_archive_root(app, new_root)
        .map_err(|e| format!("could not persist new archive root: {e}"))?;
    swap_state_to(
        app,
        state,
        new_root,
        Some(Box::new(|_old_root| {
            let scopes = app.state::<tauri::scope::Scopes>();
            if let Err(e) = scopes.allow_directory(new_root, true) {
                tracing::error!(
                    "failed to extend asset scope to {}: {e}",
                    new_root.display()
                );
            }
        })),
    )
}

/// Resolve a user-supplied path to an absolute, canonicalised path. We
/// create the directory if it doesn't exist yet (the wizard's
/// "pick a folder" UX lets users create new ones).
fn canonicalize_user_path(s: &str) -> Result<PathBuf, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("Archive path cannot be empty".into());
    }
    let p = PathBuf::from(trimmed);
    if !p.exists() {
        std::fs::create_dir_all(&p)
            .map_err(|e| format!("could not create {}: {e}", p.display()))?;
    }
    std::fs::canonicalize(&p)
        .map_err(|e| format!("could not resolve {}: {e}", p.display()))
}

/// Close the current `AppState` and replace it with one rooted at
/// `new_root`. The optional `before_swap` callback runs while the
/// old state is still live — used by the migrate command to update
/// the asset-protocol scope before the swap so the new directory is
/// already whitelisted by the time commands start reading from it.
fn swap_state_to<'a, R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &State<'_, std::sync::Mutex<AppState>>,
    new_root: &Path,
    before_swap: Option<Box<dyn FnOnce(&Path) + 'a>>,
) -> Result<(), String> {
    let old_root: PathBuf;
    {
        let st = lock_state(state)?;
        old_root = st.archive_root.clone();
        // Drop the guard before opening the new state — SQLite's
        // open() blocks if the same dir is busy. Note: the WAL file
        // is closed when the Connection is dropped, so the dir is
        // unlocked by the time we open it again below.
        drop(st);
    }

    if let Some(cb) = before_swap {
        cb(&old_root);
    }

    let new_state = AppState::open(new_root)
        .map_err(|e| format!("could not open new archive at {}: {e}", new_root.display()))?;

    // Tauri's `manage` only allows one instance per type. To swap
    // AppState in place we use `state::<Mutex<AppState>>()` and write
    // directly to the inner Mutex. This is the same pattern Tauri uses
    // internally for hot-reloadable singletons.
    let mutex = app.state::<std::sync::Mutex<AppState>>();
    let mut guard = mutex
        .lock()
        .map_err(|e| format!("state poisoned: {e}"))?;
    *guard = new_state;
    Ok(())
}
