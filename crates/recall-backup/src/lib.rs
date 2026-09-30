//! Backup sync: copy every archived object to a secondary drive, using
//! SHA-256 to decide what's already there. Each run records a row in
//! `backup_runs`.
//!
//! Design (PRD §6.2):
//! - Replicate `objects/` (content-addressed), sidecar files, and the
//!   catalog DB.
//! - Skip files whose sha256 matches the destination; copy otherwise.
//! - Use temp+rename for crash safety on the destination.
//!
//! ## Destinations
//!
//! The destination is anything implementing [`BackupDestination`].
//! Local filesystem is built in; remote (S3 / B2 / R2) is the next
//! step — see [`destination`] for the trait surface.

#![forbid(unsafe_code)]

pub mod destination;
pub mod local_destination;
pub mod migrate;

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use recall_core::{ArchivePath, BackupId, Progress, Result};
use recall_db::{BackupRunRow, Catalog, MediaRow};
use recall_storage::ObjectStore;

pub use destination::{BackupDestination, Key};
pub use local_destination::LocalDestination;

// ---------------------------------------------------------------------------
// Generic backup routine that takes any `BackupDestination`.
// ---------------------------------------------------------------------------

/// Generic backup loop: iterate every archived object + sidecar, push it
/// to the destination (skipping when content already matches).
pub fn backup_to(
    conn: &mut Connection,
    src: &ObjectStore,
    destination_label: &str,
    dst: &dyn BackupDestination,
    progress: Option<&dyn Progress>,
) -> Result<BackupStats> {
    let now_nanos = Utc::now().timestamp_nanos_opt().unwrap_or_else(|| Utc::now().timestamp() * 1_000_000_000);
    let run_id = BackupId::new(format!("backup_{}", now_nanos));
    let now = now_nanos / 1_000_000_000;

    {
        let cat = Catalog::new(conn);
        cat.record_backup_run(&BackupRunRow {
            id: run_id.clone(),
            destination_path: destination_label.to_string(),
            started_at: now,
            finished_at: None,
            bytes_copied: 0,
            files_copied: 0,
            files_skipped: 0,
            files_failed: 0,
            status: "running".into(),
        })?;
    }

    let mut stats = BackupStats::default();

    // 1. Replicate every object file.
    for (archive_path, src_abs) in src.iter_objects() {
        let key = archive_path.as_path().as_str().to_string();
        let hash = match sha256_file(&src_abs) {
            Ok(h) => h,
            Err(e) => {
                stats.files_failed += 1;
                eprintln!("backup hash failed {}: {e}", src_abs.display());
                if let Some(p) = progress {
                    p.on_event(&recall_core::ProgressEvent::BackupFailed {
                        path: src_abs.clone(),
                        reason: e.to_string(),
                    });
                }
                continue;
            }
        };
        match push_one(dst, &key, &src_abs, &hash) {
            PushOutcome::Copied(bytes) => {
                stats.files_copied += 1;
                stats.bytes_copied += bytes;
                if let Some(p) = progress {
                    p.on_event(&recall_core::ProgressEvent::BackupCopied {
                        path: src_abs.clone(),
                        bytes: bytes as u64,
                    });
                }
            }
            PushOutcome::Skipped => {
                stats.files_skipped += 1;
                if let Some(p) = progress {
                    p.on_event(&recall_core::ProgressEvent::BackupSkipped {
                        path: src_abs.clone(),
                    });
                }
            }
            PushOutcome::Failed(e) => {
                stats.files_failed += 1;
                eprintln!("backup copy failed {}: {e}", src_abs.display());
                if let Some(p) = progress {
                    p.on_event(&recall_core::ProgressEvent::BackupFailed {
                        path: src_abs.clone(),
                        reason: e.to_string(),
                    });
                }
            }
        }
    }

    // 2. Copy sidecars.
    let sidecars_dir = src.root().join("sidecars");
    if sidecars_dir.exists() {
        for entry in walkdir::WalkDir::new(&sidecars_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(&sidecars_dir).unwrap();
            let key = format!("sidecars/{}", rel.to_string_lossy());
            let hash = match sha256_file(entry.path()) {
                Ok(h) => h,
                Err(_) => continue,
            };
            match push_one(dst, &key, entry.path(), &hash) {
                PushOutcome::Copied(bytes) => {
                    stats.files_copied += 1;
                    stats.bytes_copied += bytes;
                    if let Some(p) = progress {
                        p.on_event(&recall_core::ProgressEvent::BackupCopied {
                            path: entry.path().to_path_buf(),
                            bytes: bytes as u64,
                        });
                    }
                }
                PushOutcome::Skipped => {
                    stats.files_skipped += 1;
                    if let Some(p) = progress {
                        p.on_event(&recall_core::ProgressEvent::BackupSkipped {
                            path: entry.path().to_path_buf(),
                        });
                    }
                }
                PushOutcome::Failed(e) => {
                    stats.files_failed += 1;
                    eprintln!("backup sidecar failed {}: {e}", entry.path().display());
                    if let Some(p) = progress {
                        p.on_event(&recall_core::ProgressEvent::BackupFailed {
                            path: entry.path().to_path_buf(),
                            reason: e.to_string(),
                        });
                    }
                }
            }
        }
    }

    let final_status = if stats.files_failed > 0 {
        "partial"
    } else {
        "ok"
    };

    {
        let cat = Catalog::new(conn);
        cat.record_backup_run(&BackupRunRow {
            id: run_id,
            destination_path: destination_label.to_string(),
            started_at: now,
            finished_at: Some(Utc::now().timestamp()),
            bytes_copied: stats.bytes_copied,
            files_copied: stats.files_copied,
            files_skipped: stats.files_skipped,
            files_failed: stats.files_failed,
            status: final_status.into(),
        })?;
    }

    Ok(stats)
}

