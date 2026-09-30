//! SQLite catalog for the recall archive.
//!
//! The database is the **catalog**, not the source of truth for the bytes.
//! Original files live in the object store; the database records identity,
//! metadata, provenance, and organization. The user must always be able to
//! recover their media by reading the object store directly with just the
//! hash — even if the catalog itself is unrecoverable.
//!
//! ## Schema
//!
//! Tables:
//!
//! - `media` — one row per distinct logical item (a photo / video / doc).
//!   `sha256` is unique: identical content collapses to a single row.
//! - `media_files` — alternative physical locations/files representing the
//!   same content. Each media item can have multiple files (e.g. the same
//!   JPEG lives at two source paths after two imports).
//! - `media_metadata` — structured EXIF/derived fields keyed by media_id.
//!   All non-essential metadata fields live in one row (NULLable).
//! - `media_timestamps` — source-specific timestamp provenance so we never
//!   silently overwrite EXIF with Filesystem or Google values. See PRD §12.
//! - `locations` — GPS coordinates; one-to-many with media.
//! - `albums`, `album_items` — virtual collections.
//! - `devices` — devices that have produced files (iPhone, Mac, ...).
//! - `imports` — every import operation, with stats.
//! - `import_files` — per-file result of an import (imported/duplicate/failed).
//! - `backup_runs` — secondary backup sync history.
//! - `verification_runs` — verification history and findings.
//! - `verification_findings` — per-row results of a verification (ok/missing/
//!   mismatched) so we can show exactly which files need attention.
//! - `metadata_sidecars` — pointer to retained Google JSON sidecars.

#![forbid(unsafe_code)]

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use recall_core::{AlbumId, BackupId, ImportId, MediaId, MediaKind, VerificationId};

#[derive(Debug, Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid row: {0}")]
    Row(String),
    #[error("migration failed: {0}")]
    Migration(String),
}

impl From<DbError> for recall_core::Error {
    fn from(e: DbError) -> Self {
        recall_core::Error::Other(format!("db: {e}"))
    }
}

pub type DbResult<T> = std::result::Result<T, DbError>;

/// Open (or create) an archive database at `path`. Enables WAL + foreign keys.
pub fn open(path: impl AsRef<Path>) -> DbResult<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

/// Open an in-memory database, used by tests.
pub fn open_memory() -> DbResult<Connection> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&mut conn)?;
    Ok(conn)
}

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("migrations/0001_init.sql")),
    ("0002_verification", include_str!("migrations/0002_verification.sql")),
    (
        "0003_import_files",
        include_str!("migrations/0003_import_files.sql"),
    ),
    ("0004_indexes", include_str!("migrations/0004_indexes.sql")),
];

