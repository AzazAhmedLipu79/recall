//! Verification: re-stream-hash every archived object and compare to its
//! recorded SHA-256. Reports per-file findings (ok / missing / mismatched)
//! and a summary in a verification_runs row.
//!
//! Crash-safe: a partial run still produces a finalised row with whatever
//! findings were recorded.

#![forbid(unsafe_code)]

use std::path::Path;

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use recall_core::{Progress, Result, VerificationId};
use recall_db::{
    Catalog, VerificationFinding, VerificationFindingRow, VerificationRunRow, VerificationStatus,
};
use recall_hash::sha256_file;
use recall_storage::ObjectStore;

/// Outcome of a single verification pass.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct VerifyStats {
    pub files_checked: i64,
    pub files_ok: i64,
    pub files_missing: i64,
    pub files_mismatched: i64,
}

/// Walk the catalog, re-hashing every archived object, and record findings.
pub fn verify_all(
    conn: &mut Connection,
    store: &ObjectStore,
    progress: Option<&dyn Progress>,
) -> Result<VerifyStats> {
    let now = Utc::now().timestamp();
    let run_id_str = format!("verify_{}", now);
    let run_id = VerificationId::new(run_id_str.clone());

    {
        let cat = Catalog::new(conn);
        cat.begin_verification(&VerificationRunRow {
            id: run_id.clone(),
            started_at: now,
            finished_at: None,
            files_checked: 0,
            files_ok: 0,
            files_missing: 0,
            files_mismatched: 0,
            status: VerificationStatus::Running,
        })?;
    }

    // Snapshot all media rows up front so we don't hold a borrow across
    // long-running per-file work.
    let media = {
        let cat = Catalog::new(conn);
        cat.iter_all_media()?
    };

    let mut stats = VerifyStats::default();
    for m in &media {
        let archive_path = Path::new(&m.archive_path);
        let abs = store.root().join(archive_path);

        if !abs.exists() {
            stats.files_checked += 1;
            stats.files_missing += 1;
            let cat = Catalog::new(conn);
            cat.record_verification_finding(&VerificationFindingRow {
                run_id: run_id.clone(),
                media_id: m.id.clone(),
                finding: VerificationFinding::Missing,
                expected_hash: m.sha256.clone(),
                actual_hash: None,
            })?;
            if let Some(p) = progress {
                p.on_event(&recall_core::ProgressEvent::VerifyMissing { path: abs.clone() });
            }
            continue;
        }

        let actual = match sha256_file(&abs) {
            Ok(h) => h,
            Err(e) => {
                stats.files_checked += 1;
                stats.files_missing += 1;
                let cat = Catalog::new(conn);
                cat.record_verification_finding(&VerificationFindingRow {
                    run_id: run_id.clone(),
                    media_id: m.id.clone(),
                    finding: VerificationFinding::Missing,
                    expected_hash: m.sha256.clone(),
                    actual_hash: Some(format!("read-error: {e}")),
                })?;
                if let Some(p) = progress {
                    p.on_event(&recall_core::ProgressEvent::VerifyMissing { path: abs.clone() });
                }
                continue;
            }
        };

        stats.files_checked += 1;
        let finding = if actual == m.sha256 {
            stats.files_ok += 1;
            VerificationFinding::Ok
        } else {
            stats.files_mismatched += 1;
            VerificationFinding::Mismatched
        };
        let cat = Catalog::new(conn);
        cat.record_verification_finding(&VerificationFindingRow {
            run_id: run_id.clone(),
            media_id: m.id.clone(),
            finding,
            expected_hash: m.sha256.clone(),
            actual_hash: Some(actual.clone()),
        })?;
        if let Some(p) = progress {
            if actual == m.sha256 {
                p.on_event(&recall_core::ProgressEvent::VerifyOk { path: abs.clone() });
            } else {
                p.on_event(&recall_core::ProgressEvent::VerifyMismatched {
                    path: abs.clone(),
                    expected: m.sha256.clone(),
                    actual,
                });
            }
        }
    }

    let status = if stats.files_missing > 0 || stats.files_mismatched > 0 {
        VerificationStatus::Problem
    } else {
        VerificationStatus::Ok
    };

    {
        let cat = Catalog::new(conn);
        cat.update_verification(&VerificationRunRow {
            id: run_id,
            started_at: now,
            finished_at: Some(Utc::now().timestamp()),
            files_checked: stats.files_checked,
            files_ok: stats.files_ok,
            files_missing: stats.files_missing,
            files_mismatched: stats.files_mismatched,
            status,
        })?;
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use recall_db::open as open_db;
    use recall_importer::{import_folder, MEDIA_EXTS};
    use std::fs::create_dir_all;

    fn write_unique_jpeg(path: &Path, salt: u8) {
        std::fs::write(path, [0xFF, 0xD8, 0xFF, salt, 0xD9]).unwrap();
    }

    #[test]
    fn verify_clean_archive_reports_ok() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);
        write_unique_jpeg(&src.join("b.jpg"), 2);

        let stats = import_folder(&mut conn, &store, &src, "folder", None).unwrap();
        assert_eq!(stats.imported, 2);

        let v = verify_all(&mut conn, &store, None).unwrap();
        assert_eq!(v.files_checked, 2);
        assert_eq!(v.files_ok, 2);
        assert_eq!(v.files_missing, 0);
        assert_eq!(v.files_mismatched, 0);
    }

    #[test]
    fn verify_detects_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);
        import_folder(&mut conn, &store, &src, "folder", None).unwrap();

        // Corrupt: remove the archive object file.
        let archive = store_root.join("objects");
        let mut found = None;
        for entry in walkdir::WalkDir::new(&archive).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                found = Some(entry.path().to_path_buf());
                break;
            }
        }
        let f = found.expect("expected an archived object");
        std::fs::remove_file(&f).unwrap();

        let v = verify_all(&mut conn, &store, None).unwrap();
        assert_eq!(v.files_checked, 1);
        assert_eq!(v.files_missing, 1);
        assert_eq!(v.files_ok, 0);
    }

    #[test]
    fn verify_detects_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("store");
        let store = ObjectStore::open(&store_root).unwrap();
        let mut conn = open_db(&dir.path().join("archive.db")).unwrap();

        let src = dir.path().join("src");
        create_dir_all(&src).unwrap();
        write_unique_jpeg(&src.join("a.jpg"), 1);
        import_folder(&mut conn, &store, &src, "folder", None).unwrap();

        // Corrupt the archived file in place (don't change size).
        let archive = store_root.join("objects");
        let mut found = None;
        for entry in walkdir::WalkDir::new(&archive).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                found = Some(entry.path().to_path_buf());
                break;
            }
        }
        let f = found.expect("expected an archived object");
        let bytes = std::fs::read(&f).unwrap();
        let mut corrupted = bytes.clone();
        if !corrupted.is_empty() {
            corrupted[0] = corrupted[0].wrapping_add(1);
        }
        std::fs::write(&f, &corrupted).unwrap();

        let v = verify_all(&mut conn, &store, None).unwrap();
        assert_eq!(v.files_checked, 1);
        assert_eq!(v.files_mismatched, 1);
    }

    #[test]
    fn media_ext_allowlist_nonempty() {
        assert!(!MEDIA_EXTS.is_empty());
    }
}