enum PushOutcome {
    Copied(i64),
    Skipped,
    Failed(io::Error),
}

fn push_one(dst: &dyn BackupDestination, key: &str, src: &Path, hash: &str) -> PushOutcome {
    match dst.same_content(key, hash) {
        Ok(true) => return PushOutcome::Skipped,
        Ok(false) => {}
        Err(e) => {
            return PushOutcome::Failed(io::Error::other(format!("same_content: {e}")));
        }
    }
    let bytes = match fs::read(src) {
        Ok(b) => b,
        Err(e) => return PushOutcome::Failed(e),
    };
    if let Err(e) = dst.put(key, &bytes) {
        return PushOutcome::Failed(io::Error::other(format!("put: {e}")));
    }
    PushOutcome::Copied(bytes.len() as i64)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BackupStats {
    pub files_copied: i64,
    pub files_skipped: i64,
    pub files_failed: i64,
    pub bytes_copied: i64,
}

/// Copy every archived object + sidecar from `src` into `dst_root`.
pub fn backup(
    conn: &mut Connection,
    src: &ObjectStore,
    dst_root: &Path,
) -> Result<BackupStats> {
    let now_nanos = Utc::now().timestamp_nanos_opt().unwrap_or_else(|| Utc::now().timestamp() * 1_000_000_000);
    let run_id = BackupId::new(format!("backup_{}", now_nanos));
    let now = now_nanos / 1_000_000_000;

    fs::create_dir_all(dst_root)?;
    let dst_objects = dst_root.join("objects");
    fs::create_dir_all(&dst_objects)?;

    {
        let cat = Catalog::new(conn);
        cat.record_backup_run(&BackupRunRow {
            id: run_id.clone(),
            destination_path: dst_root.to_string_lossy().to_string(),
            started_at: now,
            finished_at: None,
            bytes_copied: 0,
            files_copied: 0,
            files_skipped: 0,
            files_failed: 0,
            status: "running".into(),
        })?;
    }

    let mut stats = BackupStats::default();

    // 1. Replicate every object file.
    for (archive_path, src_abs) in src.iter_objects() {
        let dst_abs = dst_root.join(archive_path.as_path());
        if let Some(parent) = dst_abs.parent() {
            fs::create_dir_all(parent)?;
        }
        match copy_if_different(&src_abs, &dst_abs) {
            CopyOutcome::Copied(bytes) => {
                stats.files_copied += 1;
                stats.bytes_copied += bytes;
            }
            CopyOutcome::Skipped => {
                stats.files_skipped += 1;
            }
            CopyOutcome::Failed(e) => {
                stats.files_failed += 1;
                eprintln!("backup copy failed {}: {e}", src_abs.display());
            }
        }
    }

    // 2. Copy sidecars (sidecars/<sha256>.json).
    let sidecars_dir = src.root().join("sidecars");
    if sidecars_dir.exists() {
        let dst_sidecars = dst_root.join("sidecars");
        fs::create_dir_all(&dst_sidecars)?;
        for entry in walkdir::WalkDir::new(&sidecars_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(&sidecars_dir).unwrap();
            let dst_abs = dst_sidecars.join(rel);
            if let Some(parent) = dst_abs.parent() {
                fs::create_dir_all(parent)?;
            }
            match copy_if_different(entry.path(), &dst_abs) {
                CopyOutcome::Copied(bytes) => {
                    stats.files_copied += 1;
                    stats.bytes_copied += bytes;
                }
                CopyOutcome::Skipped => stats.files_skipped += 1,
                CopyOutcome::Failed(e) => {
                    stats.files_failed += 1;
                    eprintln!(
                        "backup sidecar failed {}: {e}",
                        entry.path().display()
                    );
                }
            }
        }
    }

    let final_status = if stats.files_failed > 0 {
        "partial"
    } else {
        "ok"
    };

    {
        let cat = Catalog::new(conn);
        cat.record_backup_run(&BackupRunRow {
            id: run_id,
            destination_path: dst_root.to_string_lossy().to_string(),
            started_at: now,
            finished_at: Some(Utc::now().timestamp()),
            bytes_copied: stats.bytes_copied,
            files_copied: stats.files_copied,
            files_skipped: stats.files_skipped,
            files_failed: stats.files_failed,
            status: final_status.into(),
        })?;
    }

    Ok(stats)
}

fn copy_if_different(src_path: &Path, dst_path: &Path) -> CopyOutcome {
    if dst_path.exists() && same_contents(src_path, dst_path) {
        return CopyOutcome::Skipped;
    }
    match copy_file_atomic(src_path, dst_path) {
        Ok(n) => CopyOutcome::Copied(n),
        Err(e) => CopyOutcome::Failed(e),
    }
}

fn same_contents(a: &Path, b: &Path) -> bool {
    let Ok(ma) = fs::metadata(a) else { return false };
    let Ok(mb) = fs::metadata(b) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    let ha = match sha256_file(a) {
        Ok(h) => h,
        Err(_) => return false,
    };
    let hb = match sha256_file(b) {
        Ok(h) => h,
        Err(_) => return false,
    };
    ha == hb
}

pub(crate) fn sha256_file(p: &Path) -> std::io::Result<String> {
    let mut f = File::open(p)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Streaming copy with a temp + rename. Reads in 1 MiB chunks so
/// multi-GB videos don't blow up the process RSS.
pub(crate) fn copy_file_atomic(src: &Path, dst: &Path) -> std::io::Result<i64> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut reader = File::open(src)?;
    let mut tmp = dst.to_path_buf();
    let new_ext = match dst.extension().and_then(|s| s.to_str()) {
        Some(e) => format!("{e}.partial"),
        None => "partial".to_string(),
    };
    tmp.set_extension(new_ext);
    let mut writer = File::create(&tmp)?;
    let mut buf = [0u8; 1024 * 1024];
    let mut total: i64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        total += n as i64;
    }
    writer.sync_all()?;
    drop(writer);
    fs::rename(&tmp, dst)?;
    Ok(total)
}

