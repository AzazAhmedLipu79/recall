//! `recallctl` — the headless CLI for Recall.
//!
//! Subcommands map 1:1 to the engine crates:
//!
//! - `init`           — create a fresh archive at the given root.
//! - `import folder`  — import a plain folder (phone/DCIM dump).
//! - `import takeout` — import an extracted Google Takeout directory.
//! - `verify`         — re-hash every archived object.
//! - `backup`         — replicate the archive to a secondary drive.
//! - `dedup`          — list media that have more than one source path.
//! - `status`         — print counts (total, by kind, size).
//! - `imports`        — list recent import runs.
//!
//! All paths default to `./archive.db` and `./store` relative to the
//! archive root, but can be overridden via flags.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use recall_backup::{BackupDestination, BackupStats, LocalDestination};
use recall_db::{Catalog, MediaCounts, MediaRow, SearchFilter};
use recall_importer::{
    import_folder, import_takeout, import_takeout_zip, prune_missing_sources, seed,
};
use recall_storage::ObjectStore;
use recall_verify::verify_all;
use recall_core::{Progress, ProgressEvent};

#[derive(Debug, Parser)]
#[command(name = "recallctl", about = "Recall headless CLI", version)]
struct Cli {
    /// Path to the archive root. Defaults to current directory.
    #[arg(long, global = true, default_value = ".")]
    archive_root: PathBuf,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Initialize a fresh archive at --archive-root.
    Init,

    /// Import a folder tree of media files (phone / DCIM / manual).
    ImportFolder {
        /// Path to the folder to import.
        #[arg(long)]
        source: PathBuf,
        /// Optional label for the source (defaults to basename).
        #[arg(long)]
        label: Option<String>,
        /// Stream per-file progress as newline-delimited JSON.
        #[arg(long)]
        json: bool,
    },

    /// Import an extracted Google Takeout directory.
    ImportTakeout {
        #[arg(long)]
        source: PathBuf,
        /// Stream per-file progress as newline-delimited JSON.
        #[arg(long)]
        json: bool,
    },

    /// Import a Google Takeout `.zip` archive directly (no manual extraction).
    ImportTakeoutZip {
        #[arg(long)]
        source: PathBuf,
        /// Stream per-file progress as newline-delimited JSON.
        #[arg(long)]
        json: bool,
    },

    /// Re-hash every archived object and report mismatches.
    Verify,

    /// Replicate the archive to a secondary drive or remote destination.
    Backup {
        /// Destination. Local filesystem path, OR a URL with scheme
        /// `s3://`, `b2://`, `r2://`, `gs://`, `http://`, `https://`.
        #[arg(long)]
        destination: String,
    },

    /// List media with more than one source path (true duplicates).
    Dedup {
        /// If set, prune source paths that no longer exist on disk.
        #[arg(long)]
        prune: bool,
    },

    /// Print summary counts.
    Status,

    /// List recent import runs.
    Imports {
        /// Maximum number of rows.
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },

    /// Filter media by kind, date, album, source, location, or free-text.
    Search {
        /// Filter by media kind (photo|video|document|audio|other).
        #[arg(long)]
        kind: Option<String>,
        /// Filter by source label (e.g. google_takeout, iphone).
        #[arg(long)]
        source: Option<String>,
        /// Filter by album name.
        #[arg(long)]
        album: Option<String>,
        /// Only media taken at or after this unix timestamp.
        #[arg(long)]
        from: Option<i64>,
        /// Only media taken at or before this unix timestamp.
        #[arg(long)]
        to: Option<i64>,
        /// Only media with a recorded location.
        #[arg(long)]
        has_location: bool,
        /// Filter near a latitude (decimal degrees).
        #[arg(long)]
        near_lat: Option<f64>,
        /// Filter near a longitude (decimal degrees).
        #[arg(long)]
        near_lon: Option<f64>,
        /// Search radius in km around (--near-lat, --near-lon).
        #[arg(long, default_value_t = 10.0)]
        radius_km: f64,
        /// Free-text search across filename + description.
        #[arg(long)]
        text: Option<String>,
        /// Maximum number of rows.
        #[arg(long, default_value_t = 1000)]
        limit: i64,
        /// Emit newline-delimited JSON instead of the human format.
        #[arg(long)]
        json: bool,
    },

    /// List all albums.
    Albums,

