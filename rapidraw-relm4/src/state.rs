use std::path::PathBuf;
use std::sync::Arc;

use image::DynamicImage;
use rapidraw_core::image_processing::{AllAdjustments, GpuContext};
use rapidraw_core::mask_generation::{AiPatchDefinition, MaskDefinition};
use rapidraw_core::lut_processing::Lut;

/// Shared, cheaply-clonable handle to the GPU engine context.
#[derive(Clone)]
pub struct Engine {
    pub ctx: Arc<GpuContext>,
}

/// Per-session editing state: the open folder, the active image, its decoded
/// base, and the current adjustment stack.
pub struct Session {
    pub current_folder: Option<PathBuf>,
    pub active_path: Option<PathBuf>,
    pub base_image: Option<Arc<DynamicImage>>,
    pub adjustments: AllAdjustments,
    pub masks: Vec<MaskDefinition>,
    /// AI inpaint patches (generative replace / quick erase). Baked onto the
    /// base before the adjustment pipeline runs, same as the Tauri UI.
    pub ai_patches: Vec<AiPatchDefinition>,
    /// Loaded 3D LUT (.cube/.3dl), applied at `adjustments.global.lut_intensity`.
    pub lut: Option<Arc<Lut>>,
    /// Editable photo metadata (Info panel): author fields, tags, colour label.
    pub meta: crate::sidecar::ImageMeta,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            current_folder: None,
            active_path: None,
            base_image: None,
            adjustments: AllAdjustments::default(),
            masks: Vec::new(),
            ai_patches: Vec::new(),
            lut: None,
            meta: Default::default(),
        }
    }
}
