//! Importer: discovery, hashing, metadata extraction, cataloging.
//!
//! Two flows:
//!
//! 1. **Google Takeout**: ZIP or extracted folder, where media files have
//!    corresponding sidecar JSON files (`<name>.json` next to `<name>`).
//!    Albums live under `Albums/<album-name>/<media>`. The sidecar JSON
//!    describes the Google Photos entry's timestamps, descriptions,
//!    geo, etc.
//! 2. **Plain folder / phone**: discover any file under the source root,
//!    filter by extension, deduplicate against the catalog, archive.
//!
//! Both flows write to the same `ObjectStore` + `Catalog`. The import is
//! idempotent: re-running on the same source produces zero net new media.
//!
//! ## Failure handling
//!
//! We never silently skip a file. Every source path produces an
//! `import_files` row in the catalog with `outcome` ∈
//! {imported, duplicate, failed} and a human-readable `error` if failed.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::Deserialize;

use recall_core::{ArchivePath, MediaId, MediaKind, Progress, ProgressEvent, Result, RunKind};
use recall_db::{
    AlbumKind, Catalog, ImportFileOutcome, ImportFileRow, ImportRow, ImportStatus,
    LocationRow, MediaFileRow, MediaMetadataRow, MediaRow, TimestampRow,
};
use recall_metadata as meta;
use recall_storage::ObjectStore;

// ---------------------------------------------------------------------------
// Google Photos sidecar JSON
// ---------------------------------------------------------------------------

