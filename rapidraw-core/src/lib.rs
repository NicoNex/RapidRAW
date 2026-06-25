//! rapidraw-core: tauri-free image engine extracted from src-tauri.

pub mod formats;
pub mod albums;
pub mod folders;
pub mod raw_processing;
pub use raw_processing::embedded_preview;
pub mod auto_curve;
pub use auto_curve::auto_tone_curve;
pub mod lut_processing;
pub mod image_processing;
pub mod gpu_processing;
pub mod exif;
pub mod image_loader;
pub use image_loader::load_base_image;
pub mod mask_generation;

/// ONNX inference (AI masks, inpaint, denoise, CLIP). Feature-gated so core
/// stays lean for consumers that don't need `ort`.
#[cfg(feature = "ai")]
pub mod ai;

/// HTTP client for the external generative-inpaint backend (local AI-connector
/// or cloud middleware). Feature-gated with `ai` since it needs `reqwest`.
#[cfg(feature = "ai")]
pub mod ai_connector;

mod context;
pub use context::headless_context;

mod render;
pub use render::render;
