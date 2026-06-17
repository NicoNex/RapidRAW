//! Tauri orchestration layer for the GPU engine.
//!
//! The image-processing kernels (`GpuProcessor`, `RenderRequest`, `Roi`,
//! `WgpuDisplay`, `DisplayTransform`, `to_rgba_f16`) now live in
//! `rapidraw_core::gpu_processing`. This module keeps only the pieces that are
//! coupled to Tauri/`AppState`: GPU context initialization and the cached
//! `process_and_get_dynamic_image*` wrappers.

#[cfg(not(any(target_os = "android", target_os = "linux")))]
use std::num::NonZero;
use std::sync::Arc;
use std::time::Instant;

use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba};

#[cfg(not(any(target_os = "android", target_os = "linux")))]
use tauri::Manager;
use wgpu::util::{DeviceExt, TextureDataOrder};

// Re-export the engine kernel types through this module so the historical
// `crate::gpu_processing::{GpuProcessor, RenderRequest, Roi, ...}` paths keep
// resolving across `src-tauri`.
pub use rapidraw_core::gpu_processing::{GpuProcessor, RenderRequest, Roi, to_rgba_f16};
// Display types are only constructed in the surface-backed (non-Linux/Android)
// path of `get_or_init_gpu_context`.
#[cfg(not(any(target_os = "android", target_os = "linux")))]
use rapidraw_core::gpu_processing::{DisplayTransform, WgpuDisplay};
use rapidraw_core::image_processing::GpuContext;

use crate::{AppState, GpuImageCache};