/// Subset of the Google Photos JSON sidecar fields we care about. See PRD §7.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleSidecar {
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_views: Option<i64>,
    pub creation_time: Option<Timestamp>,
    pub photo_taken_time: Option<Timestamp>,
    pub geo_data: Option<Geo>,
    pub geo_data_exif: Option<Geo>,
    #[serde(default)]
    pub people: Vec<Person>,
    pub url: Option<String>,
    pub google_photos_origin: Option<Origin>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timestamp {
    pub timestamp: Option<String>,
    pub formatted: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Geo {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub name: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Origin {
    #[serde(default)]
    pub album_names: Vec<String>,
}

/// Convert Google Photos "RFC 3339 with subsecond" timestamp to unix seconds.
pub fn google_timestamp_secs(s: &str) -> Option<i64> {
    let dt = DateTime::parse_from_rfc3339(s).ok()?;
    Some(dt.timestamp())
}

// ---------------------------------------------------------------------------
// File classification
// ---------------------------------------------------------------------------

/// Heuristic list of media extensions we care about for full imports. Other
/// extensions are skipped silently during a Google Takeout import (Takeout
/// contains many non-media JSON files).
pub const MEDIA_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "webp", "gif", "tiff", "tif",
    "bmp", "raw", "cr2", "nef", "arw", "dng", "orf", "raf", "rw2", "pef",
    "mp4", "mov", "m4v", "avi", "mkv", "webm", "3gp",
    "mp3", "m4a", "wav", "flac", "aac", "ogg", "opus",
    "pdf", "doc", "docx",
];

fn looks_like_media(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .map(|s| MEDIA_EXTS.iter().any(|m| *m == s.as_str()))
        .unwrap_or(false)
}

/// Resolve a sidecar stem (e.g. `/.../img_0025` or `/.../img_0025.jpg`)
/// to the actual media file. Accepts the stem if it appears in
/// `known_media_paths` (ZIP case — virtual entry name not on disk) OR
/// if it exists on disk after trying every known media extension.
///
/// Used by [`pair_sidecars_from_paths`] for `img.jpg.json` filenames
/// where `with_extension("")` would wrongly strip `.jpg`.
fn resolve_media_path_any(
    stem: &Path,
    known_media_paths: &std::collections::HashSet<PathBuf>,
) -> Option<PathBuf> {
    // The stripped stem (e.g. `img.jpg`) might already be a media path.
    if known_media_paths.contains(stem) {
        return Some(stem.to_path_buf());
    }
    if stem.exists() {
        return Some(stem.to_path_buf());
    }
    for ext in MEDIA_EXTS {
        let candidate = stem.with_extension(ext);
        if known_media_paths.contains(&candidate) || candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Sidecar JSON pairing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct SidecarPair {
    /// Absolute path of the media file.
    pub media: PathBuf,
    /// Absolute path of the corresponding sidecar JSON, if any.
    pub sidecar: Option<PathBuf>,
    /// The album name this file belongs to (from path under "Albums/"), if any.
    pub album: Option<String>,
}

/// Walk a directory and produce `SidecarPair`s by matching each media file
/// to its closest sibling `<basename>.json`. If the file lives under
/// `Albums/<album-name>/`, that album is recorded.
pub fn pair_sidecars(root: &Path) -> Result<Vec<SidecarPair>> {
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            paths.push(entry.path().to_path_buf());
        }
    }
    Ok(pair_sidecars_from_paths(root, &paths))
}

/// Pure path-based variant of [`pair_sidecars`]. Used by the ZIP importer
/// to derive sidecar/album associations from virtual entry names.
pub fn pair_sidecars_from_paths(
    root: &Path,
    paths: &[PathBuf],
) -> Vec<SidecarPair> {
    // Two passes:
    //   1. Collect every media path so JSON sidecars can resolve their
    //      sibling even when the media path doesn't exist on disk
    //      (ZIP virtual paths).
    //   2. Walk again, building SidecarPairs.
    let mut media_paths: std::collections::HashSet<PathBuf> =
        std::collections::HashSet::new();
    for p in paths {
        if looks_like_media(p) {
            media_paths.insert(p.clone());
        }
    }

    let mut pairs = BTreeMap::<PathBuf, SidecarPair>::new();

    for p in paths {
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if ext == "json" {
            // The media lives next to the sidecar with the same stem
            // plus a media extension (`.jpg`, `.mp4`, ...). Compute
            // the stem by stripping only ".json" — not by
            // `with_extension("")` which would also strip `.jpg` from
            // names like `img.jpg.json`.
            let stem_no_json = match p.file_name().and_then(|s| s.to_str()) {
                Some(name) if name.to_ascii_lowercase().ends_with(".json") => {
                    p.with_file_name(&name[..name.len() - 5])
                }
                _ => p.with_extension(""),
            };
            // Resolve to the media file: either on disk OR in the
            // same input list (ZIP case).
            let resolved_media = resolve_media_path_any(&stem_no_json, &media_paths);
            if let Some(media_path) = resolved_media {
                let pair = pairs.entry(media_path.clone()).or_insert_with(|| SidecarPair {
                    media: media_path,
                    sidecar: None,
                    album: derive_album(p, root),
                });
                pair.sidecar = Some(p.clone());
            }
            continue;
        }
        if looks_like_media(p) {
            let album = derive_album(p, root);
            let pair = pairs.entry(p.clone()).or_insert_with(|| SidecarPair {
                media: p.clone(),
                sidecar: None,
                album: album.clone(),
            });
            if pair.album.is_none() && album.is_some() {
                pair.album = album;
            }
        }
    }

    pairs.into_values().collect()
}

/// If a file path lies under `<root>/Albums/<album-name>/...`, return the
/// album name; otherwise None.
pub fn derive_album(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    // Filter out "." components (current directory).
    let comps: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .filter(|s| *s != "." && *s != "/")
        .collect();
    let first = comps.first()?;
    if first.eq_ignore_ascii_case("Albums") || first.eq_ignore_ascii_case("Album") {
        return comps.get(1).map(|s| s.to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// Plain folder import (used by phone + manual flows)
// ---------------------------------------------------------------------------

/// Discover media files in a folder tree. Returns absolute paths.
pub fn discover_folder(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() && looks_like_media(entry.path()) {
            out.push(entry.path().to_path_buf());
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Per-file import pipeline
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FileOutcome {
    pub source_path: PathBuf,
    pub media_id: Option<MediaId>,
    pub outcome: ImportFileOutcome,
    pub error: Option<String>,
}

/// Import a single source file. Used by both the Takeout and folder flows.
///
/// 1. Compute SHA-256 of `source` via the object store (which hashes as
///    it copies).
/// 2. Check `catalog.hash_exists`. If found, register an additional
///    `media_files` row instead of duplicating.
/// 3. Otherwise extract metadata, insert a `media` row, write sidecar if
///    provided.
///
/// `original_path_override`, if set, replaces `source` in the recorded
/// `media_files.original_path`. The physical `source` is still read for
/// hashing + metadata extraction. Used by the ZIP importer to record the
/// zip entry's path while extracting to a temp file for I/O.
///
/// `progress`, if provided, receives a [`ProgressEvent`] describing each
/// step (imported / duplicate / failed). The engine never blocks on the
/// callback; pass `None` to disable.
pub fn import_file(
    conn: &mut Connection,
    store: &ObjectStore,
    source: &Path,
    source_label: &str,
    import_id: &str,
    sidecar: Option<&Path>,
    original_path_override: Option<&str>,
    progress: Option<&dyn Progress>,
) -> FileOutcome {
    let report = |ev: &ProgressEvent| {
        if let Some(p) = progress {
            p.on_event(ev);
        }
    };
    // Archive into the object store. This *reads* the source but never
    // writes to it — original preservation guaranteed.
    let hash = match store.import(source) {
        Ok(h) => h,
        Err(e) => {
            let out = FileOutcome {
                source_path: source.to_path_buf(),
                media_id: None,
                outcome: ImportFileOutcome::Failed,
                error: Some(format!("{e}")),
            };
            report(&ProgressEvent::FileFailed {
                path: source.to_path_buf(),
                reason: format!("{e}"),
            });
            return out;
        }
    };

    // Use a single Catalog for the whole pipeline so we don't have to
    // juggle &mut borrows in nested scopes.
    let cat = Catalog::new(conn);

    if cat.hash_exists(&hash).unwrap_or(false) {
        // Duplicate: link the source to the existing media row.
        let media = match cat.media_by_hash(&hash).ok().flatten() {
            Some(m) => m,
            None => {
                let out = FileOutcome {
                    source_path: source.to_path_buf(),
                    media_id: None,
                    outcome: ImportFileOutcome::Failed,
                    error: Some("hash_exists true but media_by_hash None".into()),
                };
                report(&ProgressEvent::FileFailed {
                    path: source.to_path_buf(),
                    reason: out.error.clone().unwrap_or_default(),
                });
                return out;
            }
        };
        let (size, modified) = file_metadata(source);
        let recorded_path = original_path_override
            .map(|s| s.to_string())
            .unwrap_or_else(|| source.to_string_lossy().to_string());
        let _ = cat.add_media_file(&MediaFileRow {
            media_id: media.id.clone(),
            original_path: recorded_path,
            device_id: None,
            import_id: Some(import_id.to_string()),
            size_bytes: size,
            modified_at: modified,
        });
        report(&ProgressEvent::FileDuplicate {
            path: source.to_path_buf(),
            media_id: media.id.as_str().to_string(),
        });
        return FileOutcome {
            source_path: source.to_path_buf(),
            media_id: Some(media.id),
            outcome: ImportFileOutcome::Duplicate,
            error: None,
        };
    }

    // New content: insert a fresh media row.
    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = MediaKind::from_extension(&ext);

    let ex = meta::extract(source).unwrap_or_default();
    let media_id = MediaId::new(format!("m_{}", &hash[..16.min(hash.len())]));
    let archive_path = ArchivePath::from_hash(&hash);
    let (size, modified) = file_metadata(source);
    let created = std::fs::metadata(source)
        .and_then(|m| m.created())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);

    let canonical_taken_at = ex.taken_at.or(modified).or(created);

    let original_filename = original_path_override
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|s| s.to_string_lossy().to_string())
        .or_else(|| source.file_name().map(|s| s.to_string_lossy().to_string()));
    let row = MediaRow {
        id: media_id.clone(),
        kind,
        sha256: hash.clone(),
        size_bytes: size,
        mime_type: ex.mime_type.clone(),
        original_filename,
        archive_path: archive_path.0.to_string(),
        canonical_taken_at,
        created_at: created,
        modified_at: modified,
        source_label: source_label.to_string(),
    };
    if let Err(e) = cat.insert_media(&row) {
        let out = FileOutcome {
            source_path: source.to_path_buf(),
            media_id: None,
            outcome: ImportFileOutcome::Failed,
            error: Some(format!("db insert_media: {e}")),
        };
        report(&ProgressEvent::FileFailed {
            path: source.to_path_buf(),
            reason: out.error.clone().unwrap_or_default(),
        });
        return out;
    }
    let recorded_path = original_path_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| source.to_string_lossy().to_string());
    let _ = cat.add_media_file(&MediaFileRow {
        media_id: media_id.clone(),
        original_path: recorded_path,
        device_id: None,
        import_id: Some(import_id.to_string()),
        size_bytes: size,
        modified_at: modified,
    });
    if let Err(e) = cat.insert_metadata(&MediaMetadataRow {
        media_id: media_id.clone(),
        camera_make: ex.camera_make.clone(),
        camera_model: ex.camera_model.clone(),
        lens: ex.lens.clone(),
        focal_length: ex.focal_length,
        iso: ex.iso,
        aperture: ex.aperture,
        shutter_speed: ex.shutter_speed.clone(),
        orientation: ex.orientation,
        width: ex.width,
        height: ex.height,
        duration_seconds: ex.duration_seconds,
        description: ex.description.clone(),
    }) {
        let out = FileOutcome {
            source_path: source.to_path_buf(),
            media_id: Some(media_id.clone()),
            outcome: ImportFileOutcome::Failed,
            error: Some(format!("db insert_metadata: {e}")),
        };
        report(&ProgressEvent::FileFailed {
            path: source.to_path_buf(),
            reason: out.error.clone().unwrap_or_default(),
        });
        return out;
    }
    // Provenance timestamps.
    if let Some(t) = ex.taken_at {
        let _ = cat.insert_timestamp(&TimestampRow {
            media_id: media_id.clone(),
            source: if ex.parser.contains("exif") {
                "exif".into()
            } else if ex.parser.contains("mp4") {
                "container".into()
            } else {
                "filesystem".into()
            },
            value: t,
            label: Some("taken_at".into()),
        });
    }
    if let Some(t) = modified {
        let _ = cat.insert_timestamp(&TimestampRow {
            media_id: media_id.clone(),
            source: "filesystem".into(),
            value: t,
            label: Some("mtime".into()),
        });
    }

    // GPS.
    if let (Some(lat), Some(lon)) = (ex.latitude, ex.longitude) {
        let _ = cat.insert_location(&LocationRow {
            media_id: media_id.clone(),
            latitude: lat,
            longitude: lon,
            altitude: ex.altitude,
            source: "exif".into(),
        });
    }

    // Sidecar JSON: parse, link album, record Google timestamp.
    let mut description = ex.description.clone();
    let mut taken_at: Option<i64> = ex.taken_at;
    let mut sidecar_error: Option<String> = None;
    if let Some(sc_path) = sidecar {
        match parse_sidecar_file(sc_path) {
            Ok(sc) => {
                if description.is_none() {
                    description = sc.description.clone();
                }
                if let Some(t) = sc
                    .photo_taken_time
                    .as_ref()
                    .and_then(|t| t.timestamp.as_deref())
                {
                    if let Some(s) = google_timestamp_secs(t) {
                        if taken_at.is_none() {
                            taken_at = Some(s);
                        }
                        let _ = cat.insert_timestamp(&TimestampRow {
                            media_id: media_id.clone(),
                            source: "google".into(),
                            value: s,
                            label: Some("photoTakenTime".into()),
                        });
                    }
                }
                if let Some(g) = sc.geo_data_exif.clone().or(sc.geo_data.clone()) {
                    if let (Some(lat), Some(lon)) = (g.latitude, g.longitude) {
                        let _ = cat.insert_location(&LocationRow {
                            media_id: media_id.clone(),
                            latitude: lat,
                            longitude: lon,
                            altitude: g.altitude,
                            source: "google".into(),
                        });
                    }
                }
                // Persist raw sidecar bytes for traceability.
                if let Ok(bytes) = std::fs::read(sc_path) {
                    let _ = store.write_sidecar(&hash, &bytes);
                }
            }
            Err(e) => {
                sidecar_error = Some(format!("sidecar parse: {e}"));
            }
        }
    }

    // Persist canonical taken_at.
    if let Some(t) = taken_at {
        let _ = cat.set_canonical_taken_at(&media_id, t);
    }

    // If the description changed (Google overrode it), re-insert the
    // metadata row so the description column reflects the final value.
    if description != ex.description {
        let _ = cat.insert_metadata(&MediaMetadataRow {
            media_id: media_id.clone(),
            camera_make: ex.camera_make.clone(),
            camera_model: ex.camera_model.clone(),
            lens: ex.lens.clone(),
            focal_length: ex.focal_length,
            iso: ex.iso,
            aperture: ex.aperture,
            shutter_speed: ex.shutter_speed.clone(),
            orientation: ex.orientation,
            width: ex.width,
            height: ex.height,
            duration_seconds: ex.duration_seconds,
            description,
        });
    }

    if let Some(err) = sidecar_error {
        let out = FileOutcome {
            source_path: source.to_path_buf(),
            media_id: Some(media_id),
            outcome: ImportFileOutcome::Failed,
            error: Some(err),
        };
        report(&ProgressEvent::FileFailed {
            path: source.to_path_buf(),
            reason: out.error.clone().unwrap_or_default(),
        });
        return out;
    }

    report(&ProgressEvent::FileImported {
        path: source.to_path_buf(),
        media_id: media_id.as_str().to_string(),
    });
    FileOutcome {
        source_path: source.to_path_buf(),
        media_id: Some(media_id),
        outcome: ImportFileOutcome::Imported,
        error: None,
    }
}

fn file_metadata(source: &Path) -> (i64, Option<i64>) {
    let md = match std::fs::metadata(source) {
        Ok(m) => m,
        Err(_) => return (0, None),
    };
    let size = md.len() as i64;
    let modified = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    (size, modified)
}

fn parse_sidecar_file(path: &Path) -> std::result::Result<GoogleSidecar, serde_json::Error> {
    let bytes = std::fs::read(path).map_err(|e| {
        serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::Other, e))
    })?;
    serde_json::from_slice(&bytes)
}

// ---------------------------------------------------------------------------
// Top-level import functions
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
pub struct ImportStats {
    pub discovered: i64,
    pub imported: i64,
    pub duplicates: i64,
    pub failed: i64,
}

fn begin_import(
    conn: &mut Connection,
    id: &str,
    source_kind: &str,
    source_path: &str,
    discovered: i64,
) -> Result<()> {
    let now_secs = Utc::now().timestamp();
    let cat = Catalog::new(conn);
    cat.begin_import(&ImportRow {
        id: id.to_string().into(),
        source_kind: source_kind.into(),
        source_path: source_path.to_string(),
        started_at: now_secs,
        finished_at: None,
        discovered,
        imported: 0,
        duplicates: 0,
        failed: 0,
        status: ImportStatus::Running.as_str().into(),
    })?;
    Ok(())
}

fn finalize_import(
    conn: &mut Connection,
    id: &str,
    source_kind: &str,
    source_path: &str,
    started_at: i64,
    stats: ImportStats,
) -> Result<()> {
    let cat = Catalog::new(conn);
    let status = if stats.failed > 0 {
        ImportStatus::Partial.as_str()
    } else {
        ImportStatus::Success.as_str()
    };
    cat.update_import(&ImportRow {
        id: id.to_string().into(),
        source_kind: source_kind.into(),
        source_path: source_path.to_string(),
        started_at,
        finished_at: Some(Utc::now().timestamp()),
        discovered: stats.discovered,
        imported: stats.imported,
        duplicates: stats.duplicates,
        failed: stats.failed,
        status: status.into(),
    })?;
    Ok(())
}

fn record_outcome(conn: &mut Connection, import_id: &str, o: &FileOutcome) -> Result<()> {
    let cat = Catalog::new(conn);
    cat.record_import_file(&ImportFileRow {
        import_id: import_id.to_string().into(),
        source_path: o.source_path.to_string_lossy().to_string(),
        media_id: o.media_id.clone(),
        outcome: o.outcome,
        error: o.error.clone(),
    })?;
    Ok(())
}

/// Import an extracted Google Takeout folder. Each media file is paired
/// with its sidecar JSON, albums are reconstructed, metadata normalized.
pub fn import_takeout(
    conn: &mut Connection,
    store: &ObjectStore,
    takeout_root: &Path,
    progress: Option<&dyn Progress>,
) -> Result<ImportStats> {
    let root = takeout_root.to_path_buf();
    let pairs = pair_sidecars(&root)?;
    let report = |ev: &ProgressEvent| {
        if let Some(p) = progress {
            p.on_event(ev);
        }
    };
    report(&ProgressEvent::Started {
        kind: RunKind::ImportTakeout,
        total: Some(pairs.len() as u64),
    });

    let now_nanos = Utc::now().timestamp_nanos_opt().unwrap_or_else(|| Utc::now().timestamp() * 1_000_000_000);
    let import_id = format!("imp_takeout_{now_nanos}");
    let started_at = Utc::now().timestamp();
    begin_import(
        conn,
        &import_id,
        "google_takeout",
        &root.to_string_lossy(),
        pairs.len() as i64,
    )?;

    let mut stats = ImportStats {
        discovered: pairs.len() as i64,
        imported: 0,
        duplicates: 0,
        failed: 0,
    };

    for pair in &pairs {
        let outcome = import_file(
            conn,
            store,
            &pair.media,
            "google_takeout",
            &import_id,
            pair.sidecar.as_deref(),
            None,
            progress,
        );
        record_outcome(conn, &import_id, &outcome)?;

        match outcome.outcome {
            ImportFileOutcome::Imported => stats.imported += 1,
            ImportFileOutcome::Duplicate => stats.duplicates += 1,
            ImportFileOutcome::Failed => stats.failed += 1,
        }

        // Albums.
        if let (ImportFileOutcome::Imported, Some(media_id), Some(album)) =
            (outcome.outcome, outcome.media_id.clone(), pair.album.clone())
        {
            let cat = Catalog::new(conn);
            let album_id = cat.upsert_album_by_name(&album, AlbumKind::GoogleTakeout, started_at)?;
            let _ = cat.add_to_album(&album_id, &media_id, 0);
        }
    }

    let finished = ProgressEvent::Finished {
        kind: RunKind::ImportTakeout,
        ok: stats.imported as u64,
        failed: stats.failed as u64,
        skipped: stats.duplicates as u64,
    };
    finalize_import(
        conn,
        &import_id,
        "google_takeout",
        &root.to_string_lossy(),
        started_at,
        stats,
    )?;
    report(&finished);

    Ok(stats)
}

/// Import a plain folder (e.g. iPhone DCIM dump). No sidecar JSON; just
/// discovery + dedup + archive.
pub fn import_folder(
    conn: &mut Connection,
    store: &ObjectStore,
    folder: &Path,
    source_label: &str,
    progress: Option<&dyn Progress>,
) -> Result<ImportStats> {
    let files = discover_folder(folder)?;
    let report = |ev: &ProgressEvent| {
        if let Some(p) = progress {
            p.on_event(ev);
        }
    };
    report(&ProgressEvent::Started {
        kind: RunKind::ImportFolder,
        total: Some(files.len() as u64),
    });
    let now_nanos = Utc::now().timestamp_nanos_opt().unwrap_or_else(|| Utc::now().timestamp() * 1_000_000_000);
    let started_at = Utc::now().timestamp();
    let import_id = format!("imp_folder_{now_nanos}");
    begin_import(
        conn,
        &import_id,
        "folder",
        &folder.to_string_lossy(),
        files.len() as i64,
    )?;

    let mut stats = ImportStats {
        discovered: files.len() as i64,
        imported: 0,
        duplicates: 0,
        failed: 0,
    };
    for f in &files {
        let outcome = import_file(conn, store, f, source_label, &import_id, None, None, progress);
        record_outcome(conn, &import_id, &outcome)?;
        match outcome.outcome {
            ImportFileOutcome::Imported => stats.imported += 1,
            ImportFileOutcome::Duplicate => stats.duplicates += 1,
            ImportFileOutcome::Failed => stats.failed += 1,
        }
    }

    let finished = ProgressEvent::Finished {
        kind: RunKind::ImportFolder,
        ok: stats.imported as u64,
        failed: stats.failed as u64,
        skipped: stats.duplicates as u64,
    };
    finalize_import(
        conn,
        &import_id,
        "folder",
        &folder.to_string_lossy(),
        started_at,
        stats,
    )?;
    report(&finished);
    Ok(stats)
}

// ---------------------------------------------------------------------------
// Takeout ZIP import
// ---------------------------------------------------------------------------

/// Import directly from a Google Takeout `.zip` archive without requiring
/// the user to extract it first.
///
/// Strategy:
/// 1. Open the zip; collect every regular-file entry's name into a virtual
///    file tree, then run `pair_sidecars_from_paths` so we get the same
///    `SidecarPair`s the folder flow would produce.
/// 2. For each media entry, extract its bytes to a temp file with the
///    right extension (so `meta::extract` works), then call the existing
///    `import_file` pipeline. The recorded `media_files.original_path`
///    is overridden to the zip entry's name so users can locate the
///    source later.
/// 3. Temp files are cleaned up at the end.
pub fn import_takeout_zip(
    conn: &mut Connection,
    store: &ObjectStore,
    zip_path: &Path,
    progress: Option<&dyn Progress>,
) -> Result<ImportStats> {
    use zip::ZipArchive;
    let report = |ev: &ProgressEvent| {
        if let Some(p) = progress {
            p.on_event(ev);
        }
    };

    let f = File::open(zip_path).map_err(|e| recall_core::Error::io(zip_path, e))?;
    let mut archive = ZipArchive::new(f)?;

    // 1. Build the virtual file list and pairing.
    let entries: Vec<(PathBuf, PathBuf)> = (0..archive.len())
        .filter_map(|i| {
            let entry = archive.by_index(i).ok()?;
            if entry.is_dir() {
                return None;
            }
            let name = entry.name().to_string();
            // Normalise: strip a common leading directory if every entry
            // shares one (e.g. "Takeout/"). This matches what an extracted
            // Takeout folder would look like.
            let p = PathBuf::from(&name);
            Some((p, PathBuf::from(name)))
        })
        .collect();

    // Detect a common leading directory.
    let common_prefix = common_path_prefix(&entries.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>());
    // Always use an empty root for the virtual pairing — after we strip the
    // common prefix, paths no longer share a "root" with any non-empty
    // prefix. An empty root keeps `derive_album` looking at the first
    // component of the virtual path, which is what we want.
    let virtual_root = PathBuf::new();
    let virtual_paths: Vec<PathBuf> = entries
        .iter()
        .map(|(p, _)| match &common_prefix {
            Some(prefix) => p.strip_prefix(prefix).unwrap_or(p).to_path_buf(),
            None => p.clone(),
        })
        .collect();
    let pairs = pair_sidecars_from_paths(&virtual_root, &virtual_paths);

    // Map pair.media (virtual path) back to the original zip entry name.
    let mut media_to_zip_name: std::collections::HashMap<PathBuf, String> =
        std::collections::HashMap::new();
    for (orig, _v) in &entries {
        let stripped = match &common_prefix {
            Some(prefix) => orig.strip_prefix(prefix).unwrap_or(orig).to_path_buf(),
            None => orig.clone(),
        };
        media_to_zip_name.insert(stripped, orig.to_string_lossy().to_string());
    }

    let now = Utc::now().timestamp();
    let now_nanos = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp() * 1_000_000_000);
    let import_id = format!("imp_takeout_zip_{now_nanos}");
    let started_at = now;

    report(&ProgressEvent::Started {
        kind: RunKind::ImportTakeoutZip,
        total: Some(pairs.len() as u64),
    });
    begin_import(
        conn,
        &import_id,
        "google_takeout_zip",
        &zip_path.to_string_lossy(),
        pairs.len() as i64,
    )?;

    let mut stats = ImportStats {
        discovered: pairs.len() as i64,
        imported: 0,
        duplicates: 0,
        failed: 0,
    };

    // Temp dir for extracted entries; cleaned up at the end.
    let temp_root = std::env::temp_dir().join(format!(
        "recall-zip-{}-{}",
        std::process::id(),
        now_nanos
    ));
    std::fs::create_dir_all(&temp_root).map_err(|e| recall_core::Error::io(&temp_root, e))?;

    for (idx, pair) in pairs.iter().enumerate() {
        let outcome = import_one_zip_entry(
            conn,
            store,
            &mut archive,
            pair,
            &media_to_zip_name,
            &temp_root,
            "google_takeout_zip",
            &import_id,
            idx,
            progress,
        );
        record_outcome(conn, &import_id, &outcome)?;
        match outcome.outcome {
            ImportFileOutcome::Imported => stats.imported += 1,
            ImportFileOutcome::Duplicate => stats.duplicates += 1,
            ImportFileOutcome::Failed => stats.failed += 1,
        }

        if let (ImportFileOutcome::Imported, Some(media_id), Some(album)) =
            (outcome.outcome, outcome.media_id.clone(), pair.album.clone())
        {
            let cat = Catalog::new(conn);
            let album_id = cat.upsert_album_by_name(&album, AlbumKind::GoogleTakeout, started_at)?;
            let _ = cat.add_to_album(&album_id, &media_id, 0);
        }
    }

    // Cleanup.
    let _ = std::fs::remove_dir_all(&temp_root);

    finalize_import(
        conn,
        &import_id,
        "google_takeout_zip",
        &zip_path.to_string_lossy(),
        started_at,
        stats,
    )?;
    report(&ProgressEvent::Finished {
        kind: RunKind::ImportTakeoutZip,
        ok: stats.imported as u64,
        failed: stats.failed as u64,
        skipped: stats.duplicates as u64,
    });
    Ok(stats)
}

