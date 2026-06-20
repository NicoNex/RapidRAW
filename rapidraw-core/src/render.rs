use std::sync::Arc;

use image::{DynamicImage, GenericImageView, GrayImage, RgbaImage};
use wgpu::util::{DeviceExt, TextureDataOrder};

use crate::gpu_processing::{to_rgba_f16, GpuProcessor, RenderRequest};
use crate::image_processing::{AllAdjustments, GpuContext};
use crate::lut_processing::Lut;
use crate::mask_generation::{generate_mask_bitmap, MaskDefinition};

/// Render `base` through the GPU pipeline with `adj`. Optionally apply a 3D
/// `lut` (its strength is `adj.global.lut_intensity`, 0.0..1.0) and optionally
/// downscale the longest edge to `max_dim` first (for fast previews). Returns
/// an RGBA8 image.
pub fn render(
    ctx: &GpuContext,
    base: &DynamicImage,
    adj: &AllAdjustments,
    masks: &[MaskDefinition],
    lut: Option<Arc<Lut>>,
    max_dim: Option<u32>,
    ai_resolver: Option<crate::mask_generation::AiResolver>,
) -> Result<DynamicImage, String> {
    // Capture full dimensions before any downscale.
    let base_full_dims = base.dimensions();

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

    // Compute scale for mask rasterization (render size / full size). The
    // preview downscale is aspect-preserving (resize longest edge), so X and Y
    // scale are equal — a single uniform `scale` is correct.
    let scale = if base_full_dims.0 > 0 {
        width as f32 / base_full_dims.0 as f32
    } else {
        1.0
    };

    // AllAdjustments is Pod (Copy); flag whether a LUT is bound so the shader
    // applies it (`has_lut`/`lut_intensity` gate the mix in shader.wgsl).
    let mut adj = *adj;
    let mut mask_bitmaps: Vec<GrayImage> = Vec::new();
    let mut layer = 0usize;
    for m in masks.iter().take(crate::image_processing::MAX_MASKS) {
        // ponytail: `base` here is pre-color-grading AND preview-downscaled, so
        // color/luminance masks sample the unadjusted image and compare their
        // full-res target_x/target_y against downscaled dimensions. Acceptable
        // for the foundation pass (relm4 masks are empty today); revisit both
        // when color/luminance mask wiring lands.
        if let Some(bmp) =
            generate_mask_bitmap(m, width, height, scale, (0.0, 0.0), Some(&base), ai_resolver)
        {
            adj.mask_adjustments[layer] =
                crate::image_processing::get_mask_adjustments_from_json(&m.adjustments);
            mask_bitmaps.push(bmp);
            layer += 1;
        }
    }
    adj.mask_count = mask_bitmaps.len() as u32;
    adj.global.has_lut = if lut.is_some() { 1 } else { 0 };

    let request = RenderRequest {
        adjustments: adj,
        mask_bitmaps: &mask_bitmaps,
        lut,
        roi: None,
    };

    // Reuse a per-thread GpuProcessor keyed by size: building one compiles the
    // (large) shader, so doing it every frame makes slider drags choppy. Drive
    // renders from a single persistent worker thread and the processor is built
    // once per image size, not per frame.
    let (pixels, out_w, out_h) = PROCESSOR_CACHE.with(|cache| {
        let mut slot = cache.borrow_mut();
        let reuse = matches!(&*slot, Some((w, h, _)) if *w == width && *h == height);
        if !reuse {
            *slot = Some((width, height, GpuProcessor::new(ctx.clone(), width, height)?));
        }
        let (_, _, processor) = slot.as_ref().unwrap();
        let (pixels, out_w, out_h, _x, _y) =
            processor.run(&input_view, width, height, request, false, false)?;
        Ok::<_, String>((pixels, out_w, out_h))
    })?;

    let img = RgbaImage::from_raw(out_w, out_h, pixels)
        .ok_or_else(|| "readback buffer size mismatch".to_string())?;
    Ok(DynamicImage::ImageRgba8(img))
}

