// recall-core: shared types and error definitions used across all crates.
#![forbid(unsafe_code)]

pub mod error;
pub mod ids;
pub mod media_kind;
pub mod paths;
pub mod progress;

pub use error::{Error, Result};
pub use ids::{MediaId, ImportId, BackupId, VerificationId, AlbumId, DeviceId};
pub use media_kind::MediaKind;
pub use paths::{ArchivePath, RelativePath};
pub use progress::{NoopProgress, Progress, ProgressEvent, RunKind};
