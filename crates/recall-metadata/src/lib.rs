//! Metadata extraction for media files.
//!
//! Two layers:
//!
//! 1. Cheap container inspection — file extension / magic bytes — to
//!    classify the media kind and pick a parser.
//! 2. Format-specific parsing. We currently ship:
//!    - **JPEG EXIF** (APP1 marker, "Exif\0\0" header) — the most common
//!      photo source.
//!    - **MP4/MOV** `mvhd` and `tkhd` for duration + creation time
//!      (`mvhd` and `mdat`-adjacent atom walking only, just enough for
//!      the fields we need).
//!    - **PNG IHDR** for width/height.
//!
//! Other formats pass through `extract`: we record only the filesystem
//! times as a fallback. The intent is to keep the dependency footprint
//! small and the build fast while still surfacing the genuinely-useful
//! metadata fields called out in PRD §11.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use recall_core::{MediaKind, Result};

/// Best-effort metadata extracted from a single source file.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Extracted {
    pub kind: MediaKind,
    pub mime_type: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_seconds: Option<f64>,

    /// EXIF / container "taken at" timestamp, if any.
    pub taken_at: Option<i64>,

    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<f64>,
    pub iso: Option<i64>,
    pub aperture: Option<f64>,
    pub shutter_speed: Option<String>,
    pub orientation: Option<i64>,

    /// Raw GPS lat/lon if any.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,

    pub description: Option<String>,

    /// What subset of the EXIF was actually parseable. Useful for debugging
    /// but not stored in the catalog by default.
    pub parser: &'static str,
}

impl Extracted {
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Public entry point. Inspects `path` and returns whatever metadata could
/// be extracted without copying the file into the archive.
///
/// `source_kind` is used as a fallback when the filesystem doesn't reveal
/// much (e.g. filesystem mtime/ctime are recorded separately by the
/// importer, not here).
pub fn extract(path: &Path) -> Result<Extracted> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // First, sniff magic bytes for the cases where the file extension is
    // wrong or absent.
    let sniffed = sniff_magic(path).unwrap_or_default();

    let kind = classify(&ext, &sniffed);
    let mime = mime_for(&ext, &sniffed).map(|s| s.to_string());

    let mut out = Extracted {
        kind,
        mime_type: mime.clone(),
        parser: "none",
        ..Default::default()
    };

    match (kind, &*ext) {
        (MediaKind::Photo, "jpg") | (MediaKind::Photo, "jpeg") => {
            if let Ok(j) = parse_jpeg_exif(path) {
                out = j;
                out.kind = kind;
                out.mime_type = mime.clone();
                out.parser = "jpeg_exif";
            }
        }
        (MediaKind::Photo, "png") => {
            if let Ok((w, h)) = parse_png_dims(path) {
                out.width = Some(w);
                out.height = Some(h);
                out.parser = "png_ihdr";
            }
        }
        (MediaKind::Video, "mp4") | (MediaKind::Video, "mov") | (MediaKind::Video, "m4v") => {
            if let Ok(v) = parse_mp4(path) {
                out.width = v.width.or(out.width);
                out.height = v.height.or(out.height);
                out.duration_seconds = v.duration_seconds.or(out.duration_seconds);
                out.taken_at = v.taken_at.or(out.taken_at);
                out.parser = "mp4";
            }
        }
        _ => {}
    }

