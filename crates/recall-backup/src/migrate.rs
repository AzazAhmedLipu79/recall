//! In-place archive migration: copy `archive.db*`, the object tree, and
//! `logs/` from a source archive root to a destination root. Used by the
//! "Move archive…" UI in Settings and the first-run wizard.
//!
//! The caller is responsible for opening / closing the source `AppState`
//! and swapping it for a fresh one opened at the destination — this
//! module only handles the file copy and progress reporting.

#![forbid(unsafe_code)]

use std::fs::{self};
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use recall_core::{Error, Progress, Result};
use recall_storage::ObjectStore;

use crate::{copy_file_atomic, sha256_file};

/// Stats from a migration run.
#[derive(Debug, Default, Clone, Copy)]
pub struct MigrationStats {
    pub files_copied: i64,
    pub bytes_copied: i64,
    pub logs_copied: i64,
    pub db_bytes: i64,
    pub verify_failures: i64,
}

/// Migrate `src_root` -> `dst_root`. The destination must not already
/// contain an `archive.db`; we refuse rather than risk clobbering an
/// existing library.
///
/// Order of operations (matters for crash safety):
///   1. Verify the destination is empty of any `archive.db*`.
///   2. Checkpoint the source WAL so all writes are in `archive.db`.
///   3. Walk every object under `src_root/store/objects/` and copy.
///   4. Walk every sidecar under `src_root/store/sidecars/` and copy.
///   5. Walk every metadata sidecar under `src_root/store/metadata/`.
///   6. Copy the entire `logs/` directory.
///   7. Copy `archive.db` last (after the heavy bytes).
///
/// The source archive root is never modified.
pub fn migrate_archive(
    conn: &mut Connection,
    src: &ObjectStore,
    src_root: &Path,
    dst_root: &Path,
    progress: Option<&dyn Progress>,
) -> Result<MigrationStats> {
    if !src_root.is_dir() {
        return Err(Error::Other(format!(
            "source archive root does not exist: {}",
            src_root.display()
        )));
    }
    if dst_root.join("archive.db").exists() {
        return Err(Error::Other(
            "destination already has an archive — pick an empty folder".into(),
        ));
    }
    fs::create_dir_all(dst_root).map_err(|e| Error::io(dst_root, e))?;

    // WAL checkpoint so archive.db contains everything (the WAL file
    // would otherwise hold recent writes that would be missing from
    // the copied db).
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;

    let mut stats = MigrationStats::default();

    // Objects live under `<dst_root>/store/objects/...` to mirror the
    // existing layout that `ObjectStore::open(<dst>/store)` expects.
    let dst_store_root = dst_root.join("store");
    let src_objects = src.root().join("objects");
    if src_objects.exists() {
        for (archive_path, src_abs) in src.iter_objects() {
            let dst_abs = dst_store_root.join(archive_path.as_path().as_std_path());
            if let Some(parent) = dst_abs.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            match copy_file_atomic(&src_abs, &dst_abs) {
                Ok(bytes) => {
                    stats.files_copied += 1;
                    stats.bytes_copied += bytes;
                    if let Some(p) = progress {
                        p.on_event(&recall_core::ProgressEvent::BackupCopied {
                            path: dst_abs.clone(),
                            bytes: bytes as u64,
                        });
                    }
                }
                Err(e) => {
                    return Err(Error::io(&dst_abs, e));
                }
            }
        }
    }

    // Sidecars + metadata sidecars live next to objects under the same
    // `store/` parent — copy them in the same pass.
    for sub in ["sidecars", "metadata"] {
        let src_dir = src.root().join(sub);
        if !src_dir.exists() {
            continue;
        }
        let dst_dir = dst_root.join("store").join(sub);
        for entry in walkdir::WalkDir::new(&src_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(&src_dir).unwrap();
            let dst_abs = dst_dir.join(rel);
            if let Some(parent) = dst_abs.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            match copy_file_atomic(entry.path(), &dst_abs) {
                Ok(bytes) => {
                    stats.files_copied += 1;
                    stats.bytes_copied += bytes;
                }
                Err(e) => {
                    return Err(Error::io(&dst_abs, e));
                }
            }
        }
    }

    // Copy the whole logs/ tree to preserve the rolling-file layout.
    let src_logs = src_root.join("logs");
    if src_logs.exists() {
        copy_dir_recursive(&src_logs, &dst_root.join("logs"), &mut stats.logs_copied)?;
    }

    // archive.db goes last so the heavy bytes are already on the
    // destination when we touch it.
    let src_db = src_root.join("archive.db");
    if src_db.exists() {
        let dst_db = dst_root.join("archive.db");
        let bytes = copy_file_atomic(&src_db, &dst_db)?;
        stats.db_bytes = bytes;

        // Copy WAL/SHM sidecars too — they'll be empty after the
        // TRUNCATE checkpoint above, but copy for symmetry in case the
        // user reopens the destination before the destination SQLite
        // engine gets a chance to recreate them.
        for ext in ["wal", "shm"] {
            let src_side = src_root.join(format!("archive.db-{ext}"));
            if src_side.exists() {
                let _ = copy_file_atomic(
                    &src_side,
                    &dst_root.join(format!("archive.db-{ext}")),
                );
            }
        }
    }

    Ok(stats)
}

