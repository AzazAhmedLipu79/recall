//! Local filesystem [`BackupDestination`].
//!
//! Atomic via temp + rename. `same_content` re-hashes the file at
//! `key` and compares to the expected sha256 — cheap because we only
//! hash when the file already exists.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use recall_core::{Error, Result};
use recall_hash::sha256_reader;

use super::destination::BackupDestination;

#[derive(Debug, Clone)]
pub struct LocalDestination {
    root: PathBuf,
}

impl LocalDestination {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn partial_path(&self, key: &str) -> PathBuf {
        let resolved = self.root.join(key);
        let parent = resolved.parent().unwrap_or(Path::new("."));
        let fname = resolved
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "data".into());
        parent.join(format!(".{fname}.partial"))
    }

    /// Stream `reader` into `key` while hashing. Returns the resulting
    /// sha256. Not part of the trait so we can keep the trait
    /// dyn-compatible.
    pub fn put_reader_hashed<R: Read>(&self, key: &str, mut reader: R) -> Result<String> {
        let final_p = self.root.join(key);
        if let Some(parent) = final_p.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let tmp = self.partial_path(key);
        let mut hasher = Sha256::new();
        {
            let mut f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader
                    .read(&mut buf)
                    .map_err(|e| Error::io("<reader>", e))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                f.write_all(&buf[..n]).map_err(|e| Error::io(&tmp, e))?;
            }
            f.sync_all().map_err(|e| Error::io(&tmp, e))?;
        }
        let hash = hex::encode(hasher.finalize());
        fs::rename(&tmp, &final_p).map_err(|e| Error::io(&final_p, e))?;
        Ok(hash)
    }
}

impl BackupDestination for LocalDestination {
    fn same_content(&self, key: &str, expected_sha256: &str) -> Result<bool> {
        let p = self.root.join(key);
        if !p.exists() {
            return Ok(false);
        }
        let f = File::open(&p).map_err(|e| Error::io(&p, e))?;
        let actual = sha256_reader(f)?;
        Ok(actual == expected_sha256)
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let final_p = self.root.join(key);
        if let Some(parent) = final_p.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let tmp = self.partial_path(key);
        {
            let mut f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            f.write_all(bytes).map_err(|e| Error::io(&tmp, e))?;
            f.sync_all().map_err(|e| Error::io(&tmp, e))?;
        }
        fs::rename(&tmp, &final_p).map_err(|e| Error::io(&final_p, e))?;
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Vec<u8>> {
        let p = self.root.join(key);
        let bytes = fs::read(&p).map_err(|e| Error::io(&p, e))?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn put_then_same_content() {
        let dir = tempfile::tempdir().unwrap();
        let dst = LocalDestination::new(dir.path().to_path_buf());
        dst.put("a/b/c.bin", b"hello").unwrap();
        assert!(dst.same_content("a/b/c.bin", &sha256_hex(b"hello")).unwrap());
    }

    #[test]
    fn same_content_returns_false_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let dst = LocalDestination::new(dir.path().to_path_buf());
        assert!(!dst.same_content("missing", "x").unwrap());
    }

    #[test]
    fn put_reader_hashes_during_write() {
        let dir = tempfile::tempdir().unwrap();
        let dst = LocalDestination::new(dir.path().to_path_buf());
        let hash = dst
            .put_reader_hashed("data.bin", Cursor::new(b"streaming"))
            .unwrap();
        assert_eq!(hash, sha256_hex(b"streaming"));
        assert!(dst.same_content("data.bin", &hash).unwrap());
    }

    #[test]
    fn get_returns_what_was_put() {
        let dir = tempfile::tempdir().unwrap();
        let dst = LocalDestination::new(dir.path().to_path_buf());
        dst.put("x", b"abc").unwrap();
        assert_eq!(dst.get("x").unwrap(), b"abc");
    }

    fn sha256_hex(b: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(b);
        hex::encode(h.finalize())
    }
}
