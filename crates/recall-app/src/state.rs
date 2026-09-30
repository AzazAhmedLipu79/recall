//! Shared, thread-safe application state.
//!
//! The host process keeps one open SQLite connection (via
//! `recall_db::open`) and one `ObjectStore` so commands don't have
//! to reopen the catalog on every IPC call. SQLite is fine to share
//! across threads behind a `Mutex` because the engine is read-heavy
//! and short-lived writes serialize naturally.

use std::path::{Path, PathBuf};
use std::time::Instant;

use recall_db::Catalog;
use recall_storage::ObjectStore;

/// How long a cached `disk_usage` value is considered valid. The walk
/// is O(object count) so refreshing on every IPC tick would be wasteful
/// for statusbar updates that fire after every Cmd_R or library nav.
const DISK_USAGE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

pub struct AppState {
    pub archive_root: PathBuf,
    pub db_path: PathBuf,
    pub store_root: PathBuf,
    pub conn: rusqlite::Connection,
    pub store: ObjectStore,
    /// Memoised disk-usage result so statusbar refreshes don't walk
    /// the object tree on every keystroke. Invalidated after
    /// `DISK_USAGE_CACHE_TTL` or by an explicit `invalidate_disk_usage`.
    pub disk_usage_cache: Option<(Instant, u64)>,
}

impl AppState {
    /// Open (or initialise) a Recall archive rooted at `archive_root`.
    /// Creates `archive.db` and `store/` if missing — same semantics as
    /// `recallctl init`.
    pub fn open(archive_root: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(archive_root)?;
        let db_path = archive_root.join("archive.db");
        let store_root = archive_root.join("store");

        // If the DB doesn't exist yet, run `init`-equivalent setup.
        if !db_path.exists() {
            // The recall-db::open helper runs migrations on first open.
            // We then also ensure the on-disk store layout exists.
            let store = ObjectStore::open(&store_root)?;
            let conn = recall_db::open(&db_path)?;
            return Ok(Self {
                archive_root: archive_root.to_path_buf(),
                db_path,
                store_root,
                conn,
                store,
                disk_usage_cache: None,
            });
        }

        let conn = recall_db::open(&db_path)?;
        let store = ObjectStore::open(&store_root)?;
        Ok(Self {
            archive_root: archive_root.to_path_buf(),
            db_path,
            store_root,
            conn,
            store,
            disk_usage_cache: None,
        })
    }

    /// Borrow the catalog wrapper. Cloning would be overkill; we
    /// hand out references because commands hold the AppState mutex
    /// for the duration of the call.
    pub fn catalog(&mut self) -> Catalog<'_> {
        Catalog::new(&mut self.conn)
    }

    /// Cached disk-usage lookup; falls through to `compute_disk_usage`
    /// when the cache is stale or empty.
    pub fn disk_usage(&mut self) -> u64 {
        let now = Instant::now();
        if let Some((at, value)) = self.disk_usage_cache {
            if now.duration_since(at) < DISK_USAGE_CACHE_TTL {
                return value;
            }
        }
        let value = compute_disk_usage_uncached(self);
        self.disk_usage_cache = Some((now, value));
        value
    }
}

/// Walk the on-disk archive to sum up object + db + log bytes.
/// `state.disk_usage` is the public, cached entry point.
fn compute_disk_usage_uncached(state: &AppState) -> u64 {
    let mut total = 0u64;
    if let Ok(n) = state.store.disk_usage() {
        total += n;
    }
    for ext in ["", "-wal", "-shm"] {
        let p = if ext.is_empty() {
            state.db_path.clone()
        } else {
            state.db_path.with_file_name(format!(
                "{}{ext}",
                state
                    .db_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("archive.db")
            ))
        };
        if let Ok(meta) = std::fs::metadata(&p) {
            total += meta.len();
        }
    }
    let logs = state.archive_root.join("logs");
    total += dir_size_bounded(&logs, 4);
    total
}

/// Recursive directory size with a depth cap so a runaway symlink loop
/// can't stall `cmd_archive_info`.
fn dir_size_bounded(dir: &Path, depth_remaining: u8) -> u64 {
    if depth_remaining == 0 {
        return 0;
    }
    let mut total = 0u64;
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() {
            total += dir_size_bounded(&p, depth_remaining - 1);
        }
    }
    total
}