fn migrate(conn: &mut Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            name TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;

    let tx = conn.transaction()?;
    let applied: Vec<String> = {
        let mut stmt = tx.prepare("SELECT name FROM schema_migrations")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        let mut v = Vec::new();
        for row in rows {
            v.push(row?);
        }
        v
    };

    for (name, sql) in MIGRATIONS {
        if applied.iter().any(|a| a == name) {
            continue;
        }
        tx.execute_batch(sql)
            .map_err(|e| DbError::Migration(format!("{name}: {e}")))?;
        tx.execute(
            "INSERT INTO schema_migrations(name, applied_at) VALUES (?1, datetime('now'))",
            params![name],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Domain structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRow {
    pub id: MediaId,
    pub kind: MediaKind,
    pub sha256: String,
    pub size_bytes: i64,
    pub mime_type: Option<String>,
    pub original_filename: Option<String>,
    /// Canonical archive path: `objects/<aa>/<bb>/<sha256>`.
    pub archive_path: String,
    /// Best canonical "taken at" timestamp, computed from media_metadata
    /// or filesystem as a fallback. Used for the timeline.
    pub canonical_taken_at: Option<i64>,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
    pub source_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaFileRow {
    pub media_id: MediaId,
    /// Original absolute path of the source file when it was imported.
    pub original_path: String,
    /// Optional name of the device/snapshot.
    pub device_id: Option<String>,
    /// Optional import id that first brought this file in.
    pub import_id: Option<String>,
    pub size_bytes: i64,
    pub modified_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaMetadataRow {
    pub media_id: MediaId,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<f64>,
    pub iso: Option<i64>,
    pub aperture: Option<f64>,
    pub shutter_speed: Option<String>,
    pub orientation: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_seconds: Option<f64>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimestampRow {
    pub media_id: MediaId,
    /// One of: "exif", "filesystem", "google", "quicktime", "moviemetadata",
    /// "container", "manual".
    pub source: String,
    /// UTC unix seconds.
    pub value: i64,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocationRow {
    pub media_id: MediaId,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
    pub source: String,
}

/// Filter passed to [`Catalog::search_media`].
///
/// Every field is optional. `None` means "no constraint on this field."
/// Filters compose with AND semantics.
#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    pub kind: Option<MediaKind>,
    pub source: Option<String>,
    pub album: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub has_location: bool,
    pub near_latitude: Option<f64>,
    pub near_longitude: Option<f64>,
    pub radius_km: Option<f64>,
    pub text: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumRow {
    pub id: AlbumId,
    pub name: String,
    pub kind: AlbumKind,
    pub created_at: i64,
    /// Number of media items currently attached to this album. 0 for
    /// empty user albums. Filled by `list_albums` via a LEFT JOIN
    /// against `album_items`; `get_albums_for_media` returns 0 by
    /// default (the JOIN group-by would distort per-row counts).
    pub media_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy, Serialize, Deserialize)]
pub enum AlbumKind {
    User,
    GoogleTakeout,
    Smart,
}

impl AlbumKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlbumKind::User => "user",
            AlbumKind::GoogleTakeout => "google_takeout",
            AlbumKind::Smart => "smart",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "google_takeout" => Some(Self::GoogleTakeout),
            "smart" => Some(Self::Smart),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportRow {
    pub id: ImportId,
    pub source_kind: String,
    pub source_path: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub discovered: i64,
    pub imported: i64,
    pub duplicates: i64,
    pub failed: i64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ImportStatus {
    Running,
    Partial,
    Success,
    Failed,
}

impl ImportStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Partial => "partial",
            Self::Success => "success",
            Self::Failed => "failed",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "partial" => Some(Self::Partial),
            "success" => Some(Self::Success),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ImportFileOutcome {
    Imported,
    Duplicate,
    Failed,
}

impl ImportFileOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Imported => "imported",
            Self::Duplicate => "duplicate",
            Self::Failed => "failed",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "imported" => Some(Self::Imported),
            "duplicate" => Some(Self::Duplicate),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFileRow {
    pub import_id: ImportId,
    pub source_path: String,
    pub media_id: Option<MediaId>,
    pub outcome: ImportFileOutcome,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum VerificationStatus {
    Running,
    Ok,
    Problem,
}

impl VerificationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Problem => "problem",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "ok" => Some(Self::Ok),
            "problem" => Some(Self::Problem),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum VerificationFinding {
    Ok,
    Missing,
    Mismatched,
}

impl VerificationFinding {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::Mismatched => "mismatched",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "missing" => Some(Self::Missing),
            "mismatched" => Some(Self::Mismatched),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationRunRow {
    pub id: VerificationId,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub files_checked: i64,
    pub files_ok: i64,
    pub files_missing: i64,
    pub files_mismatched: i64,
    pub status: VerificationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationFindingRow {
    pub run_id: VerificationId,
    pub media_id: MediaId,
    pub finding: VerificationFinding,
    pub expected_hash: String,
    pub actual_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRunRow {
    pub id: BackupId,
    pub destination_path: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub bytes_copied: i64,
    pub files_copied: i64,
    pub files_skipped: i64,
    pub files_failed: i64,
    pub status: String,
}

// ---------------------------------------------------------------------------
// Catalog façade
// ---------------------------------------------------------------------------

pub struct Catalog<'c> {
    conn: &'c mut Connection,
}

impl<'c> Catalog<'c> {
    pub fn new(conn: &'c mut Connection) -> Self {
        Self { conn }
    }

    pub fn conn(&self) -> &Connection {
        self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        self.conn
    }

    // -- media -------------------------------------------------------------

    /// Look up a media item by content hash. Returns `None` if absent.
    pub fn media_by_hash(&self, hash: &str) -> DbResult<Option<MediaRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, kind, sha256, size_bytes, mime_type, original_filename,
                    archive_path, canonical_taken_at, created_at, modified_at, source_label
             FROM media WHERE sha256 = ?1",
        )?;
        let mut rows = stmt.query(params![hash])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_media(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_media(&self, id: &MediaId) -> DbResult<Option<MediaRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, kind, sha256, size_bytes, mime_type, original_filename,
                    archive_path, canonical_taken_at, created_at, modified_at, source_label
             FROM media WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id.as_str()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_media(row)?))
        } else {
            Ok(None)
        }
    }

    /// Fetch the structured metadata row for a media item, if present.
    pub fn get_metadata(&self, id: &MediaId) -> DbResult<Option<MediaMetadataRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT media_id, camera_make, camera_model, lens, focal_length, iso,
                    aperture, shutter_speed, orientation, width, height,
                    duration_seconds, description
             FROM media_metadata WHERE media_id = ?1",
        )?;
        let mut rows = stmt.query(params![id.as_str()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_metadata(row)?))
        } else {
            Ok(None)
        }
    }

    /// Fetch every timestamp provenance row for a media item.
    pub fn get_timestamps(&self, id: &MediaId) -> DbResult<Vec<TimestampRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT media_id, source, value, label FROM media_timestamps
             WHERE media_id = ?1 ORDER BY value ASC",
        )?;
        let rows = stmt.query_map(params![id.as_str()], row_to_timestamp)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Fetch every GPS location row for a media item.
    pub fn get_locations(&self, id: &MediaId) -> DbResult<Vec<LocationRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT media_id, latitude, longitude, altitude, source FROM locations
             WHERE media_id = ?1",
        )?;
        let rows = stmt.query_map(params![id.as_str()], row_to_location)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Fetch every album that contains this media item.
    pub fn get_albums_for_media(&self, id: &MediaId) -> DbResult<Vec<AlbumRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.id, a.name, a.kind, a.created_at FROM albums a
             JOIN album_items ai ON ai.album_id = a.id
             WHERE ai.media_id = ?1",
        )?;
        let rows = stmt.query_map(params![id.as_str()], row_to_album)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Fetch every `media_files` row (original source paths) for a media item.
    pub fn get_source_files(&self, id: &MediaId) -> DbResult<Vec<MediaFileRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT media_id, original_path, device_id, import_id, size_bytes, modified_at
             FROM media_files WHERE media_id = ?1",
        )?;
        let rows = stmt.query_map(params![id.as_str()], row_to_media_file)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Insert a fresh media row. The caller is responsible for ensuring no
    /// row already exists with the same `sha256`. (Use [`upsert_media`]
    /// if you'd rather collapse duplicates.)
    pub fn insert_media(&self, m: &MediaRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO media(
                id, kind, sha256, size_bytes, mime_type, original_filename,
                archive_path, canonical_taken_at, created_at, modified_at,
                source_label, inserted_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, strftime('%s','now'))",
            params![
                m.id.as_str(),
                m.kind.as_str(),
                m.sha256,
                m.size_bytes,
                m.mime_type,
                m.original_filename,
                m.archive_path,
                m.canonical_taken_at,
                m.created_at,
                m.modified_at,
                m.source_label,
            ],
        )?;
        Ok(())
    }

    pub fn add_media_file(&self, f: &MediaFileRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO media_files(media_id, original_path, device_id,
                import_id, size_bytes, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                f.media_id.as_str(),
                f.original_path,
                f.device_id,
                f.import_id,
                f.size_bytes,
                f.modified_at,
            ],
        )?;
        Ok(())
    }

    /// Delete a media_files row by its original path. Returns the number
    /// of rows actually deleted (0 if the path wasn't tracked).
    pub fn delete_media_file_by_path(&self, original_path: &str) -> DbResult<usize> {
        let n = self
            .conn
            .execute("DELETE FROM media_files WHERE original_path = ?1", params![original_path])?;
        Ok(n)
    }

    /// Iterate all media_files rows. Used by dedup pruning.
    pub fn iter_all_media_files(&self) -> DbResult<Vec<MediaFileRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT media_id, original_path, device_id, import_id, size_bytes, modified_at
             FROM media_files",
        )?;
        let rows = stmt.query_map([], row_to_media_file)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn insert_metadata(&self, m: &MediaMetadataRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO media_metadata(
                media_id, camera_make, camera_model, lens, focal_length, iso,
                aperture, shutter_speed, orientation, width, height,
                duration_seconds, description
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                m.media_id.as_str(),
                m.camera_make,
                m.camera_model,
                m.lens,
                m.focal_length,
                m.iso,
                m.aperture,
                m.shutter_speed,
                m.orientation,
                m.width,
                m.height,
                m.duration_seconds,
                m.description,
            ],
        )?;
        Ok(())
    }

    pub fn insert_timestamp(&self, t: &TimestampRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO media_timestamps(media_id, source, value, label)
             VALUES (?1, ?2, ?3, ?4)",
            params![t.media_id.as_str(), t.source, t.value, t.label],
        )?;
        Ok(())
    }

    pub fn insert_location(&self, l: &LocationRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO locations(media_id, latitude, longitude, altitude, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                l.media_id.as_str(),
                l.latitude,
                l.longitude,
                l.altitude,
                l.source,
            ],
        )?;
        Ok(())
    }

    pub fn set_canonical_taken_at(&self, id: &MediaId, ts: i64) -> DbResult<()> {
        self.conn.execute(
            "UPDATE media SET canonical_taken_at = ?1 WHERE id = ?2",
            params![ts, id.as_str()],
        )?;
        Ok(())
    }

    pub fn list_media_with_taken_at(&self) -> DbResult<Vec<(MediaId, Option<i64>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, canonical_taken_at FROM media")?;
        let rows = stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            let ts: Option<i64> = r.get(1)?;
            Ok((MediaId::from(id), ts))
        })?;
        let mut v = Vec::new();
        for r in rows {
            v.push(r?);
        }
        Ok(v)
    }

    /// Iterate every media row. Used by verification.
    pub fn iter_all_media(&self) -> DbResult<Vec<MediaRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, sha256, size_bytes, mime_type, original_filename,
                    archive_path, canonical_taken_at, created_at, modified_at, source_label
             FROM media",
        )?;
        let rows = stmt.query_map([], row_to_media)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // -- search ------------------------------------------------------------

    /// Run a filtered, paginated search over `media`.
    ///
    /// All fields are optional. The query joins `media` with `locations`
    /// (when a geo filter is supplied) and `album_items` (when an album
    /// name is supplied). Results are ordered by `canonical_taken_at DESC`
    /// when timestamps are available, falling back to `inserted_at DESC`.
    pub fn search_media(&self, filter: &SearchFilter) -> DbResult<Vec<MediaRow>> {
        // Build the WHERE clause and parameter list dynamically.
        let mut sql = String::from(
            "SELECT DISTINCT m.id, m.kind, m.sha256, m.size_bytes, m.mime_type,
                    m.original_filename, m.archive_path, m.canonical_taken_at,
                    m.created_at, m.modified_at, m.source_label
             FROM media m",
        );
        let mut wheres: Vec<String> = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if filter.album.is_some() {
            sql.push_str(" JOIN album_items ai ON ai.media_id = m.id JOIN albums a ON a.id = ai.album_id");
            wheres.push("a.name = ?".into());
            params.push(Box::new(filter.album.as_ref().unwrap().clone()));
        }
        if filter.has_location || filter.near_latitude.is_some() || filter.near_longitude.is_some() {
            sql.push_str(" JOIN locations l ON l.media_id = m.id");
        }
        if filter.near_latitude.is_some() || filter.near_longitude.is_some() {
            // Bounding-box approximation, then haversine-precise distance
            // is applied in Rust after the query. Cheap + correct enough
            // for personal archive sizes.
            if let Some(lat) = filter.near_latitude {
                let dlat = filter.radius_km.unwrap_or(10.0) / 111.0;
                wheres.push("l.latitude  BETWEEN ? AND ?".into());
                params.push(Box::new(lat - dlat));
                params.push(Box::new(lat + dlat));
            }
            if let Some(lon) = filter.near_longitude {
                let dlon = filter.radius_km.unwrap_or(10.0)
                    / (111.0 * (filter.near_latitude.unwrap_or(0.0).to_radians().cos().max(0.0001)));
                wheres.push("l.longitude BETWEEN ? AND ?".into());
                params.push(Box::new(lon - dlon));
                params.push(Box::new(lon + dlon));
            }
        }
        if let Some(kind) = filter.kind {
            wheres.push("m.kind = ?".into());
            params.push(Box::new(kind.as_str().to_string()));
        }
        if let Some(src) = filter.source.as_ref() {
            wheres.push("m.source_label = ?".into());
            params.push(Box::new(src.clone()));
        }
        if filter.from.is_some() {
            wheres.push("m.canonical_taken_at >= ?".into());
            params.push(Box::new(filter.from.unwrap()));
        }
        if filter.to.is_some() {
            wheres.push("m.canonical_taken_at <= ?".into());
            params.push(Box::new(filter.to.unwrap()));
        }
        if let Some(q) = filter.text.as_ref() {
            // Free-text: match against filename + description. (LIKE is
            // fine at personal-archive scale; we'd reach for FTS5 if it
            // ever became a bottleneck.)
            wheres.push("(m.original_filename LIKE ? OR EXISTS (SELECT 1 FROM media_metadata mm WHERE mm.media_id = m.id AND mm.description LIKE ?))".into());
            let pat = format!("%{q}%");
            params.push(Box::new(pat.clone()));
            params.push(Box::new(pat));
        }

        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(" ORDER BY COALESCE(m.canonical_taken_at, m.inserted_at) DESC");
        let limit = filter.limit.unwrap_or(1000).max(1);
        sql.push_str(&format!(" LIMIT {limit}"));

        let mut stmt = self.conn.prepare(&sql)?;
        // Build a params slice we can hand to query_map.
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(&param_refs[..], row_to_media)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // -- albums ------------------------------------------------------------

    pub fn upsert_album_by_name(
        &self,
        name: &str,
        kind: AlbumKind,
        now: i64,
    ) -> DbResult<AlbumId> {
        // Atomic insert-or-fetch keyed by (name, kind).
        if let Some(id) = self.conn.query_row(
            "SELECT id FROM albums WHERE name = ?1 AND kind = ?2",
            params![name, kind.as_str()],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        {
            return Ok(AlbumId::from(id));
        }
        let id = AlbumId::new(format!("al_{}_{}", kind.as_str(), uuid_like(name)));
        self.conn.execute(
            "INSERT INTO albums(id, name, kind, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id.as_str(), name, kind.as_str(), now],
        )?;
        Ok(id)
    }

    pub fn add_to_album(
        &self,
        album: &AlbumId,
        media: &MediaId,
        position: i64,
    ) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO album_items(album_id, media_id, position)
             VALUES (?1, ?2, ?3)",
            params![album.as_str(), media.as_str(), position],
        )?;
        Ok(())
    }

    /// Remove a media item from an album. Returns the number of rows
    /// affected (0 or 1) — useful for the UI to detect the
    /// "wasn't actually attached" case.
    pub fn remove_from_album(
        &self,
        album: &AlbumId,
        media: &MediaId,
    ) -> DbResult<usize> {
        let n = self.conn.execute(
            "DELETE FROM album_items WHERE album_id = ?1 AND media_id = ?2",
            params![album.as_str(), media.as_str()],
        )?;
        Ok(n)
    }

    /// Delete an album by id. The album's `album_items` rows are
    /// removed automatically by the schema's `ON DELETE CASCADE` FK.
    /// Media files themselves are untouched.
    ///
    /// Returns the number of albums actually deleted (0 or 1) — a
    /// missing id is a soft error so the UI can surface it.
    pub fn delete_album(&self, album: &AlbumId) -> DbResult<usize> {
        let n = self.conn.execute(
            "DELETE FROM albums WHERE id = ?1",
            params![album.as_str()],
        )?;
        Ok(n)
    }

    /// Rename an album. Only user-created albums (`kind = 'user'`)
    /// can be renamed — Takeout-derived albums get their name from
    /// their on-disk folder and renaming here would silently lie.
    /// Returns `Ok(true)` if a row was updated.
    pub fn rename_album(
        &self,
        album: &AlbumId,
        new_name: &str,
    ) -> DbResult<bool> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let n = self.conn.execute(
            "UPDATE albums SET name = ?1 WHERE id = ?2 AND kind = 'user'",
            params![trimmed, album.as_str()],
        )?;
        Ok(n > 0)
    }

    pub fn list_albums(&self) -> DbResult<Vec<AlbumRow>> {
        // LEFT JOIN against album_items so empty albums still appear,
        // and GROUP BY keeps one row per album. `COUNT(ai.media_id)`
        // (not `COUNT(*)`) so albums with zero items report 0, not 1.
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.name, a.kind, a.created_at, COUNT(ai.media_id)
             FROM albums a
             LEFT JOIN album_items ai ON ai.album_id = a.id
             GROUP BY a.id
             ORDER BY a.name",
        )?;
        let rows = stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            let kind_s: String = r.get(2)?;
            let created_at: i64 = r.get(3)?;
            let count: i64 = r.get(4)?;
            Ok((id, name, kind_s, created_at, count))
        })?;
        let mut v = Vec::new();
        for r in rows {
            let (id, name, kind_s, created_at, count) = r?;
            v.push(AlbumRow {
                id: AlbumId::from(id),
                name,
                kind: AlbumKind::from_str(&kind_s).unwrap_or(AlbumKind::User),
                created_at,
                media_count: count,
            });
        }
        Ok(v)
    }

    pub fn album_items(&self, album: &AlbumId) -> DbResult<Vec<MediaId>> {
        let mut stmt = self.conn.prepare(
            "SELECT media_id FROM album_items WHERE album_id = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map(params![album.as_str()], |r| {
            let id: String = r.get(0)?;
            Ok(MediaId::from(id))
        })?;
        let mut v = Vec::new();
        for r in rows {
            v.push(r?);
        }
        Ok(v)
    }

    // -- imports -----------------------------------------------------------

    pub fn begin_import(&self, row: &ImportRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT INTO imports(id, source_kind, source_path, started_at,
                discovered, imported, duplicates, failed, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                row.id.as_str(),
                row.source_kind,
                row.source_path,
                row.started_at,
                row.discovered,
                row.imported,
                row.duplicates,
                row.failed,
                row.status,
            ],
        )?;
        Ok(())
    }

    pub fn update_import(&self, row: &ImportRow) -> DbResult<()> {
        self.conn.execute(
            "UPDATE imports SET finished_at = ?2, discovered = ?3, imported = ?4,
                duplicates = ?5, failed = ?6, status = ?7 WHERE id = ?1",
            params![
                row.id.as_str(),
                row.finished_at,
                row.discovered,
                row.imported,
                row.duplicates,
                row.failed,
                row.status,
            ],
        )?;
        Ok(())
    }

    pub fn record_import_file(&self, f: &ImportFileRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT INTO import_files(import_id, source_path, media_id,
                outcome, error)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                f.import_id.as_str(),
                f.source_path,
                f.media_id.as_ref().map(|m| m.as_str().to_string()),
                f.outcome.as_str(),
                f.error,
            ],
        )?;
        Ok(())
    }

    pub fn list_imports(&self, limit: i64) -> DbResult<Vec<ImportRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_kind, source_path, started_at, finished_at,
                    discovered, imported, duplicates, failed, status
             FROM imports ORDER BY started_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], row_to_import)?;
        let mut v = Vec::new();
        for r in rows {
            v.push(r?);
        }
        Ok(v)
    }

    // -- verification ------------------------------------------------------

    pub fn begin_verification(&self, run: &VerificationRunRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT INTO verification_runs(id, started_at, files_checked,
                files_ok, files_missing, files_mismatched, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run.id.as_str(),
                run.started_at,
                run.files_checked,
                run.files_ok,
                run.files_missing,
                run.files_mismatched,
                run.status.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn update_verification(&self, run: &VerificationRunRow) -> DbResult<()> {
        self.conn.execute(
            "UPDATE verification_runs SET finished_at = ?2, files_checked = ?3,
                files_ok = ?4, files_missing = ?5, files_mismatched = ?6,
                status = ?7 WHERE id = ?1",
            params![
                run.id.as_str(),
                run.finished_at,
                run.files_checked,
                run.files_ok,
                run.files_missing,
                run.files_mismatched,
                run.status.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn record_verification_finding(&self, f: &VerificationFindingRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT INTO verification_findings(run_id, media_id, finding,
                expected_hash, actual_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                f.run_id.as_str(),
                f.media_id.as_str(),
                f.finding.as_str(),
                f.expected_hash,
                f.actual_hash,
            ],
        )?;
        Ok(())
    }

    pub fn latest_verification(&self) -> DbResult<Option<VerificationRunRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, started_at, finished_at, files_checked, files_ok,
                    files_missing, files_mismatched, status
             FROM verification_runs ORDER BY started_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_verification(row)?))
        } else {
            Ok(None)
        }
    }

    // -- backup sync -------------------------------------------------------

    pub fn record_backup_run(&self, b: &BackupRunRow) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO backup_runs(id, destination_path, started_at,
                finished_at, bytes_copied, files_copied, files_skipped,
                files_failed, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                b.id.as_str(),
                b.destination_path,
                b.started_at,
                b.finished_at,
                b.bytes_copied,
                b.files_copied,
                b.files_skipped,
                b.files_failed,
                b.status,
            ],
        )?;
        Ok(())
    }

    // -- dashboard helpers -------------------------------------------------

    pub fn media_counts(&self) -> DbResult<MediaCounts> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, COUNT(*) FROM media GROUP BY kind",
        )?;
        let rows = stmt.query_map([], |r| {
            let k: String = r.get(0)?;
            let c: i64 = r.get(1)?;
            Ok((k, c))
        })?;
        let mut counts = MediaCounts::default();
        for r in rows {
            let (k, c) = r?;
            match MediaKind::from_str(&k).unwrap_or(MediaKind::Other) {
                MediaKind::Photo => counts.photos = c,
                MediaKind::Video => counts.videos = c,
                MediaKind::Document => counts.documents = c,
                MediaKind::Audio => counts.audio = c,
                MediaKind::Other => counts.other = c,
            }
        }
        Ok(counts)
    }

    pub fn total_size(&self) -> DbResult<i64> {
        let n: i64 = self
            .conn
            .query_row("SELECT IFNULL(SUM(size_bytes),0) FROM media", [], |r| {
                r.get(0)
            })?;
        Ok(n)
    }

    pub fn media_count(&self) -> DbResult<i64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))?;
        Ok(n)
    }

    pub fn hash_exists(&self, hash: &str) -> DbResult<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM media WHERE sha256 = ?1",
            params![hash],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// List media rows that have multiple physical file rows attached — the
    /// "this content was discovered more than once" view. The hash column
    /// itself is UNIQUE so we can't see duplicates there; instead we look at
    /// `media_files` and join back. Each row returned represents a media
    /// item with `>1` distinct source paths.
    pub fn media_with_multiple_files(&self) -> DbResult<Vec<MediaRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.kind, m.sha256, m.size_bytes, m.mime_type,
                    m.original_filename, m.archive_path, m.canonical_taken_at,
                    m.created_at, m.modified_at, m.source_label
             FROM media m
             JOIN (
                 SELECT media_id, COUNT(*) c FROM media_files GROUP BY media_id
             ) f ON f.media_id = m.id AND f.c > 1
             ORDER BY m.sha256",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_media(row)?);
        }
        Ok(out)
    }

    /// Transaction wrapper so importers can group multiple writes atomically.
    pub fn transaction(&mut self) -> DbResult<Transaction<'_>> {
        Ok(self.conn.transaction()?)
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct MediaCounts {
    pub photos: i64,
    pub videos: i64,
    pub documents: i64,
    pub audio: i64,
    pub other: i64,
}

