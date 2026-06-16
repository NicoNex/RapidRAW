use std::sync::{Arc, Mutex};

use crate::image_processing::GpuContext;

/// Build a compute-only wgpu `GpuContext` with no surface/window (no Tauri).
///
/// Mirrors the verified compute-only init path in
/// `src-tauri/src/gpu_processing.rs` (the `surface_opt = None` branch), with
/// surface/window/flag-path code stripped.
pub fn headless_context() -> Result<GpuContext, String> {
    let instance_desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    let instance = wgpu::Instance::new(instance_desc);

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        ..Default::default()
    }))
    .map_err(|e| format!("Failed to find a wgpu adapter: {}", e))?;

    let mut required_features = wgpu::Features::empty();
    if adapter
        .features()
        .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    {
        required_features |= wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    }

    let limits = adapter.limits();

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("RapidRAW Headless Device"),
        required_features,
        required_limits: limits.clone(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| e.to_string())?;

    Ok(GpuContext {
        device: Arc::new(device),
        queue: Arc::new(queue),
        limits,
        display: Arc::new(Mutex::new(None)),
    })
}
