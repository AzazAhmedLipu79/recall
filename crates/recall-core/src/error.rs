use std::path::PathBuf;
use thiserror::Error;

/// Crate-wide error type. All fallible operations in `recall-*` crates return `Result<T>`.
#[derive(Debug, Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("hash mismatch: expected {expected}, got {actual} for {path}")]
    HashMismatch {
        expected: String,
        actual: String,
        path: PathBuf,
    },

    #[error("file already archived: sha256={0}")]
    DuplicateContent(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("operation cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(format!("io: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