/// Helper: extract one media entry to a temp file and call `import_file`.
/// On the duplicate path, we still need the bytes to compute the hash
/// (which the catalog already has), so we extract either way.
fn import_one_zip_entry<R: Read + std::io::Seek>(
    conn: &mut Connection,
    store: &ObjectStore,
    archive: &mut zip::ZipArchive<R>,
    pair: &SidecarPair,
    media_to_zip_name: &std::collections::HashMap<PathBuf, String>,
    temp_root: &Path,
    source_label: &str,
    import_id: &str,
    index: usize,
    progress: Option<&dyn Progress>,
) -> FileOutcome {
    let report = |ev: &ProgressEvent| {
        if let Some(p) = progress {
            p.on_event(ev);
        }
    };
    // Find the entry index by matching the (stripped) virtual path.
    let zip_name = media_to_zip_name
        .get(&pair.media)
        .cloned()
        .unwrap_or_else(|| pair.media.to_string_lossy().to_string());

    // Step 1: find the entry index. We need this before opening the entry
    // (which would borrow `archive` for the entry's full lifetime).
    let entry_idx = match find_entry_index(archive, &zip_name) {
        Some(i) => i,
        None => match find_entry_by_basename_index(archive, &pair.media) {
            Some(i) => i,
            None => {
                let out = FileOutcome {
                    source_path: pair.media.clone(),
                    media_id: None,
                    outcome: ImportFileOutcome::Failed,
                    error: Some(format!("zip entry not found: {zip_name}")),
                };
                report(&ProgressEvent::FileFailed {
                    path: pair.media.clone(),
                    reason: out.error.clone().unwrap_or_default(),
                });
                return out;
            }
        },
    };

    // Step 2: extract the media entry to a temp file.
    let ext = Path::new(&zip_name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("bin");
    let temp_name = format!("entry-{}-{:x}.{}", index, std::process::id(), ext);
    let temp_path = temp_root.join(&temp_name);
    {
        let mut entry = match archive.by_index(entry_idx) {
            Ok(e) => e,
            Err(e) => {
                let out = FileOutcome {
                    source_path: pair.media.clone(),
                    media_id: None,
                    outcome: ImportFileOutcome::Failed,
                    error: Some(format!("zip by_index: {e}")),
                };
                report(&ProgressEvent::FileFailed {
                    path: pair.media.clone(),
                    reason: out.error.clone().unwrap_or_default(),
                });
                return out;
            }
        };
        let mut out = match File::create(&temp_path) {
            Ok(f) => f,
            Err(e) => {
                let ret = FileOutcome {
                    source_path: pair.media.clone(),
                    media_id: None,
                    outcome: ImportFileOutcome::Failed,
                    error: Some(format!("create temp: {e}")),
                };
                report(&ProgressEvent::FileFailed {
                    path: pair.media.clone(),
                    reason: ret.error.clone().unwrap_or_default(),
                });
                return ret;
            }
        };
        if let Err(e) = std::io::copy(&mut entry, &mut out) {
            let ret = FileOutcome {
                source_path: pair.media.clone(),
                media_id: None,
                outcome: ImportFileOutcome::Failed,
                error: Some(format!("extract: {e}")),
            };
            report(&ProgressEvent::FileFailed {
                path: pair.media.clone(),
                reason: ret.error.clone().unwrap_or_default(),
            });
            return ret;
        }
        let _ = out.flush();
    }
    // entry dropped here, archive is free again.

    // Step 3: extract the sidecar if present.
    let sidecar_path = if let Some(sc_virtual) = pair.sidecar.as_ref() {
        let zip_sc = media_to_zip_name
            .get(sc_virtual)
            .cloned()
            .unwrap_or_else(|| sc_virtual.to_string_lossy().to_string());
        let sc_idx = find_entry_index(archive, &zip_sc);
        match sc_idx {
            Some(i) => {
                let mut e = match archive.by_index(i) {
                    Ok(e) => e,
                    Err(_) => return finish_with_sidecar_error(temp_path, pair),
                };
                let sc_ext = Path::new(&zip_sc)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("json");
                let sc_temp = temp_root.join(format!(
                    "sidecar-{}-{}.{}",
                    index,
                    std::process::id(),
                    sc_ext
                ));
                let mut f = match File::create(&sc_temp) {
                    Ok(f) => f,
                    Err(_) => return finish_with_sidecar_error(temp_path, pair),
                };
                if std::io::copy(&mut e, &mut f).is_err() {
                    return finish_with_sidecar_error(temp_path, pair);
                }
                Some(sc_temp)
            }
            None => None,
        }
    } else {
        None
    };

    let outcome = import_file(
        conn,
        store,
        &temp_path,
        source_label,
        import_id,
        sidecar_path.as_deref(),
        Some(&zip_name),
        progress,
    );

    // Cleanup temp files for this entry.
    let _ = std::fs::remove_file(&temp_path);
    if let Some(sc) = sidecar_path {
        let _ = std::fs::remove_file(&sc);
    }

    outcome
}

fn finish_with_sidecar_error(temp_path: PathBuf, pair: &SidecarPair) -> FileOutcome {
    let _ = std::fs::remove_file(&temp_path);
    FileOutcome {
        source_path: pair.media.clone(),
        media_id: None,
        outcome: ImportFileOutcome::Failed,
        error: Some("failed to extract sidecar".into()),
    }
}

/// Helper: scan zip entries for the first one whose name equals `target`.
/// Returns the entry index.
fn find_entry_index<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    target: &str,
) -> Option<usize> {
    for i in 0..archive.len() {
        let name = match archive.by_index(i) {
            Ok(e) => {
                if e.is_dir() {
                    continue;
                }
                e.name().to_string()
            }
            Err(_) => continue,
        };
        if name == target {
            return Some(i);
        }
    }
    None
}

