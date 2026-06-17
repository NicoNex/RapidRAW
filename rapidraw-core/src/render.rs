use std::sync::Arc;

use image::{DynamicImage, GenericImageView, RgbaImage};
use wgpu::util::{DeviceExt, TextureDataOrder};

use crate::gpu_processing::{to_rgba_f16, GpuProcessor, RenderRequest};
use crate::image_processing::{AllAdjustments, GpuContext};
use crate::lut_processing::Lut;

/// Render `base` through the GPU pipeline with `adj`. Optionally apply a 3D
/// `lut` (its strength is `adj.global.lut_intensity`, 0.0..1.0) and optionally
/// downscale the longest edge to `max_dim` first (for fast previews). Returns
/// an RGBA8 image.
pub fn render(
    ctx: &GpuContext,
    base: &DynamicImage,
    adj: &AllAdjustments,
    lut: Option<Arc<Lut>>,
    max_dim: Option<u32>,
) -> Result<DynamicImage, String> {
    // Optional downscale for preview.
    let base = match max_dim {
        Some(m) => {
            let (w, h) = base.dimensions();
            if w.max(h) > m {
                base.resize(m, m, image::imageops::FilterType::Triangle)
            } else {
                base.clone()
            }
        }
        None => base.clone(),
    };

    let (width, height) = base.dimensions();
    let max_tex = ctx.limits.max_texture_dimension_2d;
    if width > max_tex || height > max_tex {
        return Ok(base); // engine policy: bypass when over GPU limits
    }

    let device = &ctx.device;
    let queue = &ctx.queue;

    // Upload base as Rgba16Float input texture (mirrors gpu_processing.rs:1745-1769).
    let f16 = to_rgba_f16(&base);
    let input_texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Core Input Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        TextureDataOrder::MipMajor,
        bytemuck::cast_slice(&f16),
    );
    let input_view = input_texture.create_view(&Default::default());

    // AllAdjustments is Pod (Copy); flag whether a LUT is bound so the shader
    // applies it (`has_lut`/`lut_intensity` gate the mix in shader.wgsl).
    let mut adj = *adj;
    adj.global.has_lut = if lut.is_some() { 1 } else { 0 };

    let processor = GpuProcessor::new(ctx.clone(), width, height)?;
    let request = RenderRequest {
        adjustments: adj,
        mask_bitmaps: &[],
        lut,
        roi: None,
    };

    let (pixels, out_w, out_h, _x, _y) =
        processor.run(&input_view, width, height, request, false, false)?;

    let img = RgbaImage::from_raw(out_w, out_h, pixels)
        .ok_or_else(|| "readback buffer size mismatch".to_string())?;
    Ok(DynamicImage::ImageRgba8(img))
}
