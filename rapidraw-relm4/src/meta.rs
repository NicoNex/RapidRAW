//! Minimal EXIF readout for the editor toolbar: shutter, aperture, ISO, focal
//! length, capture date. Pure-Rust (`kamadak-exif`), no system deps.

use std::collections::BTreeMap;
use std::path::Path;

use exif::{In, Tag};

/// Format a one-line EXIF summary, or `None` if the file has no readable EXIF.
pub fn read_summary(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut buf = std::io::BufReader::new(file);
    let reader = exif::Reader::new().read_from_container(&mut buf).ok()?;

    let disp = |tag| {
        reader
            .get_field(tag, In::PRIMARY)
            .map(|f| f.display_value().to_string())
    };

    let mut parts = Vec::new();
    if let Some(v) = disp(Tag::ExposureTime) {
        parts.push(format!("{v}s"));
    }
    if let Some(v) = disp(Tag::FNumber) {
        parts.push(format!("f/{v}"));
    }
    if let Some(v) = disp(Tag::ISOSpeed).or_else(|| disp(Tag::PhotographicSensitivity)) {
        parts.push(format!("ISO {v}"));
    }
    if let Some(v) = disp(Tag::FocalLength) {
        parts.push(format!("{v}mm"));
    }
    if let Some(v) = disp(Tag::DateTimeOriginal) {
        parts.push(v);
    }

    (!parts.is_empty()).then(|| parts.join("  ·  "))
}

/// Read the full EXIF tag set as a `name -> display string` map for the Info
/// panel, using the shared core extractor (same flow as the Tauri UI): standard
/// EXIF via kamadak, with a rawler decoder fallback for RAW without container
/// EXIF.
pub fn read_full_exif(path: &Path) -> BTreeMap<String, String> {
    match std::fs::read(path) {
        Ok(bytes) => rapidraw_core::exif::extract_metadata(&bytes),
        Err(_) => BTreeMap::new(),
    }
}