/// Helper: scan zip entries for one whose basename matches `target`'s
/// basename. Returns the entry index.
fn find_entry_by_basename_index<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    target: &Path,
) -> Option<usize> {
    let target_name = target.file_name()?.to_string_lossy().to_string();
    for i in 0..archive.len() {
        // `by_index` with a fresh `i` borrows the archive mutably for the
        // duration of the call. We capture `name()` and drop the borrow
        // before the next iteration.
        let name = match archive.by_index(i) {
            Ok(e) => {
                if e.is_dir() {
                    continue;
                }
                e.name().to_string()
            }
            Err(_) => continue,
        };
        let p = Path::new(&name);
        if p.file_name().and_then(|s| s.to_str()) == Some(&target_name) {
            return Some(i);
        }
    }
    None
}

/// Find the top-level directory that every input path shares.
///
/// Google Takeout zips always wrap their content in a single top-level
/// directory (`Takeout/`, `Google Photos/`, etc.). Stripping just that
/// wrapper gives us a "virtual root" that matches an extracted folder
/// layout. Returns the top-level dir (e.g. `Takeout`), or None if the
/// paths don't share one.
fn common_path_prefix(paths: &[PathBuf]) -> Option<PathBuf> {
    let first = paths.first()?;
    let first_comp = first.components().next()?;
    for p in &paths[1..] {
        let comp = p.components().next()?;
        if comp != first_comp {
            return None;
        }
    }
    Some(PathBuf::from(first_comp.as_os_str()))
}

