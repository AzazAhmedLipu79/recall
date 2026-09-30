use serde::{Deserialize, Serialize};

/// Classification of archived media. Stored as TEXT in SQLite.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Photo,
    Video,
    Document,
    Audio,
    #[default]
    Other,
}

impl MediaKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaKind::Photo => "photo",
            MediaKind::Video => "video",
            MediaKind::Document => "document",
            MediaKind::Audio => "audio",
            MediaKind::Other => "other",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "photo" => Some(MediaKind::Photo),
            "video" => Some(MediaKind::Video),
            "document" => Some(MediaKind::Document),
            "audio" => Some(MediaKind::Audio),
            "other" => Some(MediaKind::Other),
            _ => None,
        }
    }

    /// Best-effort classification by extension. The authoritative classification
    /// comes from MIME detection at import time, but this is used as a fallback
    /// and to categorize documents.
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" | "png" | "heic" | "heif" | "webp" | "tiff" | "tif"
            | "gif" | "bmp" | "raw" | "cr2" | "nef" | "arw" | "dng" | "orf"
            | "raf" | "rw2" | "pef" => MediaKind::Photo,
            "mp4" | "mov" | "m4v" | "avi" | "mkv" | "webm" | "wmv" | "flv"
            | "3gp" => MediaKind::Video,
            "mp3" | "m4a" | "wav" | "flac" | "aac" | "ogg" | "opus" => MediaKind::Audio,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt"
            | "rtf" | "odt" | "ods" | "odp" | "epub" | "mobi" | "csv" | "md"
            | "html" | "htm" | "json" | "xml" => MediaKind::Document,
            _ => MediaKind::Other,
        }
    }
}
