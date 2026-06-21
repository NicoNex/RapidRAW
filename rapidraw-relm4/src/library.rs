use std::path::{Path, PathBuf};

use gtk::gdk;
use gtk::glib::Bytes;
use image::RgbaImage;
pub use rapidraw_core::formats::{is_raw_file as is_raw, is_supported_image_file};

/// Library raw-status filter, mirroring the original `RawStatus`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RawFilter {
    All,
    RawOnly,
    NonRawOnly,
    /// Prefer raw: hide a non-raw file when a raw with the same stem exists.
    PreferRaw,
}

/// Library sort order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortBy {
    Name,
    DateNewest,
    DateOldest,
    RatingDesc,
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase()
}

/// Apply the raw filter, name search, then sort order to a scanned image list.
pub fn arrange(
    all: &[PathBuf],
    filter: RawFilter,
    sort: SortBy,
    search: &str,
    ratings: &std::collections::HashMap<PathBuf, u8>,
) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match filter {
        RawFilter::All => all.to_vec(),
        RawFilter::RawOnly => all.iter().filter(|p| is_raw(p)).cloned().collect(),
        RawFilter::NonRawOnly => all.iter().filter(|p| !is_raw(p)).cloned().collect(),
        RawFilter::PreferRaw => {
            let raw_stems: std::collections::HashSet<String> =
                all.iter().filter(|p| is_raw(p)).map(|p| stem(p)).collect();
            all.iter()
                .filter(|p| is_raw(p) || !raw_stems.contains(&stem(p)))
                .cloned()
                .collect()
        }
    };
    if !search.trim().is_empty() {
        let needle = search.to_lowercase();
        v.retain(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_lowercase().contains(&needle))
                .unwrap_or(false)
        });
    }
    match sort {
        SortBy::Name => v.sort(),
        SortBy::RatingDesc => {
            let r = |p: &PathBuf| *ratings.get(p).unwrap_or(&0);
            v.sort_by(|a, b| r(b).cmp(&r(a)).then(a.cmp(b)));
        }
        SortBy::DateNewest | SortBy::DateOldest => {
            let mtime = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            v.sort_by(|a, b| {
                let (ma, mb) = (mtime(a), mtime(b));
                if sort == SortBy::DateNewest {
                    mb.cmp(&ma).then(a.cmp(b))
                } else {
                    ma.cmp(&mb).then(a.cmp(b))
                }
            });
        }
    }
    v
}

/// Scan a directory (non-recursively) for supported image/RAW files, sorted.
pub fn scan_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| is_supported_image_file(p))
        .collect();
    v.sort();
    v
}

/// Build a `gdk::MemoryTexture` from an RGBA8 image. MUST be called on the GTK
/// main thread — gdk objects are not `Send`.
pub fn texture_from_rgba(rgba: &RgbaImage) -> gdk::MemoryTexture {
    let (w, h) = rgba.dimensions();
    // from_owned takes ownership of the buffer copy, removing any lifetime
    // ambiguity about what backs the texture.
    let bytes = Bytes::from_owned(rgba.as_raw().clone());
    let tex = gdk::MemoryTexture::new(
        w as i32,
        h as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        (w * 4) as usize,
    );
    log::debug!("texture built {}x{}", w, h);
    tex
}