enum CopyOutcome {
    Copied(i64),
    Skipped,
    Failed(io::Error),
}

/// Copy the catalog DB file alongside the backup. Caller passes the path
/// so this crate stays decoupled from any specific DB layout.
pub fn copy_catalog_db(db_path: &Path, dst_root: &Path) -> Result<()> {
    fs::create_dir_all(dst_root)?;
    copy_file_atomic(db_path, &dst_root.join("archive.db"))
        .map(|_| ())
        .map_err(|e| recall_core::Error::io(db_path, e))
}

#[allow(dead_code)]
fn _keep_pathbuf_imported(_: PathBuf) {}

// ---------------------------------------------------------------------------
// ZIP export: bundle every archived object plus archive.db into a single
// deflate-compressed .zip. Streamed, not buffered, so multi-GB videos
// don't OOM the process.
// ---------------------------------------------------------------------------

/// Stats from a ZIP export run.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExportStats {
    pub files_copied: i64,
    pub files_failed: i64,
    pub bytes_written: i64,
}

/// Bundle every archived media file plus `archive.db` into a single
/// `.zip` at `dst_zip`.
///
/// Entry names inside the zip are derived from each file's
/// `original_filename` where possible, falling back to its on-disk
/// `archive_path` (`objects/<aa>/<bb>/<hash>`). Names that collide within
/// the same export are disambiguated by prefixing with `<media_id>__` so
/// extracting the zip never silently overwrites a file.
pub fn export_zip_to(
    conn: &mut Connection,
    src: &ObjectStore,
    archive_db_path: &Path,
    dst_zip: &Path,
    progress: Option<&dyn Progress>,
) -> Result<ExportStats> {
    use std::collections::HashSet;
    use std::io::copy;
    use zip::write::FileOptions;
    use zip::CompressionMethod;

    // Overwrite semantics: a re-export replaces the destination.
    if dst_zip.exists() {
        fs::remove_file(dst_zip)?;
    }

    let file = File::create(dst_zip).map_err(|e| recall_core::Error::io(dst_zip, e))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);

    let mut stats = ExportStats::default();
    let mut used_names: HashSet<String> = HashSet::new();

    let rows = {
        let cat = Catalog::new(conn);
        cat.iter_all_media()?
    };

    for row in rows {
        let desired_name = entry_name_for(&row);
        let final_name = if used_names.contains(&desired_name) {
            // Disambiguate with the media id so extracting never
            // silently drops a duplicate filename.
            format!("{}__{}", row.id, desired_name)
        } else {
            desired_name.clone()
        };
        used_names.insert(final_name.clone());

        let archive_path =
            ArchivePath(camino::Utf8PathBuf::from(row.archive_path.clone()));
        let mut src_file = match src.read(&archive_path) {
            Ok(f) => f,
            Err(e) => {
                stats.files_failed += 1;
                eprintln!(
                    "export_zip read failed {}: {e}",
                    archive_path.as_path()
                );
                if let Some(p) = progress {
                    p.on_event(&recall_core::ProgressEvent::ExportFailed {
                        path: PathBuf::from(final_name),
                        reason: e.to_string(),
                    });
                }
                continue;
            }
        };

        if let Err(e) = zw.start_file(&final_name, opts) {
            stats.files_failed += 1;
            eprintln!("export_zip start_file failed {final_name}: {e}");
            if let Some(p) = progress {
                p.on_event(&recall_core::ProgressEvent::ExportFailed {
                    path: PathBuf::from(final_name),
                    reason: e.to_string(),
                });
            }
            continue;
        }

        let written = match copy(&mut src_file, &mut zw) {
            Ok(n) => n,
            Err(e) => {
                stats.files_failed += 1;
                eprintln!("export_zip copy failed {final_name}: {e}");
                if let Some(p) = progress {
                    p.on_event(&recall_core::ProgressEvent::ExportFailed {
                        path: PathBuf::from(final_name),
                        reason: e.to_string(),
                    });
                }
                continue;
            }
        };

        stats.files_copied += 1;
        stats.bytes_written += written as i64;
        if let Some(p) = progress {
            p.on_event(&recall_core::ProgressEvent::ExportCopied {
                path: PathBuf::from(final_name),
                bytes: written as u64,
            });
        }
    }

    // Embed the catalog so the bundle is self-contained. Someone
    // extracting the zip can `cp archive.db <somewhere>` and open it
    // with `recallctl status --archive-root …` to introspect.
    if archive_db_path.exists() {
        let db_name = "archive.db".to_string();
        used_names.insert(db_name.clone());
        if let Err(e) = zw.start_file(&db_name, opts) {
            stats.files_failed += 1;
            eprintln!("export_zip start_file failed {db_name}: {e}");
        } else {
            let mut db_file = match File::open(archive_db_path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("export_zip open db failed: {e}");
                    return Err(recall_core::Error::io(archive_db_path, e));
                }
            };
            match copy(&mut db_file, &mut zw) {
                Ok(n) => {
                    stats.files_copied += 1;
                    stats.bytes_written += n as i64;
                    if let Some(p) = progress {
                        p.on_event(&recall_core::ProgressEvent::ExportCopied {
                            path: PathBuf::from(db_name),
                            bytes: n as u64,
                        });
                    }
                }
                Err(e) => {
                    stats.files_failed += 1;
                    eprintln!("export_zip copy db failed: {e}");
                }
            }
        }
    }

    if let Err(e) = zw.finish() {
        return Err(e.into());
    }

    Ok(stats)
}