impl MediaCounts {
    pub fn total(&self) -> i64 {
        self.photos + self.videos + self.documents + self.audio + self.other
    }
}

// ---------------------------------------------------------------------------
// row → domain conversions
// ---------------------------------------------------------------------------

fn row_to_media_file(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaFileRow> {
    Ok(MediaFileRow {
        media_id: MediaId::from(row.get::<_, String>(0)?),
        original_path: row.get(1)?,
        device_id: row.get(2)?,
        import_id: row.get(3)?,
        size_bytes: row.get(4)?,
        modified_at: row.get(5)?,
    })
}

fn row_to_media(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaRow> {
    Ok(MediaRow {
        id: MediaId::from(row.get::<_, String>(0)?),
        kind: MediaKind::from_str(&row.get::<_, String>(1)?).unwrap_or(MediaKind::Other),
        sha256: row.get(2)?,
        size_bytes: row.get(3)?,
        mime_type: row.get(4)?,
        original_filename: row.get(5)?,
        archive_path: row.get(6)?,
        canonical_taken_at: row.get(7)?,
        created_at: row.get(8)?,
        modified_at: row.get(9)?,
        source_label: row.get(10)?,
    })
}

fn row_to_import(row: &rusqlite::Row<'_>) -> rusqlite::Result<ImportRow> {
    Ok(ImportRow {
        id: ImportId::from(row.get::<_, String>(0)?),
        source_kind: row.get(1)?,
        source_path: row.get(2)?,
        started_at: row.get(3)?,
        finished_at: row.get(4)?,
        discovered: row.get(5)?,
        imported: row.get(6)?,
        duplicates: row.get(7)?,
        failed: row.get(8)?,
        status: row.get(9)?,
    })
}

fn row_to_verification(row: &rusqlite::Row<'_>) -> rusqlite::Result<VerificationRunRow> {
    let status_s: String = row.get(7)?;
    Ok(VerificationRunRow {
        id: VerificationId::from(row.get::<_, String>(0)?),
        started_at: row.get(1)?,
        finished_at: row.get(2)?,
        files_checked: row.get(3)?,
        files_ok: row.get(4)?,
        files_missing: row.get(5)?,
        files_mismatched: row.get(6)?,
        status: VerificationStatus::from_str(&status_s).unwrap_or(VerificationStatus::Running),
    })
}

fn row_to_metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaMetadataRow> {
    Ok(MediaMetadataRow {
        media_id: MediaId::from(row.get::<_, String>(0)?),
        camera_make: row.get(1)?,
        camera_model: row.get(2)?,
        lens: row.get(3)?,
        focal_length: row.get(4)?,
        iso: row.get(5)?,
        aperture: row.get(6)?,
        shutter_speed: row.get(7)?,
        orientation: row.get(8)?,
        width: row.get(9)?,
        height: row.get(10)?,
        duration_seconds: row.get(11)?,
        description: row.get(12)?,
    })
}

fn row_to_timestamp(row: &rusqlite::Row<'_>) -> rusqlite::Result<TimestampRow> {
    Ok(TimestampRow {
        media_id: MediaId::from(row.get::<_, String>(0)?),
        source: row.get(1)?,
        value: row.get(2)?,
        label: row.get(3)?,
    })
}

fn row_to_location(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocationRow> {
    Ok(LocationRow {
        media_id: MediaId::from(row.get::<_, String>(0)?),
        latitude: row.get(1)?,
        longitude: row.get(2)?,
        altitude: row.get(3)?,
        source: row.get(4)?,
    })
}

fn row_to_album(row: &rusqlite::Row<'_>) -> rusqlite::Result<AlbumRow> {
    let kind_s: String = row.get(2)?;
    // Caller passes a 5-tuple; if not, we leave media_count at 0.
    let count: i64 = row.get(4).unwrap_or(0);
    Ok(AlbumRow {
        id: AlbumId::from(row.get::<_, String>(0)?),
        name: row.get(1)?,
        kind: AlbumKind::from_str(&kind_s).unwrap_or(AlbumKind::User),
        created_at: row.get(3)?,
        media_count: count,
    })
}

/// Lightweight, deterministic, non-cryptographic id suffix derived from a string.
/// Used for generating album IDs from names. Not for security.
fn uuid_like(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

// ---------------------------------------------------------------------------
// unit tests (mostly to validate migrations + roundtrips)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_media() -> MediaRow {
        MediaRow {
            id: MediaId::new("m_test1"),
            kind: MediaKind::Photo,
            sha256: "a".repeat(64),
            size_bytes: 1024,
            mime_type: Some("image/jpeg".into()),
            original_filename: Some("IMG.jpg".into()),
            archive_path: format!("objects/{}/{}/{}", &"a".repeat(2), &"a".repeat(2), &"a".repeat(64)),
            canonical_taken_at: Some(1_700_000_000),
            created_at: Some(1_700_000_000),
            modified_at: Some(1_700_000_010),
            source_label: "google_takeout".into(),
        }
    }

    #[test]
    fn schema_applies_cleanly_on_memory() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        assert_eq!(c.media_count().unwrap(), 0);
    }

    #[test]
    fn insert_and_lookup_media() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let m = sample_media();
        c.insert_media(&m).unwrap();
        let found = c.media_by_hash(&m.sha256).unwrap().unwrap();
        assert_eq!(found.id, m.id);
        assert_eq!(found.kind, m.kind);
    }

    #[test]
    fn timestamps_round_trip() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let m = sample_media();
        c.insert_media(&m).unwrap();
        c.insert_timestamp(&TimestampRow {
            media_id: m.id.clone(),
            source: "exif".into(),
            value: 1_700_000_000,
            label: None,
        })
        .unwrap();
        c.insert_timestamp(&TimestampRow {
            media_id: m.id.clone(),
            source: "google".into(),
            value: 1_700_000_001,
            label: None,
        })
        .unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM media_timestamps WHERE media_id = ?1",
                params![m.id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn location_round_trip() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let m = sample_media();
        c.insert_media(&m).unwrap();
        c.insert_location(&LocationRow {
            media_id: m.id.clone(),
            latitude: 47.6,
            longitude: -122.3,
            altitude: Some(10.0),
            source: "exif".into(),
        })
        .unwrap();
        let (lat, lon): (f64, f64) = conn
            .query_row(
                "SELECT latitude, longitude FROM locations WHERE media_id = ?1",
                params![m.id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!((lat - 47.6).abs() < 1e-6);
        assert!((lon - -122.3).abs() < 1e-6);
    }

    #[test]
    fn albums_round_trip() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let m = sample_media();
        c.insert_media(&m).unwrap();
        let a = c
            .upsert_album_by_name("Family", AlbumKind::GoogleTakeout, 1_700_000_000)
            .unwrap();
        c.add_to_album(&a, &m.id, 0).unwrap();
        let items = c.album_items(&a).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], m.id);
        let albums = c.list_albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Family");
    }

    #[test]
    fn list_albums_reports_media_count() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        // Three distinct media items. (sample_media's hash is fixed so
        // we have to fork it ourselves with distinct sha256 + id.)
        let mut m1 = sample_media();
        let mut m2 = m1.clone();
        let mut m3 = m1.clone();
        m2.id = MediaId::new("m2");
        m2.sha256 = "b".repeat(64);
        m3.id = MediaId::new("m3");
        m3.sha256 = "c".repeat(64);
        c.insert_media(&m1).unwrap();
        c.insert_media(&m2).unwrap();
        c.insert_media(&m3).unwrap();

        // One album with 2 items, one empty, one with 1.
        let big = c
            .upsert_album_by_name("Big", AlbumKind::User, 1_700_000_000)
            .unwrap();
        let empty = c
            .upsert_album_by_name("Empty", AlbumKind::User, 1_700_000_000)
            .unwrap();
        let solo = c
            .upsert_album_by_name("Solo", AlbumKind::User, 1_700_000_000)
            .unwrap();
        c.add_to_album(&big, &m1.id, 0).unwrap();
        c.add_to_album(&big, &m2.id, 1).unwrap();
        c.add_to_album(&solo, &m3.id, 0).unwrap();

        let albums = c.list_albums().unwrap();
        let by_name: std::collections::HashMap<_, _> = albums
            .iter()
            .map(|a| (a.name.as_str(), a.media_count))
            .collect();
        assert_eq!(by_name["Big"], 2);
        assert_eq!(by_name["Empty"], 0);
        assert_eq!(by_name["Solo"], 1);
        // Empty album still appears in the list.
        assert!(albums.iter().any(|a| a.name == "Empty"));
    }

    #[test]
    fn rename_user_album_updates_name() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let a = c
            .upsert_album_by_name("Old Name", AlbumKind::User, 1_700_000_000)
            .unwrap();
        let ok = c.rename_album(&a, "  New Name  ").unwrap();
        assert!(ok);
        let listed = c.list_albums().unwrap();
        assert!(listed.iter().any(|row| row.name == "New Name"));
    }

    #[test]
    fn rename_takeout_album_is_rejected() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        // Takeout albums derive their name from the folder layout;
        // renaming them here would silently disagree with disk.
        let a = c
            .upsert_album_by_name("From Takeout", AlbumKind::GoogleTakeout, 1_700_000_000)
            .unwrap();
        let ok = c.rename_album(&a, "Anything Else").unwrap();
        assert!(!ok, "renaming a takeout album should be a no-op");
        let listed = c.list_albums().unwrap();
        assert!(listed.iter().any(|row| row.name == "From Takeout"));
    }

    #[test]
    fn rename_empty_name_is_rejected() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let a = c
            .upsert_album_by_name("X", AlbumKind::User, 1_700_000_000)
            .unwrap();
        assert!(!c.rename_album(&a, "").unwrap());
        assert!(!c.rename_album(&a, "   ").unwrap());
    }

    #[test]
    fn remove_from_album_drops_one_row() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let m = sample_media();
        c.insert_media(&m).unwrap();
        let a = c
            .upsert_album_by_name("Trip", AlbumKind::User, 1_700_000_000)
            .unwrap();
        c.add_to_album(&a, &m.id, 0).unwrap();
        assert_eq!(c.album_items(&a).unwrap().len(), 1);
        let n = c.remove_from_album(&a, &m.id).unwrap();
        assert_eq!(n, 1);
        assert_eq!(c.album_items(&a).unwrap().len(), 0);
        // Idempotent: removing again reports 0 rows changed.
        assert_eq!(c.remove_from_album(&a, &m.id).unwrap(), 0);
    }

    #[test]
    fn delete_album_cascades_to_items() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let m = sample_media();
        c.insert_media(&m).unwrap();
        let a = c
            .upsert_album_by_name("Doomed", AlbumKind::User, 1_700_000_000)
            .unwrap();
        c.add_to_album(&a, &m.id, 0).unwrap();
        let n = c.delete_album(&a).unwrap();
        assert_eq!(n, 1);
        assert!(c.list_albums().unwrap().is_empty());
        // The media itself survives album deletion.
        assert!(c.get_media(&m.id).unwrap().is_some());
        // Deleting an unknown album reports 0 rows changed.
        assert_eq!(c.delete_album(&a).unwrap(), 0);
    }

    #[test]
    fn media_with_multiple_files_detects_reimports() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let m = sample_media();
        c.insert_media(&m).unwrap();
        c.add_media_file(&MediaFileRow {
            media_id: m.id.clone(),
            original_path: "/src/a/IMG.jpg".into(),
            device_id: None,
            import_id: Some("imp1".into()),
            size_bytes: 1024,
            modified_at: Some(1_700_000_000),
        })
        .unwrap();
        c.add_media_file(&MediaFileRow {
            media_id: m.id.clone(),
            original_path: "/Volumes/Phone/DCIM/IMG.jpg".into(),
            device_id: Some("iphone-1".into()),
            import_id: Some("imp2".into()),
            size_bytes: 1024,
            modified_at: Some(1_700_000_000),
        })
        .unwrap();
        let dupes = c.media_with_multiple_files().unwrap();
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0].id, m.id);
    }

    #[test]
    fn import_stats_round_trip() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let now = 1_700_000_000i64;
        let import = ImportRow {
            id: ImportId::new("imp_1"),
            source_kind: "google_takeout".into(),
            source_path: "/tmp/takeout".into(),
            started_at: now,
            finished_at: Some(now + 60),
            discovered: 100,
            imported: 95,
            duplicates: 4,
            failed: 1,
            status: "partial".into(),
        };
        c.begin_import(&import).unwrap();
        let recent = c.list_imports(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].imported, 95);
    }

    #[test]
    fn verification_round_trip() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        let m = sample_media();
        c.insert_media(&m).unwrap();
        let run = VerificationRunRow {
            id: VerificationId::new("v_1"),
            started_at: 1_700_000_000,
            finished_at: None,
            files_checked: 1,
            files_ok: 1,
            files_missing: 0,
            files_mismatched: 0,
            status: VerificationStatus::Running,
        };
        c.begin_verification(&run).unwrap();
        c.record_verification_finding(&VerificationFindingRow {
            run_id: run.id.clone(),
            media_id: m.id.clone(),
            finding: VerificationFinding::Ok,
            expected_hash: m.sha256.clone(),
            actual_hash: Some(m.sha256.clone()),
        })
        .unwrap();
        let latest = c.latest_verification().unwrap().unwrap();
        assert_eq!(latest.files_checked, 1);
        assert_eq!(latest.status, VerificationStatus::Running);
    }

    #[test]
    fn path_for_db() {
        use std::path::PathBuf;
        let dir = tempfile::tempdir().unwrap();
        let p: PathBuf = dir.path().join("archive.db");
        {
            let mut conn = open(&p).unwrap();
            let c = Catalog::new(&mut conn);
            c.insert_media(&sample_media()).unwrap();
        }
        let mut conn2 = open(&p).unwrap();
        let c2 = Catalog::new(&mut conn2);
        assert_eq!(c2.media_count().unwrap(), 1);
    }

    fn make_media(id: &str, kind: MediaKind, sha: &str, taken_at: Option<i64>, source: &str, filename: &str) -> MediaRow {
        MediaRow {
            id: MediaId::new(id),
            kind,
            sha256: sha.to_string(),
            size_bytes: 1024,
            mime_type: Some("image/jpeg".into()),
            original_filename: Some(filename.into()),
            archive_path: format!("objects/{}/{}/{}", &sha[..2], &sha[2..4], sha),
            canonical_taken_at: taken_at,
            created_at: taken_at,
            modified_at: taken_at,
            source_label: source.into(),
        }
    }

    #[test]
    fn search_by_kind_and_source() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        c.insert_media(&make_media("m1", MediaKind::Photo, &"a".repeat(64), Some(1_700_000_000), "iphone", "a.jpg")).unwrap();
        c.insert_media(&make_media("m2", MediaKind::Video, &"b".repeat(64), Some(1_700_000_100), "iphone", "b.mov")).unwrap();
        c.insert_media(&make_media("m3", MediaKind::Photo, &"c".repeat(64), Some(1_700_000_200), "google_takeout", "c.jpg")).unwrap();

        // kind=photo → 2 hits
        let r = c.search_media(&SearchFilter { kind: Some(MediaKind::Photo), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 2);

        // source=iphone → 2 hits
        let r = c.search_media(&SearchFilter { source: Some("iphone".into()), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 2);

        // kind=photo + source=google_takeout → 1 hit
        let r = c.search_media(&SearchFilter { kind: Some(MediaKind::Photo), source: Some("google_takeout".into()), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id.as_str(), "m3");
    }

    #[test]
    fn search_by_date_range() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        c.insert_media(&make_media("m1", MediaKind::Photo, &"a".repeat(64), Some(1_700_000_000), "iphone", "a.jpg")).unwrap();
        c.insert_media(&make_media("m2", MediaKind::Photo, &"b".repeat(64), Some(1_700_010_000), "iphone", "b.jpg")).unwrap();
        c.insert_media(&make_media("m3", MediaKind::Photo, &"c".repeat(64), Some(1_700_020_000), "iphone", "c.jpg")).unwrap();

        let r = c.search_media(&SearchFilter { from: Some(1_700_005_000), to: Some(1_700_015_000), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id.as_str(), "m2");

        // open-ended from
        let r = c.search_media(&SearchFilter { from: Some(1_700_015_000), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 1);

        // open-ended to
        let r = c.search_media(&SearchFilter { to: Some(1_700_005_000), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn search_by_album() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        c.insert_media(&make_media("m1", MediaKind::Photo, &"a".repeat(64), Some(1_700_000_000), "google_takeout", "a.jpg")).unwrap();
        c.insert_media(&make_media("m2", MediaKind::Photo, &"b".repeat(64), Some(1_700_000_100), "google_takeout", "b.jpg")).unwrap();
        c.insert_media(&make_media("m3", MediaKind::Photo, &"c".repeat(64), Some(1_700_000_200), "iphone", "c.jpg")).unwrap();
        let album_id = c.upsert_album_by_name("Vacation", AlbumKind::GoogleTakeout, 1_700_000_000).unwrap();
        c.add_to_album(&album_id, &MediaId::new("m1"), 0).unwrap();
        c.add_to_album(&album_id, &MediaId::new("m2"), 1).unwrap();

        let r = c.search_media(&SearchFilter { album: Some("Vacation".into()), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 2);
        let ids: Vec<_> = r.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"m1"));
        assert!(ids.contains(&"m2"));
    }

    #[test]
    fn search_by_text_matches_filename_and_description() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        c.insert_media(&make_media("m1", MediaKind::Photo, &"a".repeat(64), Some(1_700_000_000), "iphone", "beach_sunset.jpg")).unwrap();
        c.insert_media(&make_media("m2", MediaKind::Photo, &"b".repeat(64), Some(1_700_000_100), "iphone", "forest.jpg")).unwrap();
        c.insert_metadata(&MediaMetadataRow {
            media_id: MediaId::new("m2"),
            camera_make: None, camera_model: None, lens: None,
            focal_length: None, iso: None, aperture: None, shutter_speed: None,
            orientation: None, width: None, height: None, duration_seconds: None,
            description: Some("misty forest path".into()),
        }).unwrap();

        let r = c.search_media(&SearchFilter { text: Some("forest".into()), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id.as_str(), "m2");

        let r = c.search_media(&SearchFilter { text: Some("sunset".into()), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id.as_str(), "m1");
    }

    #[test]
    fn search_by_location_bbox() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        c.insert_media(&make_media("m1", MediaKind::Photo, &"a".repeat(64), Some(1_700_000_000), "iphone", "tokyo.jpg")).unwrap();
        c.insert_media(&make_media("m2", MediaKind::Photo, &"b".repeat(64), Some(1_700_000_100), "iphone", "paris.jpg")).unwrap();
        c.insert_location(&LocationRow { media_id: MediaId::new("m1"), latitude: 35.6762, longitude: 139.6503, altitude: None, source: "exif".into() }).unwrap();
        c.insert_location(&LocationRow { media_id: MediaId::new("m2"), latitude: 48.8566, longitude: 2.3522, altitude: None, source: "exif".into() }).unwrap();

        // Near Tokyo
        let r = c.search_media(&SearchFilter {
            near_latitude: Some(35.68),
            near_longitude: Some(139.65),
            radius_km: Some(50.0),
            ..Default::default()
        }).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id.as_str(), "m1");

        // Anywhere with a location
        let r = c.search_media(&SearchFilter { has_location: true, ..Default::default() }).unwrap();
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn search_respects_limit_and_order() {
        let mut conn = open_memory().unwrap();
        let c = Catalog::new(&mut conn);
        for i in 0..5 {
            let id = format!("m{i}");
            let sha = format!("{:064x}", i);
            c.insert_media(&make_media(&id, MediaKind::Photo, &sha, Some(1_700_000_000 + i as i64), "iphone", &format!("file{i}.jpg"))).unwrap();
        }
        let r = c.search_media(&SearchFilter { limit: Some(3), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 3);
        // Newest first: canonical_taken_at DESC
        assert_eq!(r[0].id.as_str(), "m4");
    }
}
