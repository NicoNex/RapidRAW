//! Tauri-free image decode helpers extracted from src-tauri/src/image_loader.rs.
//!
//! Coupling-cut: only the decode functions that have no `tauri::State`/AppSettings
//! dependency are moved here. `load_base_image_from_bytes`, `load_and_composite`,
//! `composite_patches_on_image`, and the cache helpers stay in src-tauri because
//! they read AppSettings / mask_generation / patch data not part of core's
//! minimal loop. The cancel-token plumbing is also dropped (core has no cancel
//! infrastructure); callers that need it keep their own copies in src-tauri.

use anyhow::{Context, Result};
use exif::{Reader as ExifReader, Tag};
use image::{DynamicImage, ImageReader};
use rawler::Orientation;
use std::io::Cursor;

use std::path::Path;

use crate::formats::is_raw_file;
use crate::image_processing::apply_cpu_default_raw_processing;
use crate::image_processing::apply_orientation;
use crate::raw_processing::develop_raw_image;

/// Decode standard image bytes, honoring EXIF orientation, into an Rgb32F image.
pub fn load_image_with_orientation(bytes: &[u8]) -> Result<DynamicImage> {
    let cursor = Cursor::new(bytes);
    let mut reader = ImageReader::new(cursor.clone())
        .with_guessed_format()
        .context("Failed to guess image format")?;

    reader.no_limits();

    let image = reader.decode().context("Failed to decode image")?;

    let oriented_image = {
        let exif_reader = ExifReader::new();
        if let Ok(exif) = exif_reader.read_from_container(&mut cursor.clone()) {
            if let Some(orientation) = exif
                .get_field(Tag::Orientation, exif::In::PRIMARY)
                .and_then(|f| f.value.get_uint(0))
            {
                apply_orientation(image, Orientation::from_u16(orientation as u16))
            } else {
                image
            }
        } else {
            image
        }
    };

    Ok(DynamicImage::ImageRgb32F(oriented_image.to_rgb32f()))
}

/// Decode any supported image (standard or RAW) into a display-ready base image.
///
/// RAW files are developed via `develop_raw_image` (engine default `linear_mode`
/// = "auto", matching `default_linear_raw_mode()` in src-tauri) followed by the
/// CPU default RAW processing pass. Standard formats honor EXIF orientation.
pub fn load_base_image(path: &Path) -> Result<DynamicImage> {
    let bytes = std::fs::read(path).context("Failed to read image file")?;
    if is_raw_file(path) {
        let mut img = develop_raw_image(&bytes, false, 2.5, "auto".to_string(), None)?;
        apply_cpu_default_raw_processing(&mut img);
        Ok(img)
    } else {
        load_image_with_orientation(&bytes)
    }
}