/// Pick a sensible entry name for a media row. Prefer the original
/// filename; strip any leading slashes / drive letters so the zip stays
/// relative. Fall back to the content-addressed archive path when no
/// filename was recorded.
fn entry_name_for(row: &MediaRow) -> String {
    let raw = row
        .original_filename
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(sanitize_zip_name)
        .unwrap_or_else(|| row.archive_path.clone());
    // Always use forward slashes inside the zip, even on Windows.
    raw.replace('\\', "/")
}

/// Make a string safe to use as a zip entry name: strip path separators
/// and zip-illegal characters, fall back to a placeholder if everything
/// got stripped.
fn sanitize_zip_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c == ' ' || c == '_');
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recall_db::open as open_db;
    use recall_importer::import_folder;
    use std::fs::create_dir_all;

    fn write_unique_jpeg(path: &Path, salt: u8) {
        std::fs::write(path, [0xFF, 0xD8, 0xFF, salt, 0xD9]).unwrap();
    }

    #[test]
    fn backup_copies_all_objects() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);
        write_unique_jpeg(&src.join("b.jpg"), 2);
        import_folder(&mut conn, &store, &src, "folder", None).unwrap();

        let dst = dir.path().join("backup");
        let stats = backup(&mut conn, &store, &dst).unwrap();
        assert!(stats.files_copied >= 2);
        assert_eq!(stats.files_failed, 0);

        let mut count = 0;
        for entry in walkdir::WalkDir::new(dst.join("objects"))
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                count += 1;
            }
        }
        assert!(count >= 2);
    }

    #[test]
    fn backup_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);
        import_folder(&mut conn, &store, &src, "folder", None).unwrap();

        let dst = dir.path().join("backup");
        let s1 = backup(&mut conn, &store, &dst).unwrap();
        let s2 = backup(&mut conn, &store, &dst).unwrap();
        assert!(s1.files_copied >= 1);
        assert_eq!(s2.files_copied, 0, "second run should skip everything");
        assert!(s2.files_skipped >= s1.files_copied);
    }

    #[test]
    fn copy_catalog_db_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("archive.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let conn = open_db(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS probe (id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO probe(v) VALUES ('hello');",
        )
        .unwrap();
        drop(conn);

        let dst = dir.path().join("backup");
        copy_catalog_db(&db, &dst).unwrap();
        let copied = dst.join("archive.db");
        assert!(copied.exists());
        let c2 = open_db(&copied).unwrap();
        let v: String = c2
            .query_row("SELECT v FROM probe", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "hello");
    }

    // -----------------------------------------------------------------
    // ZIP export tests
    // -----------------------------------------------------------------

    fn seed_two_jpegs_into(dir: &Path, conn: &mut Connection, store: &ObjectStore) {
        let src = dir.join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 0xAA);
        write_unique_jpeg(&src.join("b.jpg"), 0xBB);
        import_folder(conn, store, &src, "folder", None).unwrap();
    }

    fn read_zip_entries(zip_path: &Path) -> std::collections::HashMap<String, Vec<u8>> {
        use std::io::Read;
        let f = File::open(zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(f).unwrap();
        let mut out = std::collections::HashMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let name = entry.name().to_string();
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).unwrap();
            out.insert(name, buf);
        }
        out
    }

    #[test]
    fn export_zip_writes_every_media() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let db_path = dir.path().join("archive.db");
        let mut conn = open_db(&db_path).unwrap();

        seed_two_jpegs_into(dir.path(), &mut conn, &store);

        let zip_path = dir.path().join("export.zip");
        let stats = export_zip_to(&mut conn, &store, &db_path, &zip_path, None).unwrap();
        assert!(stats.files_copied >= 2);
        assert_eq!(stats.files_failed, 0);

        let entries = read_zip_entries(&zip_path);
        // Both original filenames must be in the zip.
        assert!(entries.contains_key("a.jpg"), "missing a.jpg in {entries:?}");
        assert!(entries.contains_key("b.jpg"), "missing b.jpg in {entries:?}");
        // Plus the embedded archive.db.
        assert!(entries.contains_key("archive.db"));

        // The bytes for a.jpg must match the original file bytes.
        let original = std::fs::read(dir.path().join("src/a.jpg")).unwrap();
        assert_eq!(entries.get("a.jpg").unwrap(), &original);
    }

    #[test]
    fn export_zip_embeds_archive_db() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let db_path = dir.path().join("archive.db");
        {
            let conn = open_db(&db_path).unwrap();
            // Touch a unique value so we can confirm the DB round-tripped.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS probe (id INTEGER PRIMARY KEY, v TEXT);
                 INSERT INTO probe(v) VALUES ('hello-export');",
            )
            .unwrap();
        }

        let mut conn = open_db(&db_path).unwrap();
        let zip_path = dir.path().join("export.zip");
        export_zip_to(&mut conn, &store, &db_path, &zip_path, None).unwrap();

        let entries = read_zip_entries(&zip_path);
        let db_bytes = entries.get("archive.db").expect("archive.db missing");
        assert!(!db_bytes.is_empty());

        // Write the embedded DB to disk and open it with recall_db.
        let extracted_db = dir.path().join("extracted.db");
        std::fs::write(&extracted_db, db_bytes).unwrap();
        let c2 = open_db(&extracted_db).unwrap();
        let v: String = c2
            .query_row("SELECT v FROM probe", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "hello-export");
    }

    #[test]
    fn export_zip_dedups_collision() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let db_path = dir.path().join("archive.db");
        let mut conn = open_db(&db_path).unwrap();

        // Two source folders that both contain IMG_001.jpg. The
        // importer records original_filename = "IMG_001.jpg" for both,
        // so the exporter must disambiguate.
        let src1 = dir.path().join("src1");
        let src2 = dir.path().join("src2");
        create_dir_all(&src1).unwrap();
        create_dir_all(&src2).unwrap();
        write_unique_jpeg(&src1.join("IMG_001.jpg"), 0xC1);
        write_unique_jpeg(&src2.join("IMG_001.jpg"), 0xC2);
        import_folder(&mut conn, &store, &src1, "folder", None).unwrap();
        import_folder(&mut conn, &store, &src2, "folder", None).unwrap();

        let zip_path = dir.path().join("export.zip");
        let stats = export_zip_to(&mut conn, &store, &db_path, &zip_path, None).unwrap();
        assert_eq!(stats.files_failed, 0);
        // Two media + one db.
        assert_eq!(stats.files_copied, 3);

        let entries = read_zip_entries(&zip_path);
        // One entry keeps the original name; the other is prefixed.
        let names: Vec<&String> = entries.keys().collect();
        assert!(names.iter().any(|n| n.as_str() == "IMG_001.jpg"));
        assert!(
            names.iter().any(|n| n.ends_with("__IMG_001.jpg")),
            "expected one prefixed entry, got {names:?}"
        );
    }

    #[test]
    fn export_zip_continues_after_failure() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let db_path = dir.path().join("archive.db");
        let mut conn = open_db(&db_path).unwrap();

        seed_two_jpegs_into(dir.path(), &mut conn, &store);

        // Locate one of the stored objects and corrupt it.
        let cat = Catalog::new(&mut conn);
        let rows = cat.iter_all_media().unwrap();
        assert!(rows.len() >= 2);
        let victim_path = rows[0].archive_path.clone();
        let victim_abs = store_root.join(&victim_path);
        // Remove the file outright so the read fails.
        std::fs::remove_file(&victim_abs).unwrap();

        let zip_path = dir.path().join("export.zip");
        let stats = export_zip_to(&mut conn, &store, &db_path, &zip_path, None).unwrap();
        // The bad one failed; the good one plus archive.db still copied.
        assert!(stats.files_failed >= 1);
        assert!(stats.files_copied >= 2, "got {}", stats.files_copied);
        assert!(zip_path.exists());
    }
}