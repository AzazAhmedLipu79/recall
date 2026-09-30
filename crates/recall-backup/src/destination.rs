//! Backup destination abstraction.
//!
//! A `BackupDestination` is something that can receive a stream of bytes
//! at a logical key (e.g. `objects/aa/bb/<sha256>`). The local
//! filesystem is one impl; an S3-compatible bucket is another.
//!
//! The trait is intentionally narrow: `same_content`, `put`, `get`. No
//! generic methods (those break dyn-compatibility). Streaming happens
//! in concrete impls where the type is known.

use recall_core::Result;

/// Logical key (relative path under the destination root).
pub type Key = String;

/// Abstraction over a backup target.
pub trait BackupDestination {
    /// Returns true if `key` already has an object with the same sha256.
    ///
    /// Implementations may compare by ETag for HTTP-backed destinations
    /// and by content-hash for local filesystems.
    fn same_content(&self, key: &str, expected_sha256: &str) -> Result<bool>;

    /// Atomically write `bytes` at `key`.
    ///
    /// "Atomic" means: a crash mid-write leaves either no object at
    /// `key` or the full new bytes. For local filesystems this is
    /// `temp + rename`. For HTTP-backed destinations this is
    /// `PUT` to a temporary key followed by a server-side move, or
    /// PUT with the right `If-None-Match` semantics.
    fn put(&self, key: &str, bytes: &[u8]) -> Result<()>;

    /// Read the bytes at `key`. Used to compute a sha256 when the
    /// destination doesn't expose ETags.
    fn get(&self, key: &str) -> Result<Vec<u8>>;
}