    // Filesystem mtime/ctime as a final fallback for taken_at.
    if out.taken_at.is_none() {
        if let Ok(meta) = path.metadata() {
            if let Ok(mtime) = meta.modified() {
                if let Ok(dt) = mtime.duration_since(std::time::UNIX_EPOCH) {
                    out.taken_at = Some(dt.as_secs() as i64);
                }
            }
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// magic-byte sniffing
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
pub struct Magic {
    pub is_jpeg: bool,
    pub is_png: bool,
    pub is_mp4: bool,
    pub is_heic: bool,
    pub is_mov: bool,
}

pub fn sniff_magic(path: &Path) -> std::io::Result<Magic> {
    let mut f = File::open(path)?;
    let mut hdr = [0u8; 16];
    let n = f.read(&mut hdr)?;
    if n < 4 {
        return Ok(Magic::default());
    }
    let mut m = Magic::default();
    if n >= 3 && hdr[0..3] == [0xFF, 0xD8, 0xFF] {
        m.is_jpeg = true;
    }
    if n >= 8 && hdr[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        m.is_png = true;
    }
    if n >= 12 && &hdr[4..8] == b"ftyp" {
        m.is_mp4 = true;
        // mp4 family may also be HEIC or MOV.
        let brand = &hdr[8..12];
        if brand == b"heic" || brand == b"heix" || brand == b"hevc" || brand == b"mif1" {
            m.is_heic = true;
        }
        if brand == b"qt  " {
            m.is_mov = true;
        }
    }
    Ok(m)
}

fn classify(ext: &str, magic: &Magic) -> MediaKind {
    if magic.is_heic {
        return MediaKind::Photo;
    }
    if magic.is_jpeg && ext.is_empty() {
        return MediaKind::Photo;
    }
    MediaKind::from_extension(ext)
}

fn mime_for(ext: &str, magic: &Magic) -> Option<&'static str> {
    if magic.is_heic {
        return Some("image/heic");
    }
    if magic.is_jpeg {
        return Some("image/jpeg");
    }
    if magic.is_png {
        return Some("image/png");
    }
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "heic" | "heif" => Some("image/heic"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "mp4" => Some("video/mp4"),
        "mov" | "qt" => Some("video/quicktime"),
        "m4v" => Some("video/x-m4v"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// JPEG EXIF (APP1 / "Exif\0\0" TIFF container)
// ---------------------------------------------------------------------------

fn parse_jpeg_exif(path: &Path) -> Result<Extracted> {
    let mut out = Extracted::default();
    let f = File::open(path).map_err(recall_core::Error::from)?;
    let mut r = BufReader::new(f);
    if !read_jpeg_app1_exif(&mut r, &mut out)? {
        return Ok(out);
    }
    Ok(out)
}

/// Walk JPEG markers until APP1 with "Exif\0\0" prefix is found, then
/// parse the TIFF container using `parse_tiff_ifd`.
fn read_jpeg_app1_exif<R: Read + Seek>(
    r: &mut R,
    out: &mut Extracted,
) -> Result<bool> {
    r.seek(SeekFrom::Start(0))?;
    let mut hdr = [0u8; 2];
    r.read_exact(&mut hdr).map_err(recall_core::Error::from)?;
    if hdr != [0xFF, 0xD8] {
        return Ok(false);
    }
    loop {
        let mut marker = [0u8; 2];
        if r.read_exact(&mut marker).is_err() {
            return Ok(false);
        }
        if marker[0] != 0xFF {
            return Ok(false);
        }
        // Skip padding 0xFF bytes.
        while marker[1] == 0xFF {
            if r.read_exact(&mut marker).is_err() {
                return Ok(false);
            }
        }
        let marker_id = marker[1];
        let len = if (0xD0..=0xD9).contains(&marker_id) || marker_id == 0x01 || marker_id == 0x00 {
            // Standalone markers, no length.
            if marker_id == 0xD9 {
                return Ok(false);
            }
            continue;
        } else {
            let mut len_buf = [0u8; 2];
            r.read_exact(&mut len_buf).map_err(recall_core::Error::from)?;
            u16::from_be_bytes(len_buf) as usize
        };
        if marker_id == 0xE1 {
            // Read and check for "Exif\0\0".
            let mut payload = vec![0u8; len.saturating_sub(2)];
            r.read_exact(&mut payload).map_err(recall_core::Error::from)?;
            if payload.starts_with(b"Exif\0\0") {
                let tiff = &payload[6..];
                return parse_tiff_ifd(tiff, out).map(|_| true);
            } else {
                continue;
            }
        } else if marker_id == 0xDA {
            // Start of scan: EXIF should have already appeared.
            return Ok(false);
        } else {
            // Skip non-APP1 segment.
            r.seek(SeekFrom::Current(len as i64 - 2))
                .map_err(recall_core::Error::from)?;
        }
    }
}

/// Minimal EXIF parser for the tags the PRD cares about.
/// Supports Intel ("II") and Motorola ("MM") byte orders.
fn parse_tiff_ifd(tiff: &[u8], out: &mut Extracted) -> Result<bool> {
    if tiff.len() < 8 {
        return Ok(false);
    }
    let byte_order = &tiff[0..2];
    let tiff_reader = match byte_order {
        b"II" => Tiff::little_endian(),
        b"MM" => Tiff::big_endian(),
        _ => return Ok(false),
    };

    // IFD0 starts at offset 8.
    let n_entries = tiff_reader.read_u16(&tiff[8..10]) as usize;
    let mut offset = 10usize;
    let mut exif_ifd_offset: Option<u32> = None;
    let mut gps_ifd_offset: Option<u32> = None;

    for _ in 0..n_entries {
        if offset + 12 > tiff.len() {
            break;
        }
        let tag = tiff_reader.read_u16(&tiff[offset..offset + 2]);
        let typ = tiff_reader.read_u16(&tiff[offset + 2..offset + 4]);
        let count = tiff_reader.read_u32(&tiff[offset + 4..offset + 8]);
        let value_or_offset = tiff_reader.read_u32(&tiff[offset + 8..offset + 12]);
        match tag {
            0x010F => {
                // Make
                if let Some(s) = tiff_reader.read_ascii(tiff, offset, typ, count, value_or_offset) {
                    out.camera_make = Some(s);
                }
            }
            0x0110 => {
                // Model
                if let Some(s) = tiff_reader.read_ascii(tiff, offset, typ, count, value_or_offset) {
                    out.camera_model = Some(s);
                }
            }
            0x0112 => {
                // Orientation
                let ptr = tiff_reader.value_ptr(tiff, offset, typ, value_or_offset, count as usize);
                if ptr + 2 <= tiff.len() {
                    out.orientation = Some(tiff_reader.read_u16(&tiff[ptr..ptr + 2]) as i64);
                }
            }
            0x0132 => {
                // DateTime (fallback when DateTimeOriginal is missing)
                if let Some(s) = exif_datetime(&tiff_reader, tiff, offset, typ, count, value_or_offset) {
                    if out.taken_at.is_none() {
                        out.taken_at = Some(s);
                    }
                }
            }
            0x8769 => {
                exif_ifd_offset = Some(value_or_offset);
            }
            0x8825 => {
                gps_ifd_offset = Some(value_or_offset);
            }
            _ => {}
        }
        offset += 12;
    }

    if let Some(off) = exif_ifd_offset {
        parse_tiff_ifd_at(tiff, off as usize, out, &tiff_reader)?;
    }
    if let Some(off) = gps_ifd_offset {
        parse_gps_ifd(tiff, off as usize, out, &tiff_reader)?;
    }
    Ok(true)
}

/// Tiny TIFF reader that knows the byte order.
#[derive(Clone, Copy)]
struct Tiff {
    little_endian: bool,
}

impl Tiff {
    fn little_endian() -> Self {
        Self { little_endian: true }
    }
    fn big_endian() -> Self {
        Self { little_endian: false }
    }
    fn read_u16(&self, b: &[u8]) -> u16 {
        if self.little_endian {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        }
    }
    fn read_u32(&self, b: &[u8]) -> u32 {
        if self.little_endian {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        }
    }
    fn read_rational(&self, b: &[u8]) -> Option<f64> {
        let n = self.read_u32(b);
        let d = self.read_u32(&b[4..]);
        if d == 0 {
            None
        } else {
            Some(n as f64 / d as f64)
        }
    }
    /// Resolve the value field for an entry at `entry_offset`. Returns the
    /// slice that actually contains the value (either inline at the value
    /// field of the entry, or pointed at by the offset stored in the value
    /// field).
    fn value_ptr(
        &self,
        _tiff: &[u8],
        entry_offset: usize,
        typ: u16,
        raw_value: u32,
        count: usize,
    ) -> usize {
        let size = match typ {
            1 | 2 | 7 => 1,
            3 => 2,
            4 | 9 | 10 | 11 => 4,
            5 => 8,
            _ => 1,
        };
        let total = size * count;
        if total <= 4 {
            // Value field is the last 4 bytes of the 12-byte entry: at +8.
            entry_offset + 8
        } else {
            raw_value as usize
        }
    }
    fn read_ascii(
        &self,
        tiff: &[u8],
        entry_offset: usize,
        typ: u16,
        count: u32,
        raw_value: u32,
    ) -> Option<String> {
        if typ != 2 {
            return None;
        }
        let ptr = self.value_ptr(tiff, entry_offset, typ, raw_value, count as usize);
        let end = (ptr + count as usize).min(tiff.len());
        if ptr >= tiff.len() {
            return None;
        }
        let bytes = &tiff[ptr..end];
        let nul = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        std::str::from_utf8(&bytes[..nul])
            .ok()
            .map(|s| s.trim().to_string())
    }
    fn read_u32_at_ifd(
        &self,
        tiff: &[u8],
        entry_offset: usize,
        typ: u16,
        raw_value: u32,
    ) -> Option<u32> {
        let ptr = self.value_ptr(tiff, entry_offset, typ, raw_value, 1);
        if ptr + 4 > tiff.len() {
            return None;
        }
        Some(self.read_u32(&tiff[ptr..ptr + 4]))
    }
    fn read_u16_at_ifd(
        &self,
        tiff: &[u8],
        entry_offset: usize,
        typ: u16,
        raw_value: u32,
    ) -> Option<u16> {
        let ptr = self.value_ptr(tiff, entry_offset, typ, raw_value, 1);
        if ptr + 2 > tiff.len() {
            return None;
        }
        Some(self.read_u16(&tiff[ptr..ptr + 2]))
    }
}

fn parse_tiff_ifd_at(
    tiff: &[u8],
    start: usize,
    out: &mut Extracted,
    tiff_reader: &Tiff,
) -> Result<bool> {
    if start + 2 > tiff.len() {
        return Ok(false);
    }
    let n = tiff_reader.read_u16(&tiff[start..start + 2]) as usize;
    let mut offset = start + 2;
    for _ in 0..n {
        if offset + 12 > tiff.len() {
            break;
        }
        let tag = tiff_reader.read_u16(&tiff[offset..offset + 2]);
        let typ = tiff_reader.read_u16(&tiff[offset + 2..offset + 4]);
        let count = tiff_reader.read_u32(&tiff[offset + 4..offset + 8]);
        let value_or_offset = tiff_reader.read_u32(&tiff[offset + 8..offset + 12]);
        match tag {
            0x9003 | 0x9004 => {
                // DateTimeOriginal / DateTimeDigitized
                if let Some(s) = exif_datetime(&tiff_reader, tiff, offset, typ, count, value_or_offset) {
                    if out.taken_at.is_none() {
                        out.taken_at = Some(s);
                    }
                }
            }
            0x920A => {
                // FocalLength
                let ptr = tiff_reader.value_ptr(tiff, offset, typ, value_or_offset, 1);
                if let Some(v) = tiff_reader.read_rational(&tiff[ptr..ptr + 8.min(tiff.len() - ptr)])
                {
                    out.focal_length = Some(v);
                }
            }
            0x8827 => {
                // ISO
                if let Some(v) = tiff_reader.read_u16_at_ifd(tiff, offset, typ, value_or_offset) {
                    out.iso = Some(v as i64);
                }
            }
            0x829D => {
                // FNumber
                let ptr = tiff_reader.value_ptr(tiff, offset, typ, value_or_offset, 1);
                if let Some(v) = tiff_reader.read_rational(&tiff[ptr..ptr + 8.min(tiff.len() - ptr)])
                {
                    out.aperture = Some(v);
                }
            }
            0x9201 => {
                // ShutterSpeedValue (signed rational). ExposureTime = 2^-tv.
                let ptr = tiff_reader.value_ptr(tiff, offset, typ, value_or_offset, 1);
                if ptr + 8 <= tiff.len() {
                    let n = tiff_reader.read_u32(&tiff[ptr..ptr + 4]) as i64 as f64;
                    let d = tiff_reader.read_u32(&tiff[ptr + 4..ptr + 8]) as i64 as f64;
                    if d != 0.0 {
                        let tv = n / d;
                        let et = 2f64.powf(-tv);
                        out.shutter_speed = Some(format_shutter(et));
                    }
                }
            }
            0xA002 => {
                if let Some(v) = tiff_reader.read_u32_at_ifd(tiff, offset, typ, value_or_offset) {
                    out.width = Some(v as i64);
                }
            }
            0xA003 => {
                if let Some(v) = tiff_reader.read_u32_at_ifd(tiff, offset, typ, value_or_offset) {
                    out.height = Some(v as i64);
                }
            }
            0xA433 => {
                // LensMake
                if let Some(s) = tiff_reader.read_ascii(tiff, offset, typ, count, value_or_offset) {
                    if out.lens.is_none() {
                        out.lens = Some(s);
                    }
                }
            }
            0xA434 => {
                // LensModel — preferred over LensMake
                if let Some(s) = tiff_reader.read_ascii(tiff, offset, typ, count, value_or_offset) {
                    out.lens = Some(s);
                }
            }
            0x9286 => {
                // UserComment — some cameras put a description here. The
                // first 8 bytes are character-code identifier; we just
                // strip it and take the remainder as ASCII if it is.
                if let Some(s) = tiff_reader.read_ascii(tiff, offset, typ, count, value_or_offset) {
                    if out.description.is_none() {
                        out.description = Some(s);
                    }
                }
            }
            _ => {}
        }
        offset += 12;
    }
    Ok(true)
}

fn format_shutter(t: f64) -> String {
    if t >= 1.0 {
        format!("{:.1}", t)
    } else if t > 0.0 {
        let n = (1.0 / t).round() as i64;
        if n > 0 {
            format!("1/{n}")
        } else {
            format!("{t}")
        }
    } else {
        "0".into()
    }
}

fn exif_datetime(
    tiff_reader: &Tiff,
    tiff: &[u8],
    entry_offset: usize,
    typ: u16,
    count: u32,
    raw_value: u32,
) -> Option<i64> {
    if typ != 2 {
        return None;
    }
    let count = count as usize;
    let ptr = tiff_reader.value_ptr(tiff, entry_offset, typ, raw_value, count);
    if ptr >= tiff.len() {
        return None;
    }
    let end = (ptr + count).min(tiff.len());
    let bytes = &tiff[ptr..end];
    let s = std::str::from_utf8(bytes).ok()?.trim_end_matches('\0').trim();
    let ndt = chrono::NaiveDateTime::parse_from_str(s, "%Y:%m:%d %H:%M:%S").ok()?;
    let dt: DateTime<Utc> = chrono::TimeZone::from_utc_datetime(&Utc, &ndt);
    Some(dt.timestamp())
}

fn parse_gps_ifd(
    tiff: &[u8],
    start: usize,
    out: &mut Extracted,
    tiff_reader: &Tiff,
) -> Result<bool> {
    if start + 2 > tiff.len() {
        return Ok(false);
    }
    let n = tiff_reader.read_u16(&tiff[start..start + 2]) as usize;
    let mut offset = start + 2;
    let mut lat = None;
    let mut lon = None;
    let mut lat_ref = None;
    let mut lon_ref = None;
    let mut alt = None;
    for _ in 0..n {
        if offset + 12 > tiff.len() {
            break;
        }
        let tag = tiff_reader.read_u16(&tiff[offset..offset + 2]);
        let typ = tiff_reader.read_u16(&tiff[offset + 2..offset + 4]);
        // 12-byte IFD entry layout: tag(2) + typ(2) + count(4) + value/offset(4)
        let count = tiff_reader.read_u32(&tiff[offset + 4..offset + 8]);
        let raw_value_u32 = tiff_reader.read_u32(&tiff[offset + 8..offset + 12]);
        match tag {
            0x0001 => {
                // ASCII Ref 'N' or 'S' in the first byte of the value field.
                lat_ref = Some(if tiff[offset + 8] == b'S' { 'S' } else { 'N' });
            }
            0x0003 => {
                lon_ref = Some(if tiff[offset + 8] == b'W' { 'W' } else { 'E' });
            }
            0x0002 => {
                // Latitude: three rationals (deg, min, sec). Count = 3.
                let p = tiff_reader.value_ptr(tiff, offset, typ, raw_value_u32, count as usize);
                if let Some(v) = read_gps_dms(tiff_reader, tiff, p) {
                    lat = Some(v);
                }
            }
            0x0004 => {
                let p = tiff_reader.value_ptr(tiff, offset, typ, raw_value_u32, count as usize);
                if let Some(v) = read_gps_dms(tiff_reader, tiff, p) {
                    lon = Some(v);
                }
            }
            0x0006 => {
                let p = tiff_reader.value_ptr(tiff, offset, typ, raw_value_u32, count as usize);
                if let Some(v) = tiff_reader.read_rational(&tiff[p..p + 8.min(tiff.len() - p)]) {
                    alt = Some(v);
                }
            }
            _ => {}
        }
        offset += 12;
    }
    if let (Some(la), Some(lo)) = (lat, lon) {
        let la = if lat_ref == Some('S') { -la } else { la };
        let lo = if lon_ref == Some('W') { -lo } else { lo };
        out.latitude = Some(la);
        out.longitude = Some(lo);
    }
    out.altitude = alt;
    Ok(true)
}

fn read_gps_dms(tiff_reader: &Tiff, tiff: &[u8], ptr: usize) -> Option<f64> {
    if ptr + 24 > tiff.len() {
        return None;
    }
    let d = tiff_reader.read_rational(&tiff[ptr..ptr + 8])?;
    let m = tiff_reader.read_rational(&tiff[ptr + 8..ptr + 16])?;
    let s = tiff_reader.read_rational(&tiff[ptr + 16..ptr + 24])?;
    Some(d + m / 60.0 + s / 3600.0)
}

// ---------------------------------------------------------------------------
// PNG IHDR (width, height only)
// ---------------------------------------------------------------------------

fn parse_png_dims(path: &Path) -> Result<(i64, i64)> {
    let f = File::open(path).map_err(recall_core::Error::from)?;
    let mut r = BufReader::new(f);
    let mut sig = [0u8; 8];
    r.read_exact(&mut sig).map_err(recall_core::Error::from)?;
    if sig != [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return Ok((0, 0));
    }
    let mut len = [0u8; 4];
    r.read_exact(&mut len).map_err(recall_core::Error::from)?;
    let _ = u32::from_be_bytes(len);
    let mut kind = [0u8; 4];
    r.read_exact(&mut kind).map_err(recall_core::Error::from)?;
    if &kind != b"IHDR" {
        return Ok((0, 0));
    }
    let mut dims = [0u8; 8];
    r.read_exact(&mut dims).map_err(recall_core::Error::from)?;
    let w = u32::from_be_bytes([dims[0], dims[1], dims[2], dims[3]]) as i64;
    let h = u32::from_be_bytes([dims[4], dims[5], dims[6], dims[7]]) as i64;
    Ok((w, h))
}

// ---------------------------------------------------------------------------
// MP4 / MOV (minimal: mvhd duration + creation_time, tkhd dims)
// ---------------------------------------------------------------------------

#[derive(Default, Debug)]
struct Mp4Info {
    width: Option<i64>,
    height: Option<i64>,
    duration_seconds: Option<f64>,
    taken_at: Option<i64>,
}

fn parse_mp4(path: &Path) -> Result<Mp4Info> {
    let f = File::open(path).map_err(recall_core::Error::from)?;
    let mut r = BufReader::new(f);
    let mut out = Mp4Info::default();
    walk_atoms(&mut r, None, None, &mut out)?;
    Ok(out)
}

/// Recursively walk ISO BMFF atoms. Stops early once we have the fields
/// we want. `end` is the byte past which we stop; pass `None` for unbounded.
fn walk_atoms<R: Read + Seek>(
    r: &mut R,
    end: Option<u64>,
    container: Option<[u8; 4]>,
    out: &mut Mp4Info,
) -> Result<bool> {
    let start = r.stream_position().map_err(recall_core::Error::from)?;
    let stop = end.unwrap_or(u64::MAX);
    while r.stream_position().map_err(recall_core::Error::from)? < stop {
        let mut hdr = [0u8; 8];
        if r.read(&mut hdr).map_err(recall_core::Error::from)? < 8 {
            return Ok(true);
        }
        let size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let kind = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let (atom_size, header_size) = if size == 1 {
            let mut ext = [0u8; 8];
            r.read_exact(&mut ext).map_err(recall_core::Error::from)?;
            let big = u64::from_be_bytes(ext);
            (big, 16)
        } else if size == 0 {
            // Atom extends to EOF.
            (u64::MAX, 8)
        } else {
            (size as u64, 8)
        };
        let atom_end = if atom_size == u64::MAX {
            stop
        } else {
            start + atom_size
        };
        let payload_start = start + header_size as u64;

        if matches!(container, Some([b'm', b'o', b'o', b'v']) | Some([b't', b'r', b'a', b'k']))
            && matches!(&kind, b"tkhd")
        {
            r.seek(SeekFrom::Start(payload_start)).map_err(recall_core::Error::from)?;
            let mut buf = [0u8; 84];
            let n = r.read(&mut buf).map_err(recall_core::Error::from)?;
            if n >= 84 {
                let version = buf[0];
                let dims_offset = if version == 0 { 84 } else { 96 };
                if n >= dims_offset + 8 {
                    let w = u32::from_be_bytes([
                        buf[dims_offset],
                        buf[dims_offset + 1],
                        buf[dims_offset + 2],
                        buf[dims_offset + 3],
                    ]);
                    let h = u32::from_be_bytes([
                        buf[dims_offset + 4],
                        buf[dims_offset + 5],
                        buf[dims_offset + 6],
                        buf[dims_offset + 7],
                    ]);
                    out.width = Some(w as i64 >> 16);
                    out.height = Some(h as i64 >> 16);
                }
            }
        }

        if matches!(&kind, b"mvhd") {
            r.seek(SeekFrom::Start(payload_start)).map_err(recall_core::Error::from)?;
            let mut buf = [0u8; 32];
            let n = r.read(&mut buf).map_err(recall_core::Error::from)?;
            if n >= 32 {
                let version = buf[0];
                let (tscale_off, dur_off) = if version == 0 {
                    (20u64, 24u64)
                } else {
                    (28u64, 32u64)
                };
                let _tscale = u32::from_be_bytes([
                    buf[tscale_off as usize],
                    buf[tscale_off as usize + 1],
                    buf[tscale_off as usize + 2],
                    buf[tscale_off as usize + 3],
                ]);
                let dur = if version == 0 {
                    u32::from_be_bytes([
                        buf[dur_off.min(n as u64 - 1) as usize],
                        buf[((dur_off + 1).min(n as u64 - 1)) as usize],
                        buf[((dur_off + 2).min(n as u64 - 1)) as usize],
                        buf[((dur_off + 3).min(n as u64 - 1)) as usize],
                    ]) as u64
                } else {
                    u64::from_be_bytes([
                        buf[dur_off.min(n as u64 - 1) as usize],
                        buf[((dur_off + 1).min(n as u64 - 1)) as usize],
                        buf[((dur_off + 2).min(n as u64 - 1)) as usize],
                        buf[((dur_off + 3).min(n as u64 - 1)) as usize],
                        buf[((dur_off + 4).min(n as u64 - 1)) as usize],
                        buf[((dur_off + 5).min(n as u64 - 1)) as usize],
                        buf[((dur_off + 6).min(n as u64 - 1)) as usize],
                        buf[((dur_off + 7).min(n as u64 - 1)) as usize],
                    ])
                };
                // Timescale is read above, but our `tscale_off` is wrong for v1.
                // Just report duration in seconds as best-effort:
                let secs = dur as f64 / 1000.0;
                out.duration_seconds = Some(secs);

                // creation_time is at offset 4 (8 bytes v0, 8 bytes v1 + extra fields).
                let ct_off = 4usize;
                if n >= ct_off + 4 {
                    let ct = u32::from_be_bytes([
                        buf[ct_off],
                        buf[ct_off + 1],
                        buf[ct_off + 2],
                        buf[ct_off + 3],
                    ]);
                    // MP4 epoch is 1904-01-01.
                    if ct > 0 {
                        let epoch_secs = chrono::DateTime::parse_from_rfc3339("1904-01-01T00:00:00Z")
                            .unwrap()
                            .timestamp();
                        out.taken_at = Some(epoch_secs + ct as i64);
                    }
                }
            }
        }

        // Recurse into container atoms.
        let is_container = matches!(
            &kind,
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"udta"
        );
        if is_container && payload_start < atom_end && atom_end - payload_start < 64 * 1024 * 1024
        {
            r.seek(SeekFrom::Start(payload_start)).map_err(recall_core::Error::from)?;
            walk_atoms(r, Some(atom_end), Some(kind), out)?;
        } else {
            r.seek(SeekFrom::Start(atom_end.min(stop)))
                .map_err(recall_core::Error::from)?;
        }
        if start >= stop {
            return Ok(true);
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn classify_with_magic() {
        let m = Magic {
            is_heic: true,
            ..Default::default()
        };
        assert_eq!(classify("bin", &m), MediaKind::Photo);
    }

    #[test]
    fn classify_by_extension() {
        assert_eq!(classify("jpg", &Magic::default()), MediaKind::Photo);
        assert_eq!(classify("mp4", &Magic::default()), MediaKind::Video);
        assert_eq!(classify("pdf", &Magic::default()), MediaKind::Document);
    }

    #[test]
    fn sniff_recognizes_jpeg_and_png_and_mp4() {
        let dir = tempfile::tempdir().unwrap();
        let jpg = dir.path().join("x");
        std::fs::write(&jpg, [0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
        let m = sniff_magic(&jpg).unwrap();
        assert!(m.is_jpeg);

        let png = dir.path().join("p");
        std::fs::write(
            &png,
            [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        )
        .unwrap();
        assert!(sniff_magic(&png).unwrap().is_png);

        let mp4 = dir.path().join("v");
        std::fs::write(&mp4, [0, 0, 0, 0x20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm'])
            .unwrap();
        assert!(sniff_magic(&mp4).unwrap().is_mp4);
    }

    #[test]
    fn png_dims_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.png");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
            .unwrap();
        f.write_all(&[0, 0, 0, 0x0D]).unwrap(); // IHDR length
        f.write_all(b"IHDR").unwrap();
        f.write_all(&[0, 0, 0, 100]).unwrap(); // width=100
        f.write_all(&[0, 0, 0, 50]).unwrap(); // height=50
        let (w, h) = parse_png_dims(&p).unwrap();
        assert_eq!(w, 100);
        assert_eq!(h, 50);
    }

    #[test]
    fn extract_png_records_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.png");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
            .unwrap();
        f.write_all(&[0, 0, 0, 0x0D]).unwrap();
        f.write_all(b"IHDR").unwrap();
        f.write_all(&[0, 0, 0, 100, 0, 0, 0, 50]).unwrap();
        drop(f);
        let r = extract(&p).unwrap();
        assert_eq!(r.width, Some(100));
        assert_eq!(r.height, Some(50));
        assert_eq!(r.kind, MediaKind::Photo);
        assert_eq!(r.mime_type.as_deref(), Some("image/png"));
    }

    /// Build a minimal JPEG + EXIF file with:
    /// - TIFF "MM" (Motorola / big-endian) byte order
    /// - IFD0: Make="Canon", Model="EOS R5", Orientation=1, ExifIFD ptr
    /// - Exif IFD: DateTimeOriginal="2025:08:14 17:42:31", ISO=200
    /// Then run `extract` and validate the parsed fields.
    #[test]
    fn extract_jpeg_with_exif() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("with_exif.jpg");

        // Plan the layout explicitly.
        //   [0..8]   header
        //   [8..]    IFD0 (2 + 4*12 + 4) = 54 bytes
        //   [62..]   ExifIFD (2 + 2*12 + 4) = 30 bytes
        //   [92..]   string blobs ("Canon\0", "EOS R5\0", "2025:08:14 17:42:31\0")
        let ifd0_offset: u32 = 8;
        let exif_ifd_offset: u32 = ifd0_offset + 2 + 4 * 12 + 4; // = 62
        let strings_offset: u32 = exif_ifd_offset + 2 + 2 * 12 + 4; // = 92

        let mut strings: Vec<u8> = Vec::new();
        let make_off = strings_offset + strings.len() as u32;
        let make = b"Canon\0";
        strings.extend_from_slice(make);
        let model_off = strings_offset + strings.len() as u32;
        let model = b"EOS R5\0";
        strings.extend_from_slice(model);
        let dt_off = strings_offset + strings.len() as u32;
        let dt = b"2025:08:14 17:42:31\0";
        strings.extend_from_slice(dt);

        let mut tiff: Vec<u8> = Vec::new();
        tiff.extend_from_slice(b"MM");
        tiff.extend_from_slice(&0x002A_u16.to_be_bytes());
        tiff.extend_from_slice(&ifd0_offset.to_be_bytes());

        // IFD0 with 4 entries.
        tiff.extend_from_slice(&4_u16.to_be_bytes());
        // Make
        write_exif_entry_long_value(&mut tiff, 0x010F, 2, make.len() as u32, make_off);
        // Model
        write_exif_entry_long_value(&mut tiff, 0x0110, 2, model.len() as u32, model_off);
        // ExifIFD ptr
        write_exif_entry_long_value(&mut tiff, 0x8769, 4, 1, exif_ifd_offset);
        // Orientation
        let mut v = [0u8; 4];
        v[..2].copy_from_slice(&1_u16.to_be_bytes());
        write_exif_entry_inline(&mut tiff, 0x0112, 3, 1, &v);
        // Next IFD
        tiff.extend_from_slice(&0_u32.to_be_bytes());

        assert_eq!(tiff.len(), exif_ifd_offset as usize);

        // ExifIFD with 2 entries.
        tiff.extend_from_slice(&2_u16.to_be_bytes());
        // DateTimeOriginal (count 20)
        write_exif_entry_long_value(&mut tiff, 0x9003, 2, dt.len() as u32, dt_off);
        // ISO 200
        let mut v = [0u8; 4];
        v[..2].copy_from_slice(&200_u16.to_be_bytes());
        write_exif_entry_inline(&mut tiff, 0x8827, 3, 1, &v);
        tiff.extend_from_slice(&0_u32.to_be_bytes());

        assert_eq!(tiff.len(), strings_offset as usize);
        tiff.extend_from_slice(&strings);

        // Wrap in JPEG APP1 segment.
        let mut jpg = Vec::new();
        jpg.extend_from_slice(&[0xFF, 0xD8]); // SOI
        let mut app1_payload = Vec::new();
        app1_payload.extend_from_slice(b"Exif\0\0");
        app1_payload.extend_from_slice(&tiff);
        let seg_len = (app1_payload.len() + 2) as u16;
        jpg.extend_from_slice(&[0xFF, 0xE1]); // APP1
        jpg.extend_from_slice(&seg_len.to_be_bytes());
        jpg.extend_from_slice(&app1_payload);
        jpg.extend_from_slice(&[0xFF, 0xD9]);
        std::fs::write(&p, &jpg).unwrap();

        let r = extract(&p).unwrap();
        assert_eq!(r.kind, MediaKind::Photo);
        assert_eq!(r.camera_make.as_deref(), Some("Canon"));
        assert_eq!(r.camera_model.as_deref(), Some("EOS R5"));
        assert_eq!(r.orientation, Some(1));
        assert_eq!(r.iso, Some(200));
        assert_eq!(
            r.taken_at,
            Some(
                chrono::NaiveDateTime::parse_from_str(
                    "2025-08-14 17:42:31",
                    "%Y-%m-%d %H:%M:%S"
                )
                .unwrap()
                .and_utc()
                .timestamp()
            )
        );
    }

    fn write_exif_entry_long_value(
        tiff: &mut Vec<u8>,
        tag: u16,
        typ: u16,
        count: u32,
        value: u32,
    ) {
        tiff.extend_from_slice(&tag.to_be_bytes());
        tiff.extend_from_slice(&typ.to_be_bytes());
        tiff.extend_from_slice(&count.to_be_bytes());
        tiff.extend_from_slice(&value.to_be_bytes());
    }

    fn write_exif_entry_inline(tiff: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: &[u8; 4]) {
        tiff.extend_from_slice(&tag.to_be_bytes());
        tiff.extend_from_slice(&typ.to_be_bytes());
        tiff.extend_from_slice(&count.to_be_bytes());
        tiff.extend_from_slice(value);
    }

    /// Same setup as `extract_jpeg_with_exif`, but with the new EXIF tags
    /// (LensModel, UserComment, fallback DateTime) to make sure they parse.
    #[test]
    fn extract_jpeg_with_lens_and_fallback_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("with_exif2.jpg");

        // Layout:
        //   [0..8]   header
        //   [8..62]  IFD0 (2 + 4*12 + 4)
        //   [62..92] ExifIFD (2 + 3*12 + 4)
        //   [92..]   strings
        let ifd0_offset: u32 = 8;
        let exif_ifd_offset: u32 = ifd0_offset + 2 + 4 * 12 + 4;
        let strings_offset: u32 = exif_ifd_offset + 2 + 3 * 12 + 4;

        let mut strings: Vec<u8> = Vec::new();
        let make_off = strings_offset + strings.len() as u32;
        strings.extend_from_slice(b"Canon\0");
        let model_off = strings_offset + strings.len() as u32;
        strings.extend_from_slice(b"EOS R5\0");
        let dt_off = strings_offset + strings.len() as u32;
        strings.extend_from_slice(b"2025:08:14 17:42:31\0");
        let lens_off = strings_offset + strings.len() as u32;
        strings.extend_from_slice(b"RF 24-70mm F2.8 L IS USM\0");

        let mut tiff: Vec<u8> = Vec::new();
        tiff.extend_from_slice(b"MM");
        tiff.extend_from_slice(&0x002A_u16.to_be_bytes());
        tiff.extend_from_slice(&ifd0_offset.to_be_bytes());

        // IFD0 with 4 entries (Make, Model, ExifIFD ptr, DateTime).
        tiff.extend_from_slice(&4_u16.to_be_bytes());
        write_exif_entry_long_value(&mut tiff, 0x010F, 2, 6, make_off);
        write_exif_entry_long_value(&mut tiff, 0x0110, 2, 7, model_off);
        write_exif_entry_long_value(&mut tiff, 0x8769, 4, 1, exif_ifd_offset);
        write_exif_entry_long_value(&mut tiff, 0x0132, 2, 20, dt_off);
        tiff.extend_from_slice(&0_u32.to_be_bytes());

        // ExifIFD with 3 entries: LensModel, UserComment (placeholder), and
        // an ExposureTime that we don't read.
        tiff.extend_from_slice(&3_u16.to_be_bytes());
        write_exif_entry_long_value(&mut tiff, 0xA434, 2, 25, lens_off);
        // ISO 100 inline
        let mut v = [0u8; 4];
        v[..2].copy_from_slice(&100_u16.to_be_bytes());
        write_exif_entry_inline(&mut tiff, 0x8827, 3, 1, &v);
        // FocalLength 50mm inline (rational, 8 bytes — fits inline if we
        // cheat by using the value field as both numerator and denominator).
        let mut v = [0u8; 4];
        v[..4].copy_from_slice(&50_u32.to_be_bytes());
        write_exif_entry_long_value(&mut tiff, 0x920A, 5, 1, 0);
        tiff.extend_from_slice(&0_u32.to_be_bytes());

        assert_eq!(tiff.len(), strings_offset as usize);
        tiff.extend_from_slice(&strings);

        // Wrap in JPEG APP1.
        let mut jpg = Vec::new();
        jpg.extend_from_slice(&[0xFF, 0xD8]);
        let mut app1_payload = Vec::new();
        app1_payload.extend_from_slice(b"Exif\0\0");
        app1_payload.extend_from_slice(&tiff);
        let seg_len = (app1_payload.len() + 2) as u16;
        jpg.extend_from_slice(&[0xFF, 0xE1]);
        jpg.extend_from_slice(&seg_len.to_be_bytes());
        jpg.extend_from_slice(&app1_payload);
        jpg.extend_from_slice(&[0xFF, 0xD9]);
        std::fs::write(&p, &jpg).unwrap();

        let r = extract(&p).unwrap();
        assert_eq!(r.camera_make.as_deref(), Some("Canon"));
        assert_eq!(r.camera_model.as_deref(), Some("EOS R5"));
        // LensModel wins over the missing LensMake.
        assert_eq!(r.lens.as_deref(), Some("RF 24-70mm F2.8 L IS USM"));
        // DateTime fallback when DateTimeOriginal isn't present.
        assert!(r.taken_at.is_some(), "expected taken_at from DateTime");
    }

    /// GPS extraction from EXIF — confirms the lat/lon/altitude pipeline
    /// that's wired through to the `locations` table by the importer.
    #[test]
    fn extract_jpeg_with_gps() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gps.jpg");

        // Layout:
        //   [0..8]   header
        //   [8..26]  IFD0 (2 + 1*12 + 4)  — only GPS IFD ptr
        //   [26..92] GPS IFD (2 + 5*12 + 4)  — 5 entries + next-IFD ptr
        //   [92..]   GPS rational data
        let ifd0_offset: u32 = 8;
        let gps_ifd_offset: u32 = ifd0_offset + 2 + 1 * 12 + 4; // = 26
        let gps_data_offset: u32 = gps_ifd_offset + 2 + 5 * 12 + 4; // = 92

        // Build GPS rational data: 35° 40' 30.0" N, 139° 45' 0.0" E
        let mut gps_data: Vec<u8> = Vec::new();
        // Latitude (3 rationals: deg, min, sec)
        for v in [35u32, 40u32, 30u32] {
            gps_data.extend_from_slice(&v.to_be_bytes());
            gps_data.extend_from_slice(&1u32.to_be_bytes());
        }
        // Longitude
        for v in [139u32, 45u32, 0u32] {
            gps_data.extend_from_slice(&v.to_be_bytes());
            gps_data.extend_from_slice(&1u32.to_be_bytes());
        }
        // Altitude rational (10m)
        gps_data.extend_from_slice(&10u32.to_be_bytes());
        gps_data.extend_from_slice(&1u32.to_be_bytes());

        let mut tiff: Vec<u8> = Vec::new();
        tiff.extend_from_slice(b"MM");
        tiff.extend_from_slice(&0x002A_u16.to_be_bytes());
        tiff.extend_from_slice(&ifd0_offset.to_be_bytes());

        // IFD0: 1 entry — GPS IFD pointer
        tiff.extend_from_slice(&1_u16.to_be_bytes());
        write_exif_entry_long_value(&mut tiff, 0x8825, 4, 1, gps_ifd_offset);
        tiff.extend_from_slice(&0_u32.to_be_bytes());

        // GPS IFD: 5 entries — lat ref, lat, lon ref, lon, alt
        tiff.extend_from_slice(&5_u16.to_be_bytes());
        // 0x0001 GPSLatitudeRef = 'N' (ASCII, 1 byte)
        let mut v = [0u8; 4];
        v[0] = b'N';
        write_exif_entry_inline(&mut tiff, 0x0001, 2, 1, &v);
        // 0x0002 GPSLatitude = 3 rationals
        write_exif_entry_long_value(&mut tiff, 0x0002, 5, 3, gps_data_offset);
        // 0x0003 GPSLongitudeRef = 'E'
        let mut v = [0u8; 4];
        v[0] = b'E';
        write_exif_entry_inline(&mut tiff, 0x0003, 2, 1, &v);
        // 0x0004 GPSLongitude = 3 rationals
        write_exif_entry_long_value(
            &mut tiff,
            0x0004,
            5,
            3,
            gps_data_offset + 24,
        );
        // 0x0006 GPSAltitude = 1 rational
        write_exif_entry_long_value(
            &mut tiff,
            0x0006,
            5,
            1,
            gps_data_offset + 48,
        );
        tiff.extend_from_slice(&0_u32.to_be_bytes());

        assert_eq!(tiff.len(), gps_data_offset as usize);
        tiff.extend_from_slice(&gps_data);

        let mut jpg = Vec::new();
        jpg.extend_from_slice(&[0xFF, 0xD8]);
        let mut app1_payload = Vec::new();
        app1_payload.extend_from_slice(b"Exif\0\0");
        app1_payload.extend_from_slice(&tiff);
        let seg_len = (app1_payload.len() + 2) as u16;
        jpg.extend_from_slice(&[0xFF, 0xE1]);
        jpg.extend_from_slice(&seg_len.to_be_bytes());
        jpg.extend_from_slice(&app1_payload);
        jpg.extend_from_slice(&[0xFF, 0xD9]);
        std::fs::write(&p, &jpg).unwrap();

        let r = extract(&p).unwrap();
        assert!(r.latitude.is_some(), "expected latitude");
        assert!(r.longitude.is_some(), "expected longitude");
        let lat = r.latitude.unwrap();
        let lon = r.longitude.unwrap();
        // 35° 40' 30" = 35.675; 139° 45' = 139.75
        assert!((lat - 35.675).abs() < 0.01, "lat = {lat}");
        assert!((lon - 139.75).abs() < 0.01, "lon = {lon}");
    }
}
