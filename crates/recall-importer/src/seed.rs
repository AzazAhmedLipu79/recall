//! Synthetic sample media generator.
//!
//! Originally lived in `recallctl/src/seed.rs`; moved here so both the
//! CLI and the desktop app can produce a sample archive on demand
//! (the desktop wizard uses it to populate a fresh archive with 20
//! demo photos on first launch).
//!
//! Generates minimal-JPEG photos at synthetic paths and timestamps,
//! then routes them through the normal `import_takeout` pipeline so
//! the catalog + object store end up in the same state they'd be in
//! after a real import. Album + sidecar metadata are produced for a
//! subset so the sidebar / inspector / map views have something to
//! display.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use rand::{rngs::StdRng, Rng, SeedableRng};
use rusqlite::Connection;

use recall_core::Progress;
use recall_storage::ObjectStore;

/// Stats reported by [`run`]. Kept tiny so the wizard UI can show
/// "imported 20 / duplicates 0 / failed 0" without further work.
#[derive(Debug, Clone, Copy)]
pub struct SeedStats {
    pub generated: usize,
    pub imported: i64,
    pub duplicates: i64,
    pub failed: i64,
}

/// Generate synthetic media under `dest`, then import it through
/// the normal engine pipeline.
///
/// - `count`: number of synthetic JPEGs to create.
/// - `seed`: RNG seed for reproducibility. Same seed → same data.
/// - `with_takeout`: if true, ~30% of files get a paired `.json`
///   sidecar and ~25% live under `Albums/<name>/` so the takeout
///   importer derives albums + structured metadata.
pub fn run(
    conn: &mut Connection,
    store: &ObjectStore,
    dest: &Path,
    count: usize,
    seed: u64,
    with_takeout: bool,
    progress: Option<&dyn Progress>,
) -> Result<SeedStats> {
    fs::create_dir_all(dest)
        .with_context(|| format!("create seed dest {}", dest.display()))?;

    let mut rng = StdRng::seed_from_u64(seed);

    // Spread files across 7 years so the timeline view has structure.
    let base_year = 2018;
    let year_span = 7;
    let albums = ["Vacation", "Work", "Family", "Tokyo Trip", "Recipes"];

    let mut files = Vec::with_capacity(count);

    for i in 0..count {
        // Choose album bucket. 25% chance for an album folder.
        let album_bucket: Option<&str> = if with_takeout && rng.gen_bool(0.25) {
            Some(albums[rng.gen_range(0..albums.len())])
        } else {
            None
        };

        let rel = match album_bucket {
            Some(name) => format!("Albums/{name}/img_{i:04}.jpg"),
            None => format!("Photos/2024/img_{i:04}.jpg"),
        };
        let path = dest.join(&rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // 1–500 KB of random JPEG-ish bytes.
        let size = rng.gen_range(1_000..500_000);
        write_minimal_jpeg(&path, size, &mut rng)?;

        // Spread mtime across `year_span` years for timeline variety.
        let secs_in_year: i64 = 365 * 24 * 3600;
        let year_offset = rng.gen_range(0..year_span) as i64 * secs_in_year;
        let day_offset = rng.gen_range(0..365) as i64 * 24 * 3600;
        let secs = Utc
            .with_ymd_and_hms(base_year, 1, 1, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp()
            + year_offset
            + day_offset;
        set_file_mtime(&path, secs)?;

        // 30% of files get a Takeout sidecar with a photoTakenTime +
        // a description.
        if with_takeout && rng.gen_bool(0.30) {
            let sidecar = format!(
                r#"{{
  "title": "img_{i}",
  "description": "{desc}",
  "photoTakenTime": {{ "timestamp": "{ts}", "formatted": "{fmt}" }},
  "geoData": {{ "latitude": {lat}, "longitude": {lon}, "altitude": 0.0 }}
}}"#,
                i = i,
                desc = takeout_description(&mut rng),
                ts = secs,
                fmt = Utc.timestamp_opt(secs, 0).single().unwrap().format("%Y-%m-%dT%H:%M:%SZ"),
                lat = takeout_lat(&mut rng),
                lon = takeout_lon(&mut rng),
            );
            fs::write(path.with_extension("json"), sidecar)?;
        }

        files.push(rel);
    }

    // Hand off to the normal importer so it walks the folder, hashes,
    // and populates the catalog + object store. Use `import_takeout`
    // so the Albums/<name>/ paths derive albums and the .json
    // sidecars become structured metadata.
    let label = format!("seed-{}", seed);
    let stats = if with_takeout {
        crate::import_takeout(conn, store, dest, progress)
            .context("import_takeout on seeded data")?
    } else {
        crate::import_folder(conn, store, dest, &label, progress)
            .context("import_folder on seeded data")?
    };

    Ok(SeedStats {
        generated: files.len(),
        imported: stats.imported,
        duplicates: stats.duplicates,
        failed: stats.failed,
    })
}

fn write_minimal_jpeg(path: &Path, size: usize, rng: &mut StdRng) -> Result<()> {
    let mut buf = Vec::with_capacity(size + 4);
    buf.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]); // SOI + APP0 marker start
    // Fill the body with random bytes (looks like a JPEG enough to
    // pass the magic-byte check). The importer only checks for the
    // SOI/EOI markers and the .jpg extension.
    let body = (0..size).map(|_| rng.gen::<u8>()).collect::<Vec<_>>();
    buf.extend(body);
    buf.extend_from_slice(&[0xFF, 0xD9]); // EOI

    let mut f = File::create(path)?;
    f.write_all(&buf)?;
    Ok(())
}

fn set_file_mtime(path: &Path, unix_secs: i64) -> Result<()> {
    let secs = u64::try_from(unix_secs).unwrap_or(0);
    let mtime = UNIX_EPOCH + std::time::Duration::from_secs(secs);
    File::open(path)?
        .set_modified(mtime)
        .with_context(|| format!("set mtime on {}", path.display()))?;
    Ok(())
}

fn takeout_description(rng: &mut StdRng) -> String {
    let pool = [
        "Sunset over the bay",
        "Hiking with friends",
        "Family dinner",
        "Coffee at the corner shop",
        "New puppy!",
        "First snow",
        "Office whiteboard",
        "Cherry blossoms",
        "Mountain summit",
        "Birthday cake",
        "Old neighborhood",
        "Latte art",
    ];
    pool[rng.gen_range(0..pool.len())].to_string()
}

// Five real cities so the geo search has something to match.
fn takeout_lat(rng: &mut StdRng) -> f64 {
    let cities = [37.7749, 35.6762, 40.7128, 51.5074, -33.8688]; // SF, Tokyo, NYC, London, Sydney
    cities[rng.gen_range(0..cities.len())]
        + rng.gen_range(-0.05..0.05)
}

fn takeout_lon(rng: &mut StdRng) -> f64 {
    let cities = [-122.4194, 139.6503, -74.0060, -0.1278, 151.2093];
    cities[rng.gen_range(0..cities.len())]
        + rng.gen_range(-0.05..0.05)
}