//! Auto tone curve: match the neutral RAW render to the camera's embedded JPEG
//! preview via luminance histogram specification, reproducing the in-camera
//! look (à la RawTherapee's auto-matched tone curve,
//! `rtengine/histmatching.cc`). Produces `luma_curve` control points in 0..255.
//!
//! The matching (luma CDF specification) is RT's; the adaptive point reducer
//! (`mapping_to_curve`) is ported from RT 1:1 for the same fidelity. The only
//! deliberate change is the mapping direction: we build the *forward* map
//! `input(source) -> output(target)` directly, so the curve axes are
//! unambiguous, then run RT's reducer over it. (A greedy max-error fit was
//! tried for denser placement but chased the noisy CDF inverse into visible
//! artifacts; RT's reducer smooths over per-level noise and is more faithful.)

use image::DynamicImage;

const N: usize = 256;

struct Cdf {
    /// Normalised cumulative luminance distribution (0..1). Normalised so source
    /// and target may differ in pixel count (RT instead resizes them equal).
    cdf: Vec<f64>,
    min_val: i32,
    max_val: i32,
}

/// Linear scene-referred (0..1) -> sRGB display (0..1). The engine applies the
/// luma curve in sRGB display space (`apply_all_curves(base_srgb, ...)`), so a
/// linear source must be encoded to match the curve's domain and the (already
/// sRGB) embedded-JPEG target. Without this the match is built in the wrong
/// space and crushes the image.
fn srgb_encode(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Float `DynamicImage` variants hold linear scene-referred data in this engine;
/// integer variants are already display sRGB.
fn is_linear(img: &DynamicImage) -> bool {
    matches!(
        img,
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_)
    )
}

fn get_cdf(img: &DynamicImage) -> Cdf {
    let linear = is_linear(img);
    let rgb = img.to_rgb32f();
    let mut hist = vec![0u32; N];
    for p in rgb.pixels() {
        let (r, g, b) = if linear {
            (srgb_encode(p[0]), srgb_encode(p[1]), srgb_encode(p[2]))
        } else {
            (p[0], p[1], p[2])
        };
        // Rec.709 luma, matching the engine shader's get_luma coefficients.
        let l = (0.2126 * r + 0.7152 * g + 0.0722 * b) * 255.0;
        hist[(l.round().clamp(0.0, (N - 1) as f32) as usize).min(N - 1)] += 1;
    }
    let mut min_val = -1i32;
    let mut max_val = -1i32;
    for (i, &c) in hist.iter().enumerate() {
        if c > 0 {
            if min_val < 0 {
                min_val = i as i32;
            }
            max_val = i as i32;
        }
    }
    let total: u64 = hist.iter().map(|&c| c as u64).sum();
    let inv = if total > 0 { 1.0 / total as f64 } else { 0.0 };
    let mut cdf = vec![0.0; N];
    let mut sum = 0u64;
    for i in 0..N {
        sum += hist[i] as u64;
        cdf[i] = sum as f64 * inv;
    }
    Cdf { cdf, min_val, max_val }
}

/// Smallest target level `t` with `tcdf[t] >= p` (inverse target CDF).
fn invert(tcdf: &[f64], p: f64) -> i32 {
    for (t, &c) in tcdf.iter().enumerate() {
        if c >= p {
            return t as i32;
        }
    }
    (N - 1) as i32
}

fn coord(v: i32) -> f64 {
    v as f64 / (N as f64 - 1.0)
}

/// One pass of RT's reducer: walk `mapping[start..stop]` emitting a point each
/// time the value changes ~`step` apart, or after `maxdelta` with no point.
#[allow(clippy::too_many_arguments)]
fn doit(
    curve: &mut Vec<f64>,
    mapping: &[i32],
    start: i32,
    stop: i32,
    step: i32,
    addstart: bool,
    maxdelta_in: i32,
) {
    let maxdelta = if maxdelta_in == 0 { step * 2 } else { maxdelta_in };
    let mut prev = start;
    if addstart && mapping[start as usize] >= 0 {
        curve.push(coord(start));
        curve.push(coord(mapping[start as usize]));
    }
    let mut i = start;
    while i < stop {
        let v = mapping[i as usize];
        if v < 0 {
            i += 1;
            continue;
        }
        let change = i > 0 && v != mapping[(i - 1) as usize];
        let diff = i - prev;
        if (change && (diff - step).abs() <= 1) || diff > maxdelta {
            curve.push(coord(i));
            curve.push(coord(v));
            prev = i;
        }
        i += 1;
    }
}

