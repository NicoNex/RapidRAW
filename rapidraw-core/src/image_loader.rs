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

use crate::image_processing::apply_orientation;

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
