use serde::{Deserialize, Serialize};
use std::fmt;

/// Strongly-typed IDs. Each wraps a `String` and is serialized as a plain string.
/// This prevents accidentally passing an `AlbumId` where a `MediaId` is expected.
macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    };
}

id_type!(MediaId);
id_type!(ImportId);
id_type!(BackupId);
id_type!(VerificationId);
id_type!(AlbumId);
id_type!(DeviceId);
