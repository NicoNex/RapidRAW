use std::path::{Path, PathBuf};

use gtk::gdk;
use gtk::glib::Bytes;
use image::RgbaImage;

const EXT: &[&str] = &[
    "jpg", "jpeg", "png", "tiff", "tif", "webp", "raw", "arw", "cr2", "cr3", "nef", "orf", "raf",
    "dng", "rw2", "pef", "srw", "3fr", "mef",
];

/// Scan a directory (non-recursively) for supported image/RAW files, sorted.
pub fn scan_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| EXT.contains(&x.to_lowercase().as_str()))
                .unwrap_or(false)
        })
        .collect();
    v.sort();
    v
}

/// Build a `gdk::MemoryTexture` from an RGBA8 image. MUST be called on the GTK
/// main thread — gdk objects are not `Send`.
pub fn texture_from_rgba(rgba: &RgbaImage) -> gdk::MemoryTexture {
    let (w, h) = rgba.dimensions();
    let bytes = Bytes::from(rgba.as_raw());
    gdk::MemoryTexture::new(
        w as i32,
        h as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        (w * 4) as usize,
    )
}