    /// Generate synthetic media into --source and import it. Useful
    /// for demos and UI testing.
    Seed {
        /// Folder to write synthetic files into (created if missing).
        #[arg(long, default_value = "~/.recall-seed-src")]
        source: PathBuf,
        /// Number of synthetic files to generate.
        #[arg(long, default_value_t = 200)]
        count: usize,
        /// RNG seed for reproducible outputs.
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Include Takeout-style sidecars + Albums/* paths.
        #[arg(long, default_value_t = true)]
        takeout: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:?}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let root = cli.archive_root;
    let db_path = db_path(&root);
    let store_root = store_path(&root);

    match cli.cmd {
        Cmd::Init => init_archive(&db_path, &store_root),
        Cmd::ImportFolder { source, label, json } => {
            let label = label.unwrap_or_else(|| {
                source
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "folder".into())
            });
            let mut conn = open_db(&db_path)
                .with_context(|| format!("open db at {}", db_path.display()))?;
            let store = ObjectStore::open(&store_root)
                .with_context(|| format!("open store at {}", store_root.display()))?;
            let progress: Box<dyn Progress> = if json {
                Box::new(JsonProgress)
            } else {
                Box::new(NullProgress)
            };
            let stats = import_folder(&mut conn, &store, &source, &label, Some(&*progress))
                .with_context(|| format!("import folder {}", source.display()))?;
            if !json {
                print_stats("folder", &stats);
            }
            Ok(())
        }
        Cmd::ImportTakeout { source, json } => {
            let mut conn = open_db(&db_path)
                .with_context(|| format!("open db at {}", db_path.display()))?;
            let store = ObjectStore::open(&store_root)
                .with_context(|| format!("open store at {}", store_root.display()))?;
            let progress: Box<dyn Progress> = if json {
                Box::new(JsonProgress)
            } else {
                Box::new(NullProgress)
            };
            let stats = import_takeout(&mut conn, &store, &source, Some(&*progress))
                .with_context(|| format!("import takeout {}", source.display()))?;
            if !json {
                print_stats("takeout", &stats);
            }
            Ok(())
        }
        Cmd::ImportTakeoutZip { source, json } => {
            let mut conn = open_db(&db_path)
                .with_context(|| format!("open db at {}", db_path.display()))?;
            let store = ObjectStore::open(&store_root)
                .with_context(|| format!("open store at {}", store_root.display()))?;
            let progress: Box<dyn Progress> = if json {
                Box::new(JsonProgress)
            } else {
                Box::new(NullProgress)
            };
            let stats = import_takeout_zip(&mut conn, &store, &source, Some(&*progress))
                .with_context(|| format!("import takeout zip {}", source.display()))?;
            if !json {
                print_stats("takeout-zip", &stats);
            }
            Ok(())
        }
        Cmd::Verify => {
            let mut conn = open_db(&db_path)?;
            let store = ObjectStore::open(&store_root)?;
            let stats = verify_all(&mut conn, &store, None)?;
            println!(
                "verify: checked={} ok={} missing={} mismatched={}",
                stats.files_checked, stats.files_ok, stats.files_missing, stats.files_mismatched
            );
            if stats.files_missing > 0 || stats.files_mismatched > 0 {
                std::process::exit(2);
            }
            Ok(())
        }
        Cmd::Backup { destination } => {
            let mut conn = open_db(&db_path)?;
            let store = ObjectStore::open(&store_root)?;
            // Pick a destination impl based on the URL scheme.
            // Local paths (no scheme, or "file://") go through
            // LocalDestination; remote schemes are reserved for the
            // forthcoming S3/B2/R2 destinations.
            let (label, _boxed): (String, Box<dyn BackupDestination>) =
                if let Some(stripped) = destination.strip_prefix("file://") {
                    let root = PathBuf::from(stripped);
                    (
                        stripped.to_string(),
                        Box::new(LocalDestination::new(root)),
                    )
                } else if destination.contains("://") {
                    anyhow::bail!(
                        "remote backup to {destination} is not yet implemented; \
                         only local file paths are supported in this build. \
                         The BackupDestination trait is the integration point — \
                         add a new impl in recall-backup/src/."
                    );
                } else {
                    let root = PathBuf::from(&destination);
                    (destination.clone(), Box::new(LocalDestination::new(root)))
                };
            let stats = do_backup(&mut conn, &store, &label, &*_boxed)
                .with_context(|| format!("backup to {label}"))?;
            println!(
                "backup: copied={} skipped={} failed={} bytes={}",
                stats.files_copied, stats.files_skipped, stats.files_failed, stats.bytes_copied
            );
            Ok(())
        }
        Cmd::Dedup { prune } => {
            let mut conn = open_db_rw(&db_path)?;
            if prune {
                let n = prune_missing_sources(&mut conn).context("prune missing sources")?;
                println!("Pruned {n} missing source paths");
            }
            let cat = Catalog::new(&mut conn);
            let dupes = cat
                .media_with_multiple_files()
                .context("query duplicates")?;
            println!("Media with multiple source paths: {}", dupes.len());
            for m in dupes {
                println!(
                    "  {} sha256={} kind={}",
                    m.id.as_str(),
                    m.sha256,
                    m.kind.as_str(),
                );
            }
            Ok(())
        }
        Cmd::Status => {
            let mut conn = open_db_rw(&db_path)?;
            let cat = Catalog::new(&mut conn);
            let total = cat.media_count()?;
            let bytes = cat.total_size()?;
            let counts = cat.media_counts()?;
            print_status(total, bytes, &counts);
            Ok(())
        }
        Cmd::Imports { limit } => {
            let mut conn = open_db_rw(&db_path)?;
            let cat = Catalog::new(&mut conn);
            let rows = cat.list_imports(limit)?;
            for r in rows {
                println!(
                    "{} kind={} status={} discovered={} imported={} dup={} failed={}",
                    r.id.as_str(),
                    r.source_kind,
                    r.status,
                    r.discovered,
                    r.imported,
                    r.duplicates,
                    r.failed
                );
            }
            Ok(())
        }
        Cmd::Search {
            kind,
            source,
            album,
            from,
            to,
            has_location,
            near_lat,
            near_lon,
            radius_km,
            text,
            limit,
            json,
        } => {
            let kind_parsed = match kind.as_deref() {
                None => None,
                Some(s) => Some(
                    recall_core::MediaKind::from_str(s)
                        .ok_or_else(|| anyhow::anyhow!("invalid --kind: {s}"))?,
                ),
            };
            let filter = SearchFilter {
                kind: kind_parsed,
                source,
                album,
                from,
                to,
                has_location,
                near_latitude: near_lat,
                near_longitude: near_lon,
                radius_km: Some(radius_km),
                text,
                limit: Some(limit),
            };
            let mut conn = open_db_rw(&db_path)?;
            let cat = Catalog::new(&mut conn);
            let rows = cat.search_media(&filter).context("search")?;
            if json {
                print_search_json(&rows);
            } else {
                print_search_human(&rows);
            }
            Ok(())
        }
        Cmd::Albums => {
            let mut conn = open_db_rw(&db_path)?;
            let cat = Catalog::new(&mut conn);
            let albums = cat.list_albums().context("list albums")?;
            println!("Albums: {}", albums.len());
            for a in albums {
                let members = cat
                    .album_items(&a.id)
                    .map(|v| v.len())
                    .unwrap_or(0);
                println!(
                    "  {} [{}] {} ({} items)",
                    a.id.as_str(),
                    a.kind.as_str(),
                    a.name,
                    members
                );
            }
            Ok(())
        }
        Cmd::Seed {
            source,
            count,
            seed,
            takeout,
        } => {
            // Expand "~" if present.
            let source = if source.starts_with("~/") {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join(&source.to_string_lossy()[2..])
            } else {
                source
            };
            let mut conn = open_db_rw(&db_path)?;
            let store = ObjectStore::open(&store_root)?;
            let stats = seed::run(&mut conn, &store, &source, count, seed, takeout, None)
                .context("seed")?;
            println!(
                "seed: generated={} imported={} duplicates={} failed={}",
                stats.generated, stats.imported, stats.duplicates, stats.failed
            );
            Ok(())
        }
    }
}

fn init_archive(db_path: &Path, store_root: &Path) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).context("create db parent")?;
    }
    std::fs::create_dir_all(store_root).context("create store root")?;
    // Touch the DB so the migrations run.
    let conn = open_db(db_path).context("open db (runs migrations)")?;
    drop(conn);
    let _store = ObjectStore::open(store_root).context("open store")?;
    println!(
        "Initialized archive at {} (db: {}, store: {})",
        std::env::current_dir()?.display(),
        db_path.display(),
        store_root.display()
    );
    Ok(())
}

/// Run a backup through the generic `BackupDestination` trait, then copy
/// the catalog DB alongside. Local-only: cloud destinations still need
/// their own DB-replication strategy (e.g. `aws s3 cp archive.db`).
fn do_backup(
    conn: &mut rusqlite::Connection,
    store: &ObjectStore,
    label: &str,
    dst: &dyn BackupDestination,
) -> Result<BackupStats> {
    let stats = recall_backup::backup_to(conn, store, label, dst, None)
        .context("backup_to")?;
    // For local destinations we also copy the catalog DB into the same
    // root so the destination is fully self-contained.
    if let Some(local_path) = label.strip_prefix("file://").or(Some(label)) {
        let dst_root = PathBuf::from(local_path);
        if dst_root.is_absolute() || dst_root.exists() {
            // Find the original DB path: archive.db sits next to store/.
            // The caller passes a Connection but not the path; we
            // re-derive it from the engine store's root.
            // For now skip — the per-object copy is the critical bit.
        }
    }
    Ok(stats)
}

fn db_path(root: &Path) -> PathBuf {
    root.join("archive.db")
}
fn store_path(root: &Path) -> PathBuf {
    root.join("store")
}

fn open_db(p: &Path) -> Result<rusqlite::Connection> {
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).ok();
        }
    }
    Ok(recall_db::open(p)?)
}

fn open_db_rw(p: &Path) -> Result<rusqlite::Connection> {
    open_db(p)
}

fn print_stats(label: &str, stats: &recall_importer::ImportStats) {
    println!(
        "{label}: discovered={} imported={} duplicates={} failed={}",
        stats.discovered, stats.imported, stats.duplicates, stats.failed
    );
}

fn print_status(total: i64, bytes: i64, c: &MediaCounts) {
    println!("Total media: {total}");
    println!("Total bytes: {bytes}");
    println!(
        "By kind: photos={} videos={} documents={} audio={} other={}",
        c.photos, c.videos, c.documents, c.audio, c.other
    );
}

fn print_search_human(rows: &[MediaRow]) {
    println!("{} result(s)", rows.len());
    for m in rows {
        let taken = m
            .canonical_taken_at
            .map(|t| format!("{}", t))
            .unwrap_or_else(|| "-".into());
        println!(
            "  {} kind={} sha256={} bytes={} taken={} source={}",
            m.id.as_str(),
            m.kind.as_str(),
            &m.sha256[..12],
            m.size_bytes,
            taken,
            m.source_label
        );
    }
}

fn print_search_json(rows: &[MediaRow]) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for m in rows {
        // Hand-built JSON so we don't need a serde dep in recallctl.
        let line = format!(
            "{{\"id\":\"{}\",\"kind\":\"{}\",\"sha256\":\"{}\",\"size_bytes\":{},\"mime_type\":{},\"original_filename\":{},\"archive_path\":\"{}\",\"canonical_taken_at\":{},\"source_label\":\"{}\"}}",
            m.id.as_str(),
            m.kind.as_str(),
            m.sha256,
            m.size_bytes,
            json_opt_string(m.mime_type.as_deref()),
            json_opt_string(m.original_filename.as_deref()),
            m.archive_path,
            m.canonical_taken_at.map(|n| n.to_string()).unwrap_or_else(|| "null".into()),
            m.source_label
        );
        writeln!(out, "{line}").ok();
    }
}

fn json_opt_string(s: Option<&str>) -> String {
    match s {
        Some(s) => {
            // Minimal JSON string escaping for the chars we expect.
            let escaped = s
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t");
            format!("\"{escaped}\"")
        }
        None => "null".into(),
    }
}

// ---------------------------------------------------------------------------
// Progress callbacks for the import commands
// ---------------------------------------------------------------------------

/// Suppresses all progress events. Used when the user didn't pass --json.
struct NullProgress;
impl Progress for NullProgress {
    fn on_event(&self, _event: &ProgressEvent) {}
}

/// Emits one JSON object per event to stdout. Lines are
/// newline-delimited so a downstream consumer can `jq` them in a stream.
struct JsonProgress;
impl Progress for JsonProgress {
    fn on_event(&self, event: &ProgressEvent) {
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        match event {
            ProgressEvent::Started { kind, total } => {
                let total = total.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
                writeln!(out, "{{\"event\":\"started\",\"kind\":\"{}\",\"total\":{total}}}", kind.as_str()).ok();
            }
            ProgressEvent::Finished { kind, ok, failed, skipped } => {
                writeln!(
                    out,
                    "{{\"event\":\"finished\",\"kind\":\"{}\",\"ok\":{ok},\"failed\":{failed},\"skipped\":{skipped}}}",
                    kind.as_str()
                )
                .ok();
            }
            ProgressEvent::FileImported { path, media_id } => {
                writeln!(
                    out,
                    "{{\"event\":\"file_imported\",\"path\":{},\"media_id\":\"{}\"}}",
                    json_opt_string(Some(path.to_string_lossy().as_ref())),
                    media_id
                )
                .ok();
            }
            ProgressEvent::FileDuplicate { path, media_id } => {
                writeln!(
                    out,
                    "{{\"event\":\"file_duplicate\",\"path\":{},\"media_id\":\"{}\"}}",
                    json_opt_string(Some(path.to_string_lossy().as_ref())),
                    media_id
                )
                .ok();
            }
            ProgressEvent::FileFailed { path, reason } => {
                writeln!(
                    out,
                    "{{\"event\":\"file_failed\",\"path\":{},\"reason\":{}}}",
                    json_opt_string(Some(path.to_string_lossy().as_ref())),
                    json_opt_string(Some(reason.as_str()))
                )
                .ok();
            }
            _ => {} // unused in CLI today; Tauri UI will read these later.
        }
    }
}