// ---------------------------------------------------------------------------
// Dedup: prune source paths that no longer exist on disk.
// ---------------------------------------------------------------------------

/// Walk every `media_files` row and remove any whose `original_path`
/// no longer exists. Returns the count of pruned rows.
///
/// This is safe: the actual archived object is preserved (we only drop
/// the source-path pointer). The `media` row stays put.
pub fn prune_missing_sources(conn: &mut Connection) -> Result<usize> {
    let cat = Catalog::new(conn);
    let all = cat.iter_all_media_files()?;
    drop(cat);
    let mut pruned = 0usize;
    for f in &all {
        if !std::path::Path::new(&f.original_path).exists() {
            let cat = Catalog::new(conn);
            cat.delete_media_file_by_path(&f.original_path)?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

// ---------------------------------------------------------------------------
// Synthetic sample generator.
// ---------------------------------------------------------------------------

pub mod seed;

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use recall_db::open as open_db;
    use std::fs::create_dir_all;

    fn write_jpeg_with_exif(path: &Path, _name: &str) {
        // Tiny placeholder JPEG so MediaKind classifies correctly.
        std::fs::write(path, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
    }

    fn write_unique_jpeg(path: &Path, salt: u8) {
        // Same JPEG header so MediaKind classifies, but unique bytes so
        // dedup doesn't collapse these.
        std::fs::write(path, [0xFF, 0xD8, 0xFF, salt, 0xD9]).unwrap();
    }

    #[test]
    fn pair_sidecars_finds_json_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let albums = dir.path().join("Albums").join("Family");
        create_dir_all(&albums).unwrap();
        std::fs::write(albums.join("a.jpg"), b"jpg").unwrap();
        std::fs::write(
            albums.join("a.jpg.json"),
            r#"{"title":"a","photoTakenTime":{"timestamp":"2025-08-14T17:42:31Z"}}"#,
        )
        .unwrap();
        std::fs::write(albums.join("b.jpg"), b"jpg2").unwrap();

        let pairs = pair_sidecars(dir.path()).unwrap();
        let a = pairs
            .iter()
            .find(|p| p.media.file_name().unwrap() == "a.jpg")
            .unwrap();
        assert_eq!(
            a.sidecar.as_ref().unwrap().file_name().unwrap(),
            "a.jpg.json"
        );
        assert_eq!(a.album.as_deref(), Some("Family"));
        let b = pairs
            .iter()
            .find(|p| p.media.file_name().unwrap() == "b.jpg")
            .unwrap();
        assert!(b.sidecar.is_none());
    }

    #[test]
    fn album_derivation_only_when_under_albums() {
        let dir = tempfile::tempdir().unwrap();
        let albums = dir.path().join("Albums").join("Travel");
        create_dir_all(&albums).unwrap();
        std::fs::write(albums.join("c.jpg"), b"jpg").unwrap();
        std::fs::write(dir.path().join("loose.jpg"), b"jpg").unwrap();
        let pairs = pair_sidecars(dir.path()).unwrap();
        let c = pairs
            .iter()
            .find(|p| p.media.file_name().unwrap() == "c.jpg")
            .unwrap();
        assert_eq!(c.album.as_deref(), Some("Travel"));
        let loose = pairs
            .iter()
            .find(|p| p.media.file_name().unwrap() == "loose.jpg")
            .unwrap();
        assert!(loose.album.is_none());
    }

    #[test]
    fn folder_import_dedups_on_second_run() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        // Source folder with three unique files, two identical duplicates.
        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);
        write_unique_jpeg(&src.join("b.jpg"), 2);
        std::fs::write(&src.join("c.jpg"), b"same").unwrap();
        std::fs::write(&src.join("d.jpg"), b"same").unwrap();

        let s1 = import_folder(&mut conn, &store, &src, "folder", None).unwrap();
        assert_eq!(s1.discovered, 4);
        assert_eq!(s1.imported, 3, "a, b unique; c and d share content");
        assert_eq!(s1.duplicates, 1);
        assert_eq!(s1.failed, 0);

        let s2 = import_folder(&mut conn, &store, &src, "folder", None).unwrap();
        assert_eq!(s2.discovered, 4);
        assert_eq!(s2.imported, 0);
        assert_eq!(s2.duplicates, 4);
    }

    #[test]
    fn takeout_import_links_albums_and_google_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let root = dir.path().join("Takeout");
        let albums = root.join("Albums").join("Vacation");
        create_dir_all(&albums).unwrap();

        let media = albums.join("IMG_001.jpg");
        let sidecar = albums.join("IMG_001.jpg.json");
        std::fs::write(&media, [0xFF, 0xD8, 0xAA, 0xBB, 0xCC, 0xD9]).unwrap();
        let sidecar_json = r#"{
            "title": "Beach",
            "description": "Sunny beach day",
            "photoTakenTime": { "timestamp": "2025-08-14T17:42:31Z" },
            "geoData": { "latitude": 47.6, "longitude": -122.3, "altitude": 10.0 }
        }"#;
        std::fs::write(&sidecar, sidecar_json).unwrap();

        // Also a top-level photo not in any album, with different bytes.
        std::fs::write(root.join("loose.jpg"), [0xFF, 0xD8, 0x11, 0x22, 0x33, 0xD9]).unwrap();

        let pairs = pair_sidecars(&root).unwrap();
        assert_eq!(pairs.len(), 2, "discovered two media files");
        let stats = import_takeout(&mut conn, &store, &root, None).unwrap();
        assert_eq!(stats.discovered, 2);
        assert_eq!(stats.imported, 2);
        assert_eq!(stats.failed, 0);

        // Albums listing should contain Vacation.
        {
            let mut cat = Catalog::new(&mut conn);
            let albums = cat.list_albums().unwrap();
            assert_eq!(albums.len(), 1);
            assert_eq!(albums[0].name, "Vacation");
        }
    }

    #[test]
    fn google_timestamp_parses() {
        let s = "2025-08-14T17:42:31Z";
        let secs = google_timestamp_secs(s);
        assert!(secs.is_some());
        assert!(secs.unwrap() > 0);
    }

    #[test]
    fn original_preservation_untouched() {
        // After import, the source file is unchanged.
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();
        let src = dir.path().join("src.jpg");
        let original = vec![0xFF, 0xD8, 0xFE, 0xED, 0xFA, 0xCE];
        std::fs::write(&src, &original).unwrap();
        let original_mtime = std::fs::metadata(&src).unwrap().modified().unwrap();

        let _ = import_folder(&mut conn, &store, dir.path(), "folder", None).unwrap();

        let after = std::fs::read(&src).unwrap();
        assert_eq!(after, original, "import modified source");
        let after_mtime = std::fs::metadata(&src).unwrap().modified().unwrap();
        assert_eq!(after_mtime, original_mtime, "import changed mtime");
    }

    #[test]
    fn media_kind_classification_by_extension() {
        assert_eq!(MediaKind::from_extension("jpg"), MediaKind::Photo);
        assert_eq!(MediaKind::from_extension("mp4"), MediaKind::Video);
        assert_eq!(MediaKind::from_extension("pdf"), MediaKind::Document);
    }

    #[test]
    fn prune_missing_sources_removes_deleted_paths() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);
        write_unique_jpeg(&src.join("b.jpg"), 2);
        write_unique_jpeg(&src.join("c.jpg"), 3);
        import_folder(&mut conn, &store, &src, "folder", None).unwrap();

        // Delete one of the source files. The archive object remains.
        std::fs::remove_file(src.join("b.jpg")).unwrap();

        let pruned = prune_missing_sources(&mut conn).unwrap();
        assert_eq!(pruned, 1);

        // The media row is still in the catalog (we never delete content
        // just because the source vanished). Source-path row is gone.
        let cat = Catalog::new(&mut conn);
        let files = cat.iter_all_media_files().unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.original_path != src.join("b.jpg").to_string_lossy().as_ref()));
    }

    #[test]
    fn prune_missing_sources_noop_when_all_present() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);
        import_folder(&mut conn, &store, &src, "folder", None).unwrap();

        let pruned = prune_missing_sources(&mut conn).unwrap();
        assert_eq!(pruned, 0);
    }

    // -----------------------------------------------------------------------
    // ZIP import tests
    // -----------------------------------------------------------------------

    fn write_zip_with_entries(path: &Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let f = File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(data).unwrap();
        }
        zw.finish().unwrap();
    }

    #[test]
    fn zip_import_dedups_on_second_run() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let zip_path = dir.path().join("takeout.zip");
        write_zip_with_entries(
            &zip_path,
            &[
                (
                    "Takeout/Albums/Vacation/IMG_001.jpg",
                    &[0xFF, 0xD8, 0xAA, 0xBB, 0xD9],
                ),
                (
                    "Takeout/Albums/Vacation/IMG_002.jpg",
                    &[0xFF, 0xD8, 0x11, 0x22, 0xD9],
                ),
                ("Takeout/loose.jpg", &[0xFF, 0xD8, 0x33, 0x44, 0xD9]),
            ],
        );

        let s1 = import_takeout_zip(&mut conn, &store, &zip_path, None).unwrap();
        assert_eq!(s1.discovered, 3);
        assert_eq!(s1.imported, 3);
        assert_eq!(s1.duplicates, 0);
        assert_eq!(s1.failed, 0);

        let s2 = import_takeout_zip(&mut conn, &store, &zip_path, None).unwrap();
        assert_eq!(s2.discovered, 3);
        assert_eq!(s2.imported, 0);
        assert_eq!(s2.duplicates, 3);
    }

    #[test]
    fn zip_import_records_original_zip_entry_path() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let zip_path = dir.path().join("takeout.zip");
        let entry_name = "Takeout/Albums/Vacation/IMG_001.jpg";
        write_zip_with_entries(&zip_path, &[(entry_name, &[0xFF, 0xD8, 0xCC, 0xD9])]);

        let stats = import_takeout_zip(&mut conn, &store, &zip_path, None).unwrap();
        assert_eq!(stats.imported, 1);

        let cat = Catalog::new(&mut conn);
        let files = cat.iter_all_media_files().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].original_path, entry_name);
        // The album should be recorded too.
        let albums = cat.list_albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Vacation");
    }

    #[test]
    fn zip_import_parses_google_sidecar_in_zip() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let zip_path = dir.path().join("takeout.zip");
        let sidecar_json = r#"{
            "title": "Beach",
            "description": "Sunny day",
            "photoTakenTime": { "timestamp": "2025-08-14T17:42:31Z" }
        }"#;
        write_zip_with_entries(
            &zip_path,
            &[
                (
                    "Takeout/IMG_001.jpg",
                    &[0xFF, 0xD8, 0xAA, 0xBB, 0xD9],
                ),
                ("Takeout/IMG_001.jpg.json", sidecar_json.as_bytes()),
            ],
        );

        let stats = import_takeout_zip(&mut conn, &store, &zip_path, None).unwrap();
        assert_eq!(stats.imported, 1);
        assert_eq!(stats.failed, 0);

        // Google sidecar should have produced a stored sidecar file in the
        // object store's metadata dir.
        let metadata_dir = store_root.join("metadata");
        let mut sidecar_count = 0;
        for entry in walkdir::WalkDir::new(&metadata_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                sidecar_count += 1;
            }
        }
        assert_eq!(sidecar_count, 1);
    }

    #[test]
    fn zip_import_skips_non_media_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let zip_path = dir.path().join("takeout.zip");
        write_zip_with_entries(
            &zip_path,
            &[
                ("Takeout/IMG_001.jpg", &[0xFF, 0xD8, 0xAA, 0xD9]),
                (
                    "Takeout/archive_index.txt",
                    b"some takeout metadata file",
                ),
                (
                    "Takeot/Photos from 2024/IMG_002.jpg",
                    &[0xFF, 0xD8, 0xBB, 0xD9],
                ),
            ],
        );

        let stats = import_takeout_zip(&mut conn, &store, &zip_path, None).unwrap();
        assert_eq!(stats.discovered, 2);
        assert_eq!(stats.imported, 2);
    }

    /// Collects progress events so a test can assert on them.
    struct CollectingProgress(std::sync::Mutex<Vec<ProgressEvent>>);
    impl CollectingProgress {
        fn new() -> Self {
            Self(std::sync::Mutex::new(Vec::new()))
        }
    }
    impl Progress for CollectingProgress {
        fn on_event(&self, event: &ProgressEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn folder_import_emits_progress_events() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);
        write_unique_jpeg(&src.join("b.jpg"), 2);

        let progress = CollectingProgress::new();
        let stats = import_folder(&mut conn, &store, &src, "folder", Some(&progress)).unwrap();
        assert_eq!(stats.imported, 2);

        let events = progress.0.lock().unwrap();
        // Started → 2 FileImported → Finished
        let kinds: Vec<&'static str> = events
            .iter()
            .map(|e| match e {
                ProgressEvent::Started { .. } => "started",
                ProgressEvent::FileImported { .. } => "imported",
                ProgressEvent::FileDuplicate { .. } => "duplicate",
                ProgressEvent::FileFailed { .. } => "failed",
                ProgressEvent::Finished { .. } => "finished",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds[0], "started");
        assert_eq!(*kinds.last().unwrap(), "finished");
        let imported_count = kinds.iter().filter(|k| **k == "imported").count();
        assert_eq!(imported_count, 2);
    }

    #[test]
    fn folder_import_idempotent_run_emits_duplicate_events() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);

        import_folder(&mut conn, &store, &src, "folder", None).unwrap();
        let progress = CollectingProgress::new();
        let s = import_folder(&mut conn, &store, &src, "folder", Some(&progress)).unwrap();
        assert_eq!(s.duplicates, 1);
        let events = progress.0.lock().unwrap();
        let dups = events.iter().filter(|e| matches!(e, ProgressEvent::FileDuplicate { .. })).count();
        assert_eq!(dups, 1);
    }
}