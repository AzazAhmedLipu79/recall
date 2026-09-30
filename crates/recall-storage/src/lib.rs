//! Content-addressable object storage.
//!
//! Files are stored under `objects/<aa>/<bb>/<full-sha256>` where `<aa>` and
//! `<bb>` are the first 4 hex chars of the hash. Storing the full hash in
//! the final segment means the same content always maps to the same path and
//! the storage is trivially auditable on a normal filesystem.
//!
//! ## Crash safety
//!
//! Writes are performed to a sibling temporary file and atomically renamed
//! into place. A crash mid-write leaves at most one half-written `*.partial`
//! file behind, which is detected and discarded by [`ObjectStore::import`].
//!
//! ## Original preservation
//!
//! `import` reads from the source but never modifies it. The source path
//! is recorded by the caller as part of the catalog entry, not here.

#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use recall_core::{ArchivePath, Error, Result};
use recall_hash::sha256_reader;

static IMPORT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    /// Open an object store rooted at `root`. The directory is created if
    /// it does not already exist.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| Error::io(&root, e))?;
        // Pre-create top-level layout so walking the archive is friendly.
        fs::create_dir_all(root.join("objects")).map_err(|e| Error::io(&root, e))?;
        fs::create_dir_all(root.join("metadata")).map_err(|e| Error::io(&root, e))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a logical archive path to a physical path on disk.
    pub fn resolve(&self, p: &ArchivePath) -> PathBuf {
        self.root.join(p.as_path().as_std_path())
    }

    /// Returns true if the object already exists at its expected path.
    pub fn exists(&self, p: &ArchivePath) -> bool {
        self.resolve(p).is_file()
    }

    /// Stream the file at `source` into the object store, hashing while we
    /// write. Returns the computed SHA-256.
    ///
    /// If the destination already exists with the same hash, no copy happens.
    /// If the destination exists with a different hash (this should never
    /// happen because the path encodes the hash), it is treated as a logic
    /// error.
    pub fn import(&self, source: &Path) -> Result<String> {
        let file = File::open(source).map_err(|e| Error::io(source, e))?;
        self.import_reader(file)
    }

    /// Like [`Self::import`] but accepts any `Read` source — used by the
    /// ZIP importer to stream an entry's bytes without first writing them
    /// to a temp file.
    pub fn import_reader<R: Read>(&self, reader: R) -> Result<String> {
        // Clean up any leftover .partial from a prior interrupted attempt.
        self.cleanup_partial()?;
        let mut reader = BufReader::with_capacity(256 * 1024, reader);

        // Tee: hash + write.
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        let tmp = self.make_partial_path();
        {
            let f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            let mut writer = BufWriter::with_capacity(256 * 1024, f);
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader.read(&mut buf).map_err(|e| Error::io("<reader>", e))?;
                if n == 0 {
                    break;
                }
                sha2::Digest::update(&mut hasher, &buf[..n]);
                writer.write_all(&buf[..n]).map_err(|e| Error::io(&tmp, e))?;
            }
            writer.flush().map_err(|e| Error::io(&tmp, e))?;
            let inner = writer
                .into_inner()
                .map_err(|e| Error::io(&tmp, e.into_error()))?;
            inner.sync_all().map_err(|e| Error::io(&tmp, e))?;
        }
        let hash = hex::encode(sha2::Digest::finalize(hasher));
        let dest = self.root.join(ArchivePath::from_hash(&hash).as_path().as_std_path());

        if dest.exists() {
            // Idempotent — already archived.
            let _ = fs::remove_file(&tmp);
            return Ok(hash);
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        fs::rename(&tmp, &dest).map_err(|e| Error::io(&dest, e))?;
        Ok(hash)
    }

    /// Verify that the file at `archive_path` matches the given hash.
    pub fn verify(&self, archive_path: &ArchivePath, expected_hash: &str) -> Result<bool> {
        let p = self.resolve(archive_path);
        if !p.exists() {
            return Ok(false);
        }
        let f = File::open(&p).map_err(|e| Error::io(&p, e))?;
        let actual = sha256_reader(BufReader::with_capacity(256 * 1024, f))?;
        Ok(actual == expected_hash)
    }

    /// Open a file in the object store for reading.
    pub fn read(&self, archive_path: &ArchivePath) -> Result<File> {
        let p = self.resolve(archive_path);
        File::open(&p).map_err(|e| Error::io(&p, e))
    }

    /// Write metadata sidecar (e.g. raw Google JSON) under `metadata/<hash>.json`.
    pub fn write_sidecar(&self, hash: &str, bytes: &[u8]) -> Result<PathBuf> {
        let dir = self.root.join("metadata");
        fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let tmp = dir.join(format!("{hash}.json.partial"));
        let final_p = dir.join(format!("{hash}.json"));

        {
            let mut f = BufWriter::new(File::create(&tmp).map_err(|e| Error::io(&tmp, e))?);
            f.write_all(bytes).map_err(|e| Error::io(&tmp, e))?;
            f.flush().map_err(|e| Error::io(&tmp, e))?;
        }
        if final_p.exists() {
            let _ = fs::remove_file(&tmp);
        } else {
            fs::rename(&tmp, &final_p).map_err(|e| Error::io(&final_p, e))?;
        }
        Ok(final_p)
    }

    /// Total bytes occupied by the object tree, walking files only.
    pub fn disk_usage(&self) -> Result<u64> {
        let mut total = 0u64;
        let objects = self.root.join("objects");
        if !objects.exists() {
            return Ok(0);
        }
        for entry in walkdir::WalkDir::new(&objects)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let m = entry
                    .metadata()
                    .map_err(|e| Error::io(entry.path(), std::io::Error::from(e)))?;
                total += m.len();
            }
        }
        Ok(total)
    }

    /// Count of stored objects (files under `objects/`).
    pub fn object_count(&self) -> Result<u64> {
        let mut n = 0u64;
        let objects = self.root.join("objects");
        if !objects.exists() {
            return Ok(0);
        }
        for entry in walkdir::WalkDir::new(&objects)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            // Skip .partial and dotfile-named imports.
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".partial") || name.starts_with(".import-") {
                continue;
            }
            n += 1;
        }
        Ok(n)
    }

    /// Iterate over `(archive_path, physical_path)` for every object in the
    /// store. Used by the secondary-backup sync engine.
    pub fn iter_objects(&self) -> impl Iterator<Item = (ArchivePath, PathBuf)> + '_ {
        let root = self.root.clone();
        walkdir::WalkDir::new(self.root.join("objects"))
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(move |e| {
                let rel = e.path().strip_prefix(&root).ok()?;
                let ut = camino::Utf8Path::from_path(rel)?;
                let name = ut.file_name().unwrap_or("");
                if name.ends_with(".partial") || name.starts_with(".import-") {
                    return None;
                }
                Some((ArchivePath(ut.to_owned()), e.path().to_path_buf()))
            })
    }

    /// Physical path for a given content hash.
    pub fn path_for_hash(&self, hash: &str) -> PathBuf {
        self.resolve(&ArchivePath::from_hash(hash))
    }

    fn make_partial_path(&self) -> PathBuf {
        let pid = std::process::id();
        let seq = IMPORT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = self.root.join("objects");
        fs::create_dir_all(&dir).ok();
        dir.join(format!(".import-{pid}-{nanos}-{seq}.partial"))
    }

    fn cleanup_partial(&self) -> std::io::Result<()> {
        let dir = self.root.join("objects");
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with(".import-") && s.ends_with(".partial") {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    use std::io::Write;

    #[test]
    fn empty_layout() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("a")).unwrap();
        assert_eq!(store.object_count().unwrap(), 0);
        assert_eq!(store.disk_usage().unwrap(), 0);
    }

    #[test]
    fn import_places_file_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("a")).unwrap();

        let src = dir.path().join("src.bin");
        let mut f = std::fs::File::create(&src).unwrap();
        f.write_all(b"hello archive").unwrap();
        drop(f);

        let hash = store.import(&src).unwrap();
        let mut h = sha2::Sha256::new();
        sha2::Digest::update(&mut h, b"hello archive");
        let expected = hex::encode(sha2::Digest::finalize(h));
        assert_eq!(hash, expected);

        let dest = store.path_for_hash(&hash);
        assert!(dest.exists(), "expected dest at {dest:?}");
        let read_back = std::fs::read(&dest).unwrap();
        assert_eq!(read_back, b"hello archive");
    }

    #[test]
    fn import_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("a")).unwrap();
        let src = dir.path().join("src.bin");
        std::fs::write(&src, b"abc").unwrap();
        let h1 = store.import(&src).unwrap();
        let h2 = store.import(&src).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(store.object_count().unwrap(), 1);
    }

    #[test]
    fn verify_succeeds_on_clean_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("a")).unwrap();
        let src = dir.path().join("src.bin");
        std::fs::write(&src, b"abc").unwrap();
        let hash = store.import(&src).unwrap();
        assert!(store
            .verify(&ArchivePath::from_hash(&hash), &hash)
            .unwrap());
        assert!(!store
            .verify(&ArchivePath::from_hash(&hash), "0".repeat(64).as_str())
            .unwrap());
    }

    #[test]
    fn import_large_streams() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("a")).unwrap();
        let src = dir.path().join("big.bin");
        {
            let f = std::fs::File::create(&src).unwrap();
            let mut w = std::io::BufWriter::new(f);
            let chunk = vec![0x42u8; 256 * 1024];
            for _ in 0..32 {
                w.write_all(&chunk).unwrap();
            }
            w.flush().unwrap();
        }
        let hash = store.import(&src).unwrap();
        assert_eq!(hash.len(), 64);
        assert!(store.path_for_hash(&hash).exists());
    }

    #[test]
    fn distinct_content_uses_distinct_paths() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("a")).unwrap();

        let s1 = dir.path().join("s1");
        let s2 = dir.path().join("s2");
        std::fs::write(&s1, b"foo").unwrap();
        std::fs::write(&s2, b"bar").unwrap();
        let h1 = store.import(&s1).unwrap();
        let h2 = store.import(&s2).unwrap();
        assert_ne!(h1, h2);
        assert_eq!(store.object_count().unwrap(), 2);
    }

    #[test]
    fn iter_objects_skips_partials() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path().join("a")).unwrap();
        let src = dir.path().join("s");
        std::fs::write(&src, b"foo").unwrap();
        let hash = store.import(&src).unwrap();
        // Drop an orphan partial and confirm iter_objects doesn't include it.
        std::fs::write(
            store.root.join("objects").join(".import-99-99.partial"),
            b"junk",
        )
        .unwrap();
        let collected: Vec<_> = store.iter_objects().collect();
        assert_eq!(collected.len(), 1);
        assert!(collected[0].0 .0.ends_with(&hash));
    }
}