thread_local! {
    /// Per-thread cached processor `(width, height, processor)`; rebuilt only
    /// when the render size changes.
    static PROCESSOR_CACHE: std::cell::RefCell<Option<(u32, u32, GpuProcessor)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_processing::AllAdjustments;
    use crate::mask_generation::{MaskDefinition, SubMask, SubMaskMode};
    use image::{DynamicImage, RgbaImage};
    use serde_json::json;

    #[test]
    fn mask_with_exposure_changes_only_masked_region() {
        let ctx = match crate::headless_context() {
            Ok(c) => c,
            Err(_) => {
                eprintln!("no GPU; skipping mask render test");
                return;
            }
        };
        let base = DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, image::Rgba([128, 128, 128, 255])));
        let adj = AllAdjustments::default();
        let mask = MaskDefinition {
            id: "m".into(), name: "m".into(), visible: true, invert: false, opacity: 100.0,
            adjustments: json!({ "exposure": 100.0 }),
            sub_masks: vec![SubMask {
                id: "s".into(), mask_type: "radial".into(), visible: true, invert: false,
                opacity: 100.0, mode: SubMaskMode::Additive,
                parameters: json!({ "centerX": 32.0, "centerY": 32.0, "radiusX": 16.0, "radiusY": 16.0, "rotation": 0.0, "feather": 0.2 }),
            }],
        };
        let out = render(&ctx, &base, &adj, std::slice::from_ref(&mask), None, None)
            .unwrap()
            .to_rgba8();
        let center = out.get_pixel(32, 32)[0];
        let corner = out.get_pixel(1, 1)[0];
        assert!(center > corner + 5, "masked center ({center}) should be brighter than corner ({corner})");
    }

    #[test]
    fn skipped_first_mask_does_not_misalign_adjustments() {
        let ctx = match crate::headless_context() {
            Ok(c) => c,
            Err(_) => { eprintln!("no GPU; skipping"); return; }
        };
        let base = DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, image::Rgba([128, 128, 128, 255])));
        let adj = AllAdjustments::default();
        // First mask: invisible -> generate_mask_bitmap returns None (no bitmap, no layer)
        let hidden = MaskDefinition {
            id: "hidden".into(), name: "hidden".into(), visible: false, invert: false, opacity: 100.0,
            adjustments: json!({ "exposure": -100.0 }),
            sub_masks: vec![SubMask {
                id: "h1".into(), mask_type: "radial".into(), visible: true, invert: false,
                opacity: 100.0, mode: SubMaskMode::Additive,
                parameters: json!({ "centerX": 32.0, "centerY": 32.0, "radiusX": 16.0, "radiusY": 16.0, "rotation": 0.0, "feather": 0.2 }),
            }],
        };
        // Second mask: visible radial, exposure boost. Must end up at atlas layer 0 WITH its own adjustments.
        let visible = MaskDefinition {
            id: "vis".into(), name: "vis".into(), visible: true, invert: false, opacity: 100.0,
            adjustments: json!({ "exposure": 100.0 }),
            sub_masks: vec![SubMask {
                id: "v1".into(), mask_type: "radial".into(), visible: true, invert: false,
                opacity: 100.0, mode: SubMaskMode::Additive,
                parameters: json!({ "centerX": 32.0, "centerY": 32.0, "radiusX": 16.0, "radiusY": 16.0, "rotation": 0.0, "feather": 0.2 }),
            }],
        };
        let masks = [hidden, visible];
        let out = render(&ctx, &base, &adj, &masks, None, None).unwrap().to_rgba8();
        let center = out.get_pixel(32, 32)[0];
        let corner = out.get_pixel(1, 1)[0];
        assert!(center > corner + 5, "second mask's exposure must apply at layer 0 (center {center} vs corner {corner})");
    }
}
