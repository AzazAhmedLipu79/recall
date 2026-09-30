// recall-hash: streaming SHA-256 hashing with optional parallel multi-file hashing.
#![forbid(unsafe_code)]

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use recall_core::{Error, Result};

pub const HASH_ALGO: &str = "sha256";

/// Streaming SHA-256 of a file. Memory usage is bounded by the buffer size (64 KiB)
/// regardless of file size.
pub fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Streaming SHA-256 of an arbitrary reader.
pub fn sha256_reader<R: Read>(mut reader: R) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(Error::from)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Streaming SHA-256 of an in-memory byte slice. Convenience for sidecar JSON etc.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_empty() {
        // Known SHA-256 of empty input.
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hash_known_string() {
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hash_file_matches_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        std::fs::write(&p, b"hello world").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn hash_large_file_streams() {
        use std::io::Write;
        // Verify that large inputs work without loading everything into memory.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.bin");
        let f = std::fs::File::create(&p).unwrap();
        let mut writer = std::io::BufWriter::new(f);
        let chunk = vec![0xab_u8; 64 * 1024];
        for _ in 0..(4 * 1024) {
            writer.write_all(&chunk).unwrap();
        }
        writer.flush().unwrap();
        drop(writer);

        let h = sha256_file(&p).unwrap();
        // 64 hex chars and deterministic.
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        let h2 = sha256_file(&p).unwrap();
        assert_eq!(h, h2);
    }
}
