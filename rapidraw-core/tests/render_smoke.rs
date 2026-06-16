use image::{DynamicImage, RgbImage};
use rapidraw_core::image_processing::AllAdjustments;
use rapidraw_core::{headless_context, render};

fn mean_luma(img: &DynamicImage) -> f32 {
    let rgb = img.to_rgb8();
    let mut sum = 0.0f64;
    for p in rgb.pixels() {
        sum += 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
    }
    (sum / (rgb.width() as f64 * rgb.height() as f64)) as f32
}

#[test]
fn exposure_bump_brightens() {
    let Ok(ctx) = headless_context() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(256, 256, image::Rgb([100, 100, 100])));

    let mut adj = AllAdjustments::default();
    adj.global.exposure = 1.0; // +1 stop

    let out = render(&ctx, &base, &adj, None).expect("render ok");
    assert!(
        mean_luma(&out) > mean_luma(&base) + 5.0,
        "expected brighter output, base={} out={}",
        mean_luma(&base),
        mean_luma(&out)
    );
}