pub fn get_or_init_gpu_context(
    state: &tauri::State<AppState>,
    _app_handle: &tauri::AppHandle,
) -> Result<GpuContext, String> {
    #[cfg(not(any(target_os = "android", target_os = "linux")))]
    let app_handle = _app_handle;

    let mut context_lock = state.gpu_context.lock().unwrap();
    if let Some(context) = &*context_lock {
        return Ok(context.clone());
    }

    #[allow(unused_mut)]
    let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();

    #[cfg(target_os = "windows")]
    if std::env::var("WGPU_BACKEND").is_err() {
        instance_desc.backends = wgpu::Backends::PRIMARY;
    }

    let flag_path = state.gpu_crash_flag_path.lock().unwrap().clone();
    if let Some(p) = &flag_path {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(p, "initializing_gpu");
    }

    let instance = wgpu::Instance::new(instance_desc);

    #[cfg(not(any(target_os = "android", target_os = "linux")))]
    let surface_opt = {
        let settings = crate::app_settings::load_settings(app_handle.clone()).unwrap_or_default();
        let use_wgpu_renderer = settings.use_wgpu_renderer.unwrap_or(true);

        if use_wgpu_renderer {
            if let Some(window) = app_handle.get_webview_window("main") {
                match instance.create_surface(window) {
                    Ok(surface) => Some(surface),
                    Err(e) => {
                        log::warn!(
                            "Failed to create surface, falling back to compute-only: {}",
                            e
                        );
                        if let Some(p) = &flag_path {
                            let _ = std::fs::remove_file(p);
                        }
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    #[cfg(any(target_os = "android", target_os = "linux"))]
    let surface_opt: Option<wgpu::Surface> = None;

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: surface_opt.as_ref(),
        ..Default::default()
    }))
    .map_err(|e| {
        if let Some(p) = &flag_path {
            let _ = std::fs::remove_file(p);
        }
        format!("Failed to find a wgpu adapter: {}", e)
    })?;

    let mut required_features = wgpu::Features::empty();
    if adapter
        .features()
        .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    {
        required_features |= wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    }

    let limits = adapter.limits();

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Processing Device"),
        required_features,
        required_limits: limits.clone(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| {
        if let Some(p) = &flag_path {
            let _ = std::fs::remove_file(p);
        }
        e.to_string()
    })?;

    if let Some(p) = &flag_path {
        let _ = std::fs::remove_file(p);
    }

    #[cfg(not(any(target_os = "android", target_os = "linux")))]
    let display_opt = if let Some(surface) = surface_opt {
        let window = app_handle
            .get_webview_window("main")
            .ok_or("Failed to get main window")?;

        let swapchain_caps = surface.get_capabilities(&adapter);
        let swapchain_format = swapchain_caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(swapchain_caps.formats[0]);

        let alpha_mode = if cfg!(target_os = "windows")
            && swapchain_caps
                .alpha_modes
                .contains(&wgpu::CompositeAlphaMode::Opaque)
        {
            wgpu::CompositeAlphaMode::Opaque
        } else if swapchain_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else if swapchain_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
        {
            wgpu::CompositeAlphaMode::PostMultiplied
        } else {
            swapchain_caps.alpha_modes[0]
        };

        let size = window
            .inner_size()
            .unwrap_or(tauri::PhysicalSize::new(1280, 720));
        let config = wgpu::SurfaceConfiguration {
            width: size.width.max(1),
            height: size.height.max(1),
            format: swapchain_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Display Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/display.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Display BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    count: None,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    count: None,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    count: None,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Display Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Display Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: swapchain_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: NonZero::new(0),
            cache: None,
        });

        let transform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Transform Buffer"),
            size: std::mem::size_of::<DisplayTransform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Display Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Some(WgpuDisplay {
            surface,
            config,
            pipeline,
            bind_group_layout,
            transform_buffer,
            latest_transform: DisplayTransform {
                rect: [0.0, 0.0, 100.0, 100.0],
                clip: [0.0, 0.0, 10000.0, 10000.0],
                window: [1280.0, 720.0],
                image_size: [100.0, 100.0],
                texture_size: [100.0, 100.0],
                pixelated: 0.0,
                _pad: 0.0,
                bg_primary: [24.0 / 255.0, 24.0 / 255.0, 24.0 / 255.0, 1.0],
                bg_secondary: [35.0 / 255.0, 35.0 / 255.0, 35.0 / 255.0, 1.0],
            },
            sampler,
            current_bind_group: None,
        })
    } else {
        None
    };

    #[cfg(any(target_os = "android", target_os = "linux"))]
    let display_opt = None;

    let new_context = GpuContext {
        device: Arc::new(device),
        queue: Arc::new(queue),
        limits,
        display: Arc::new(std::sync::Mutex::new(display_opt)),
    };
    *context_lock = Some(new_context.clone());
    Ok(new_context)
}

pub fn process_and_get_dynamic_image(
    context: &GpuContext,
    state: &tauri::State<AppState>,
    base_image: &DynamicImage,
    transform_hash: u64,
    request: RenderRequest,
    caller_id: &str,
) -> Result<DynamicImage, String> {
    process_and_get_dynamic_image_inner(
        context,
        state,
        base_image,
        transform_hash,
        request,
        caller_id,
        false,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn process_and_get_dynamic_image_with_analytics(
    context: &GpuContext,
    state: &tauri::State<AppState>,
    base_image: &DynamicImage,
    transform_hash: u64,
    request: RenderRequest,
    caller_id: &str,
    output_to_display: bool,
    analytics_config: Option<crate::AnalyticsConfig>,
) -> Result<DynamicImage, String> {
    process_and_get_dynamic_image_inner(
        context,
        state,
        base_image,
        transform_hash,
        request,
        caller_id,
        output_to_display,
        analytics_config,
    )
}

#[allow(clippy::too_many_arguments)]
fn process_and_get_dynamic_image_inner(
    context: &GpuContext,
    state: &tauri::State<AppState>,
    base_image: &DynamicImage,
    transform_hash: u64,
    request: RenderRequest,
    caller_id: &str,
    output_to_display: bool,
    analytics_config: Option<crate::AnalyticsConfig>,
) -> Result<DynamicImage, String> {
    let start_time = Instant::now();
    let (width, height) = base_image.dimensions();
    let device = &context.device;
    let queue = &context.queue;

    let max_dim = context.limits.max_texture_dimension_2d;
    if width > max_dim || height > max_dim {
        log::warn!(
            "Image dimensions ({}x{}) exceed GPU limits ({}). Bypassing GPU processing and returning unprocessed image to prevent a crash. Try upgrading your GPU :)",
            width,
            height,
            max_dim
        );
        return Ok(base_image.clone());
    }

    let mut old_processor = None;
    let mut reallocated = false;

    let mut processor_lock = state.gpu_processor.lock().unwrap();
    if processor_lock.is_none()
        || processor_lock.as_ref().unwrap().width < width
        || processor_lock.as_ref().unwrap().height < height
    {
        let new_width = (width + 255) & !255;
        let new_height = (height + 255) & !255;
        log::info!(
            "Creating new GPU Processor for dimensions up to {}x{}",
            new_width,
            new_height
        );
        let new_processor = GpuProcessor::new(context.clone(), new_width, new_height)?;

        old_processor = processor_lock.take();

        *processor_lock = Some(crate::GpuProcessorState {
            processor: new_processor,
            width: new_width,
            height: new_height,
        });
        reallocated = true;
    }
    let processor_state = processor_lock.as_ref().unwrap();
    let processor = &processor_state.processor;

    if reallocated && let Some(old_state) = &old_processor {
        let mut encoder = device.create_command_encoder(&Default::default());
        let copy_w = old_state.width.min(processor_state.width);
        let copy_h = old_state.height.min(processor_state.height);

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &old_state.processor.output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &processor.output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: copy_w,
                height: copy_h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        if let Ok(mut display_lock) = context.display.lock()
            && let Some(display) = display_lock.as_mut()
        {
            display.latest_transform.texture_size =
                [processor_state.width as f32, processor_state.height as f32];
            queue.write_buffer(
                &display.transform_buffer,
                0,
                bytemuck::bytes_of(&display.latest_transform),
            );

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &display.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: display.transform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &processor.output_texture_view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&display.sampler),
                    },
                ],
                label: Some("Migrated Display Bind Group"),
            });
            display.current_bind_group = Some(bind_group);
        }
    }

    let mut cache_lock = state.gpu_image_cache.lock().unwrap();
    if let Some(cache) = &*cache_lock
        && (cache.transform_hash != transform_hash
            || cache.width != width
            || cache.height != height)
    {
        *cache_lock = None;
    }

    if cache_lock.is_none() {
        let img_rgba_f16 = to_rgba_f16(base_image);
        let texture_size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("Input Texture"),
                size: texture_size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            TextureDataOrder::MipMajor,
            bytemuck::cast_slice(&img_rgba_f16),
        );
        let texture_view = texture.create_view(&Default::default());

        *cache_lock = Some(GpuImageCache {
            texture,
            texture_view,
            width,
            height,
            transform_hash,
        });
    }

    let cache = cache_lock.as_ref().unwrap();

    let skip_readback = output_to_display;

    let (processed_pixels, out_w, out_h, out_x, out_y) = processor.run(
        &cache.texture_view,
        cache.width,
        cache.height,
        request,
        skip_readback,
        output_to_display,
    )?;

    let mut final_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Final Passes Encoder"),
    });
    let mut submit_final_encoder = false;

    let mut async_readback_buffer: Option<wgpu::Buffer> = None;
    let mut async_padded_bpr: u32 = 0;
    let mut async_unpadded_bpr: u32 = 0;

    if analytics_config.is_some() && skip_readback {
        let unpadded_bytes_per_row = 4 * out_w;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = (unpadded_bytes_per_row + align - 1) & !(align - 1);
        let output_buffer_size = (padded_bytes_per_row * out_h) as u64;

        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Async Analytics Readback Buffer"),
            size: output_buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        final_encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &processor.working_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: out_x,
                    y: out_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &output_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(out_h),
                },
            },
            wgpu::Extent3d {
                width: out_w,
                height: out_h,
                depth_or_array_layers: 1,
            },
        );

        async_readback_buffer = Some(output_buffer);
        async_padded_bpr = padded_bytes_per_row;
        async_unpadded_bpr = unpadded_bytes_per_row;
        submit_final_encoder = true;
    }

    if output_to_display {
        final_encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &processor.working_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: out_x,
                    y: out_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &processor.output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: out_x,
                    y: out_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: out_w,
                height: out_h,
                depth_or_array_layers: 1,
            },
        );
        submit_final_encoder = true;
    }

    if submit_final_encoder {
        queue.submit(Some(final_encoder.finish()));
    }

    if let Some(analytics) = analytics_config {
        if let Some(buffer) = async_readback_buffer {
            let output_buffer: wgpu::Buffer = buffer;
            let padded_bytes_per_row: u32 = async_padded_bpr;
            let unpadded_bytes_per_row: u32 = async_unpadded_bpr;
            let device_clone = context.device.clone();

            std::thread::spawn(move || {
                let buffer_slice = output_buffer.slice(..);
                let (tx, rx) = std::sync::mpsc::channel::<Result<(), wgpu::BufferAsyncError>>();

                buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });

                if let Err(e) = device_clone.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(60)),
                }) {
                    log::error!("Async analytics readback poll failed: {}", e);
                    return;
                }

                if let Ok(Ok(())) = rx.recv() {
                    let padded_data = buffer_slice.get_mapped_range().to_vec();
                    output_buffer.unmap();

                    let mut unpadded_data =
                        Vec::with_capacity((unpadded_bytes_per_row * out_h) as usize);
                    if padded_bytes_per_row == unpadded_bytes_per_row {
                        unpadded_data = padded_data;
                    } else {
                        for chunk in padded_data.chunks(padded_bytes_per_row as usize) {
                            unpadded_data
                                .extend_from_slice(&chunk[..unpadded_bytes_per_row as usize]);
                        }
                    }

                    if let Some(img_buf) =
                        ImageBuffer::<Rgba<u8>, _>::from_raw(out_w, out_h, unpadded_data)
                    {
                        let dynamic_img = DynamicImage::ImageRgba8(img_buf);
                        let _ = analytics.sender.send(crate::AnalyticsJob {
                            path: analytics.path,
                            image: std::sync::Arc::new(dynamic_img),
                            compute_waveform: analytics.compute_waveform,
                            active_waveform_channel: analytics.active_waveform_channel,
                        });
                    }
                }
            });
        } else {
            let pixels_clone = processed_pixels.clone();
            std::thread::spawn(move || {
                if let Some(img_buf) =
                    ImageBuffer::<Rgba<u8>, _>::from_raw(out_w, out_h, pixels_clone)
                {
                    let dynamic_img = DynamicImage::ImageRgba8(img_buf);
                    let _ = analytics.sender.send(crate::AnalyticsJob {
                        path: analytics.path,
                        image: std::sync::Arc::new(dynamic_img),
                        compute_waveform: analytics.compute_waveform,
                        active_waveform_channel: analytics.active_waveform_channel,
                    });
                }
            });
        }
    }

    if output_to_display
        && let Ok(mut display_lock) = context.display.lock()
        && let Some(display) = display_lock.as_mut()
    {
        display.latest_transform.image_size = [width as f32, height as f32];
        display.latest_transform.texture_size =
            [processor_state.width as f32, processor_state.height as f32];

        queue.write_buffer(
            &display.transform_buffer,
            0,
            bytemuck::bytes_of(&display.latest_transform),
        );

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &display.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: display.transform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&processor.output_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&display.sampler),
                },
            ],
            label: None,
        });
        display.current_bind_group = Some(bind_group);
        display.render(device, queue);
    }

    drop(old_processor);

    if skip_readback {
        let duration = start_time.elapsed();
        let fps = 1.0 / duration.as_secs_f64();
        log::info!(
            "[{}] {}x{} native WGPU display updated in {:?} ({:.2} FPS)",
            caller_id,
            width,
            height,
            duration,
            fps
        );
        return Ok(DynamicImage::new_rgba8(0, 0));
    }

    let duration = start_time.elapsed();
    let fps = 1.0 / duration.as_secs_f64();
    log::info!(
        "[{}] {}x{} processed (ROI: {}x{}) on GPU in {:?} ({:.2} FPS)",
        caller_id,
        width,
        height,
        out_w,
        out_h,
        duration,
        fps
    );

    let img_buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(out_w, out_h, processed_pixels)
        .ok_or("Failed to create image buffer from GPU data")?;
    Ok(DynamicImage::ImageRgba8(img_buf))
}
