use std::path::PathBuf;
use std::sync::Arc;

use image::DynamicImage;
use rapidraw_core::image_processing::{AllAdjustments, GpuContext};
use rapidraw_core::mask_generation::MaskDefinition;
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
    /// Loaded 3D LUT (.cube/.3dl), applied at `adjustments.global.lut_intensity`.
    pub lut: Option<Arc<Lut>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            current_folder: None,
            active_path: None,
            base_image: None,
            adjustments: AllAdjustments::default(),
            masks: Vec::new(),
            lut: None,
        }
    }
}
