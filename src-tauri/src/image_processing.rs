//! Tauri-side shim over `rapidraw_core::image_processing`.
//!
//! The engine kernels and pure helpers now live in `rapidraw_core`. This module
//! re-exports them under the historical `crate::image_processing::*` path so the
//! rest of `src-tauri` keeps compiling unchanged, and adds back the few
//! Tauri/`AppState`-coupled wrappers that intentionally stay out of core
//! (`resolve_tonemapper_override*`, `calculate_auto_adjustments`, and the cached
//! `process_and_get_dynamic_image*` orchestration).

pub use rapidraw_core::image_processing::*;

// Cached GPU orchestration + engine kernel types historically lived under
// `crate::image_processing::*` too; re-export them so callers keep compiling.
pub use crate::gpu_processing::{
    get_or_init_gpu_context, process_and_get_dynamic_image,
    process_and_get_dynamic_image_with_analytics,
};
pub use rapidraw_core::gpu_processing::RenderRequest;

use crate::app_state::AppState;

/// Resolve the tonemapper override from full `AppSettings`.
///
/// Coupling-cut wrapper: reads the settings primitives and delegates to
/// `rapidraw_core::image_processing::resolve_tonemapper_override`.
pub fn resolve_tonemapper_override(
    settings: &crate::AppSettings,
    is_raw: bool,
) -> Option<u32> {
    rapidraw_core::image_processing::resolve_tonemapper_override(
        settings.tonemapper_override_enabled.unwrap_or(false),
        is_raw,
        settings.default_raw_tonemapper.as_deref().unwrap_or("agx"),
        settings
            .default_non_raw_tonemapper
            .as_deref()
            .unwrap_or("basic"),
    )
}

/// Resolve the tonemapper override by loading settings from the app handle.
pub fn resolve_tonemapper_override_from_handle(
    app_handle: &tauri::AppHandle,
    is_raw: bool,
) -> Option<u32> {
    let settings = crate::app_settings::load_settings(app_handle.clone()).unwrap_or_default();
    resolve_tonemapper_override(&settings, is_raw)
}

#[tauri::command]
pub fn calculate_auto_adjustments(
    state: tauri::State<AppState>,
) -> Result<serde_json::Value, String> {
    let original_image = state
        .original_image
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("No image loaded for auto adjustments")?
        .image
        .clone();

    let results = perform_auto_analysis(&original_image);

    Ok(auto_results_to_json(&results))
}
