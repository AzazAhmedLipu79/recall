//! Progress reporting for long-running operations.
//!
//! Engine functions accept an `Option<&dyn Progress>` parameter. Callers
//! that don't care pass `None`; callers that want live updates pass a
//! concrete implementation.
//!
//! All callbacks are synchronous. The engine never blocks on user code:
//! implementations should be cheap (a `println!`, a channel send, etc.).
//!
//! ## Example
//!
//! ```rust,ignore
//! use recall_core::{NoopProgress, Progress};
//!
//! let p = NoopProgress;
//! p.on_event(&ProgressEvent::Discovered { total: 10 });
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One discrete event during a long-running import / verify / backup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    /// A new run has started. The total count is a hint if known (None
    /// for streaming sources like Takeout ZIPs where we don't know
    /// until we've enumerated every entry).
    Started {
        kind: RunKind,
        total: Option<u64>,
    },
    /// A run has finished.
    Finished {
        kind: RunKind,
        ok: u64,
        failed: u64,
        skipped: u64,
    },
    /// The engine discovered this many items so far (e.g. walked a
    /// folder, indexed a zip).
    Discovered { total: u64 },
    /// Per-file outcome from an import.
    FileImported {
        path: PathBuf,
        media_id: String,
    },
    /// The file was already archived; we linked the new source path.
    FileDuplicate { path: PathBuf, media_id: String },
    /// The file failed (corrupt, unreadable, etc.).
    FileFailed {
        path: PathBuf,
        reason: String,
    },
    /// Per-file outcome from a verify pass.
    VerifyOk { path: PathBuf },
    VerifyMissing { path: PathBuf },
    VerifyMismatched { path: PathBuf, expected: String, actual: String },
    /// Per-file outcome from a backup.
    BackupCopied { path: PathBuf, bytes: u64 },
    BackupSkipped { path: PathBuf },
    BackupFailed { path: PathBuf, reason: String },
    /// Per-file outcome from a ZIP export.
    ExportCopied { path: PathBuf, bytes: u64 },
    ExportFailed { path: PathBuf, reason: String },
    /// Migration-specific events for the "Move archive…" flow.
    MigrationStarted { total_files_hint: Option<u64> },
    MigrationFileCopied { path: PathBuf, bytes: u64 },
    MigrationFinished {
        files_copied: u64,
        bytes_copied: u64,
        deleted_source: bool,
    },
    MigrationFailed { reason: String },
}

/// What kind of long-running operation is producing events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    ImportFolder,
    ImportTakeout,
    ImportTakeoutZip,
    Verify,
    Backup,
    ExportZip,
}

impl RunKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ImportFolder => "import-folder",
            Self::ImportTakeout => "import-takeout",
            Self::ImportTakeoutZip => "import-takeout-zip",
            Self::Verify => "verify",
            Self::Backup => "backup",
            Self::ExportZip => "export-zip",
        }
    }
}

/// Receiver of `ProgressEvent`s. Implementations should be cheap;
/// the engine calls them on every per-file step.
pub trait Progress {
    fn on_event(&self, event: &ProgressEvent);
}

/// Default implementation that drops everything on the floor.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopProgress;

impl Progress for NoopProgress {
    fn on_event(&self, _event: &ProgressEvent) {}
}