/// Verify a sample of copied objects by re-hashing and comparing.
/// Returns the count of mismatches (0 = healthy).
pub fn sample_verify(
    src: &ObjectStore,
    dst_root: &Path,
    sample_fraction: f64,
) -> Result<i64> {
    let entries: Vec<(PathBuf, PathBuf)> = src
        .iter_objects()
        .map(|(ap, abs)| {
            let rel = ap.as_path().as_std_path();
            (abs, dst_root.join("store").join(rel))
        })
        .collect();
    let n = entries.len();
    if n == 0 {
        return Ok(0);
    }
    let sample = ((n as f64) * sample_fraction.clamp(0.0, 1.0)).ceil() as usize;
    if sample == 0 {
        return Ok(0);
    }
    // Deterministic stride so the same archive produces the same sample.
    let stride = (n / sample.max(1)).max(1);
    let mut failures = 0i64;
    let mut i = 0;
    while i < n && (i / stride) < sample {
        let (src_p, dst_p) = &entries[i];
        match (sha256_file(src_p), sha256_file(dst_p)) {
            (Ok(a), Ok(b)) if a == b => {}
            _ => failures += 1,
        }
        i += stride;
    }
    Ok(failures)
}

fn copy_dir_recursive(src: &Path, dst: &Path, counter: &mut i64) -> Result<()> {
    fs::create_dir_all(dst).map_err(|e| Error::io(dst, e))?;
    for entry in walkdir::WalkDir::new(src)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let rel = entry.path().strip_prefix(src).unwrap();
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).map_err(|e| Error::io(&target, e))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            copy_file_atomic(entry.path(), &target).map_err(|e| Error::io(&target, e))?;
            *counter += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn migrate_copies_objects_and_db() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();

        // Seed: archive.db + a couple of objects.
        let src_root = src_dir.path();
        let mut conn = Connection::open(src_root.join("archive.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS probe (id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO probe(v) VALUES ('hello-migrate');",
        )
        .unwrap();

        let store = ObjectStore::open(src_root.join("store")).unwrap();
        let a = src_root.join("a.bin");
        let b = src_root.join("b.bin");
        std::fs::write(&a, b"alpha").unwrap();
        std::fs::write(&b, b"beta").unwrap();
        let ha = store.import(&a).unwrap();
        let _hb = store.import(&b).unwrap();

        let dst_root = dst_dir.path().join("dst");
        let stats = migrate_archive(&mut conn, &store, src_root, &dst_root, None).unwrap();
        assert!(stats.files_copied >= 2, "got {stats:?}");
        assert!(stats.db_bytes > 0, "db not copied: {stats:?}");

        // Destination should have its own archive.db that opens.
        let conn2 = Connection::open(dst_root.join("archive.db")).unwrap();
        let v: String = conn2
            .query_row("SELECT v FROM probe", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "hello-migrate");

        // And the object should be there.
        let dst_store = ObjectStore::open(dst_root.join("store")).unwrap();
        assert!(dst_store
            .verify(&recall_core::ArchivePath::from_hash(&ha), &ha)
            .unwrap());
    }

    #[test]
    fn migrate_refuses_non_empty_destination() {
        let src_dir = tempdir().unwrap();
        let dst_dir = tempdir().unwrap();
        let src_root = src_dir.path();
        let mut conn = Connection::open(src_root.join("archive.db")).unwrap();
        let store = ObjectStore::open(src_root.join("store")).unwrap();

        // Pre-populate the destination with an archive.db.
        let dst_root = dst_dir.path().join("dst");
        std::fs::create_dir_all(&dst_root).unwrap();
        std::fs::write(dst_root.join("archive.db"), b"preexisting").unwrap();

        let res = migrate_archive(&mut conn, &store, src_root, &dst_root, None);
        assert!(res.is_err(), "should refuse non-empty destination");
    }
}