/// RT's adaptive point reducer (`mappingToCurve`), ported. `mapping[s]` is the
/// output level for input level `s`, or -1 where undefined. Returns control
/// points in 0..255 (x=input, y=output), pinned at (0,0) and (255,255).
fn mapping_to_curve(mapping: &[i32]) -> Vec<(f32, f32)> {
    let n = mapping.len() as i32;
    // First index where the mapping reaches/exceeds identity (curve crosses y=x).
    let mut idx = 15;
    while idx < n && mapping[idx as usize] < idx {
        idx += 1;
    }
    if idx == n {
        idx = 1;
        while idx < n - 1 && mapping[idx as usize] < idx {
            idx += 1;
        }
    }

    let mut curve: Vec<f64> = Vec::new();
    curve.push(0.0);
    curve.push(0.0);

    let mut start = 0i32;
    while start < idx && (mapping[start as usize] < 0 || start < idx / 2) {
        start += 1;
    }

    let npoints = 8i32;
    let mut step = (n / npoints).max(1);
    let end = n;
    if idx <= end / 3 {
        doit(&mut curve, mapping, start, idx, idx / 2, true, 0);
        step = (end - idx) / 4;
        doit(&mut curve, mapping, idx, end, step, false, step);
    } else {
        let s = if idx > step { step } else { idx / 2 };
        doit(&mut curve, mapping, start, idx, s, true, 0);
        let addstart =
            idx - step > step / 2 && (curve[curve.len() - 2] - coord(idx)).abs() > 0.01;
        doit(&mut curve, mapping, idx, end, step, addstart, 0);
    }

    if curve.len() > 2 && (1.0 - curve[curve.len() - 2] <= coord(step) / 3.0) {
        curve.pop();
        curve.pop();
    }
    curve.push(1.0);
    curve.push(1.0);

    let mut pts: Vec<(f32, f32)> = curve
        .chunks_exact(2)
        .map(|c| ((c[0] * 255.0) as f32, (c[1] * 255.0) as f32))
        .collect();
    pts.truncate(16);
    pts
}

/// Auto tone curve matching `source` (neutral render) to `target` (camera
/// preview) by luminance histogram specification + RT's adaptive reducer.
/// Returns `luma_curve` control points in 0..255 (x=input, y=output); empty if
/// either image has no tonal range.
pub fn auto_tone_curve(source: &DynamicImage, target: &DynamicImage) -> Vec<(f32, f32)> {
    let scdf = get_cdf(source);
    let tcdf = get_cdf(target);
    if scdf.min_val < 0 || tcdf.min_val < 0 {
        return Vec::new();
    }
    // forward[s] = T(s): the target level whose CDF rank matches source level s.
    // -1 outside the source's populated range (RT leaves these undefined).
    let mut forward = vec![-1i32; N];
    for s in scdf.min_val..=scdf.max_val {
        forward[s as usize] = invert(&tcdf.cdf, scdf.cdf[s as usize]);
    }
    mapping_to_curve(&forward)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    /// Horizontal luma ramp 0..255, shifted by `bias` (clamped).
    fn ramp(bias: i32) -> DynamicImage {
        let mut img = RgbImage::new(256, 16);
        for (x, _y, px) in img.enumerate_pixels_mut() {
            let v = (x as i32 + bias).clamp(0, 255) as u8;
            *px = Rgb([v, v, v]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn identity_when_source_equals_target() {
        let g = ramp(0);
        let pts = auto_tone_curve(&g, &g);
        assert!(pts.len() >= 2);
        for &(x, y) in &pts {
            assert!((x - y).abs() <= 6.0, "point ({x},{y}) far from identity");
        }
    }

    #[test]
    fn brighter_target_lifts_curve() {
        let pts = auto_tone_curve(&ramp(0), &ramp(40));
        assert!(
            pts.iter()
                .any(|&(x, y)| (20.0..235.0).contains(&x) && y > x + 5.0),
            "expected a lifted midtone, got {pts:?}"
        );
    }

    #[test]
    fn monotone_and_in_range() {
        let pts = auto_tone_curve(&ramp(0), &ramp(-30));
        for w in pts.windows(2) {
            assert!(w[1].0 >= w[0].0, "x must be non-decreasing: {pts:?}");
        }
        for &(x, y) in &pts {
            assert!((0.0..=255.0).contains(&x) && (0.0..=255.0).contains(&y));
        }
    }
}
