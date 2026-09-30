use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

/// A path inside the archive's object store. Always relative and rooted at
/// `objects/<aa>/<bb>/<full_hash>`. Never includes the archive root.
///
/// The full hash is preserved (not shortened) so that the same content always
/// maps to the same path, and so that the storage layer is trivially auditable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArchivePath(pub Utf8PathBuf);

impl ArchivePath {
    pub fn from_hash(hash: &str) -> Self {
        debug_assert_eq!(hash.len(), 64, "sha256 must be 64 hex chars");
        let (a, b) = hash.split_at(2);
        let mut p = Utf8PathBuf::from("objects");
        p.push(a);
        p.push(b);
        p.push(hash);
        Self(p)
    }

    pub fn as_path(&self) -> &Utf8PathBuf {
        &self.0
    }
}

/// A path relative to an arbitrary root (used for original-path preservation).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelativePath(pub Utf8PathBuf);

impl RelativePath {
    pub fn new(p: impl Into<Utf8PathBuf>) -> Self {
        Self(p.into())
    }
}
