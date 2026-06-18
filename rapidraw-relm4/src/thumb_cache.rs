//! On-disk thumbnail cache. Decoding a RAW just to make a 300px thumbnail is
//! the expensive part of populating the grid; caching a small JPEG avoids it on
//! every folder reopen / filter / restart.
//!
//! Key = hash(absolute path + mtime + dim), so an edited/replaced file misses
//! and is re-decoded automatically. No explicit invalidation needed.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use image::RgbaImage;

fn cache_path(image: &Path, dim: u32) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let meta = std::fs::metadata(image).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    let abs = std::fs::canonicalize(image).unwrap_or_else(|_| image.to_path_buf());
    let mut h = DefaultHasher::new();
    abs.hash(&mut h);
    mtime.hash(&mut h);
    dim.hash(&mut h);
    Some(
        base.join("rapidraw-relm4")
            .join("thumbs")
            .join(format!("{:016x}.jpg", h.finish())),
    )
}

/// Load a cached thumbnail, if present and fresh.
pub fn load(image: &Path, dim: u32) -> Option<RgbaImage> {
    let p = cache_path(image, dim)?;
    Some(image::open(p).ok()?.to_rgba8())
}

/// Write a thumbnail to the cache (best-effort; errors ignored).
pub fn save(image: &Path, dim: u32, rgba: &RgbaImage) {
    let Some(p) = cache_path(image, dim) else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Store as JPEG q85 (thumbnails don't need alpha).
    let rgb = image::DynamicImage::ImageRgba8(rgba.clone()).to_rgb8();
    if let Ok(file) = std::fs::File::create(&p) {
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::io::BufWriter::new(file),
            85,
        );
        use image::ImageEncoder;
        let _ = enc.write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        );
    }
}
