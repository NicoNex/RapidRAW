# GTK Masks Foundation (P0 + P1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move non-AI mask rasterization into `rapidraw-core` (P0) and wire the relm4 render path to apply masks via the existing GPU atlas (P1), with no change to Tauri/React behaviour.

**Architecture:** The GPU engine, atlas upload, per-mask shader, and `get_mask_adjustments_from_json` already live in `rapidraw-core`. We relocate the non-AI mask data model + rasterizers from `src-tauri/src/mask_generation.rs` into a new `rapidraw-core::mask_generation` module. AI sub-mask types are handled via an injected resolver closure so core never references `ai_processing`; `src-tauri` keeps a same-signature wrapper that injects its AI resolver, leaving every existing Tauri caller untouched. Then `rapidraw_core::render` gains a `masks` argument, rasterizes them at render resolution, and passes the bitmaps + `mask_count` + per-mask `MaskAdjustments` to the processor.

**Tech Stack:** Rust, `image`/`imageproc`/`rayon` (already core deps), wgpu, relm4. Scope is non-AI only; AI masks are a later plan.

**Reference (read before starting):**
- Spec: `docs/superpowers/specs/2026-06-19-gtk-masks-design.md`
- Source data model + rasterizers: `src-tauri/src/mask_generation.rs` (esp. lines 18–229 types, 539–1318 rasterizers + dispatch)
- Core engine seam: `rapidraw-core/src/render.rs`, `rapidraw-core/src/gpu_processing.rs:798` (`run`, `RenderRequest.mask_bitmaps`), `rapidraw-core/src/image_processing.rs:2165` (`get_mask_adjustments_from_json`)
- relm4 render thread: `rapidraw-relm4/src/main.rs:573-628`

---

## File Structure

- **Create** `rapidraw-core/src/mask_generation.rs` — non-AI mask data model + rasterizers + compositing, with an optional AI-resolver hook. One responsibility: turn a `MaskDefinition` into a grayscale bitmap.
- **Modify** `rapidraw-core/src/lib.rs` — declare `pub mod mask_generation;`.
- **Modify** `rapidraw-core/src/render.rs` — `render()` accepts `masks: &[MaskDefinition]`, rasterizes, threads bitmaps + adjustments into `RenderRequest`.
- **Modify** `src-tauri/src/mask_generation.rs` — delete relocated code; re-export core types; keep AI rasterizers + AI resolver + a same-signature `generate_mask_bitmap` wrapper + `generate_mask_overlay`.
- **Modify** `rapidraw-relm4/src/main.rs` — `RenderJob` carries masks; render thread passes them to `render()`.
- **Modify** `rapidraw-relm4/src/state.rs` — session gains `masks: Vec<MaskDefinition>`.

---

# Phase P0 — Move non-AI rasterization into core

### Task 1: Create the core mask_generation module (types + radial only)

Start with the data model and one rasterizer to establish the module and prove the build, then move the rest.

**Files:**
- Create: `rapidraw-core/src/mask_generation.rs`
- Modify: `rapidraw-core/src/lib.rs:8` (add module declaration after `pub mod image_loader;`)

- [ ] **Step 1: Declare the module**

In `rapidraw-core/src/lib.rs`, after line `pub mod image_loader;`:

```rust
pub mod mask_generation;
```

- [ ] **Step 2: Create the module with types + radial rasterizer + AI resolver type**

Create `rapidraw-core/src/mask_generation.rs`. Copy the type definitions verbatim from `src-tauri/src/mask_generation.rs` lines 18–229 EXCEPT drop the `use crate::ai_processing::...` and `use crate::get_cached_full_warped_image;` imports and the `ParametricMaskParameters` (AI-only) block. Add the resolver type and the radial rasterizer:

```rust
use image::{DynamicImage, GenericImageView, GrayImage, Luma};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::f32::consts::PI;

/// Resolver for AI sub-mask types, injected by callers that have the AI
/// subsystem (src-tauri). Core never depends on `ai_processing`; for `ai-*`
/// and unknown sub-mask types core defers to this closure when provided.
pub type AiResolver<'a> =
    &'a dyn Fn(&SubMask, u32, u32, f32, (f32, f32)) -> Option<GrayImage>;

// --- paste SubMaskMode, SubMask, default_opacity, MaskDefinition (+ requires_warped_image),
//     GrowFeatherParameters, RadialMaskParameters, LinearMaskParameters (+ default_range, Default),
//     Point, BrushLine (+ default_brush_feather), BrushMaskParameters, FlowLine (+ default_line_flow),
//     FlowMaskParameters here, verbatim from src-tauri lines 27..188 (skip the AI use lines). ---

fn generate_radial_bitmap(
    params_value: &Value,
    width: u32,
    height: u32,
    scale: f32,
    crop_offset: (f32, f32),
) -> GrayImage {
    // paste body verbatim from src-tauri/src/mask_generation.rs:546-580
    unimplemented!("paste verbatim")
}
```

Note: keep `#[serde(crate = "serde")]` attributes only if `src-tauri` used them for a vendored serde; core uses the normal `serde` crate, so DROP the `#[serde(crate = "serde")]` lines when pasting into core (the `#[serde(rename_all = ...)]` and `#[serde(rename = "type")]` / `#[serde(default...)]` attributes stay).

- [ ] **Step 3: Verify it compiles**

Run: `cargo check --manifest-path rapidraw-core/Cargo.toml`
Expected: `Finished` with no errors (warnings about unused fns are fine for now).

- [ ] **Step 4: Commit**

```bash
git add rapidraw-core/src/lib.rs rapidraw-core/src/mask_generation.rs
git commit -m "feat(core): add mask_generation module with data model + radial rasterizer"
```

### Task 2: Move the remaining non-AI rasterizers + helpers into core

**Files:**
- Modify: `rapidraw-core/src/mask_generation.rs`

- [ ] **Step 1: Paste the remaining rasterizers + helpers verbatim**

From `src-tauri/src/mask_generation.rs`, copy these items verbatim into the core module (bodies unchanged):
- `grayscale_dilate` (231), `grayscale_erode` (272), `apply_grow_and_feather` (313)
- `stroke_bounds` (341), `render_stroke_layer_parallel` (386)
- `generate_linear_bitmap` (583), `generate_brush_bitmap` (637), `generate_flow_bitmap` (704)
- `generate_color_bitmap` (1040), `generate_luminance_bitmap` (1141), `generate_all_bitmap` (1241)

These use only `image`, `imageproc`, `rayon`, `serde_json`, `std` — all available in core. If any references `imageproc`, add `use imageproc::...` as in the source.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check --manifest-path rapidraw-core/Cargo.toml`
Expected: `Finished`. Fix any missing `use` lines flagged by the compiler.

- [ ] **Step 3: Commit**

```bash
git add rapidraw-core/src/mask_generation.rs
git commit -m "feat(core): move non-AI mask rasterizers into core"
```

### Task 3: Add core dispatch + compositing with the AI resolver hook

**Files:**
- Modify: `rapidraw-core/src/mask_generation.rs`

- [ ] **Step 1: Add `generate_sub_mask_bitmap` (non-AI dispatch + resolver fallback)**

```rust
fn generate_sub_mask_bitmap(
    sub_mask: &SubMask,
    width: u32,
    height: u32,
    scale: f32,
    crop_offset: (f32, f32),
    warped_image: Option<&DynamicImage>,
    ai: Option<AiResolver>,
) -> Option<GrayImage> {
    if !sub_mask.visible {
        return None;
    }
    match sub_mask.mask_type.as_str() {
        "radial" => Some(generate_radial_bitmap(&sub_mask.parameters, width, height, scale, crop_offset)),
        "linear" => Some(generate_linear_bitmap(&sub_mask.parameters, width, height, scale, crop_offset)),
        "brush" => Some(generate_brush_bitmap(&sub_mask.parameters, width, height, scale, crop_offset)),
        "flow" => Some(generate_flow_bitmap(&sub_mask.parameters, width, height, scale, crop_offset)),
        "color" => generate_color_bitmap(&sub_mask.parameters, width, height, scale, crop_offset, warped_image),
        "luminance" => generate_luminance_bitmap(&sub_mask.parameters, width, height, scale, crop_offset, warped_image),
        "all" => Some(generate_all_bitmap(width, height)),
        // ai-subject / ai-foreground / ai-sky / ai-depth / quick-eraser / unknown
        _ => ai.and_then(|f| f(sub_mask, width, height, scale, crop_offset)),
    }
}
```

- [ ] **Step 2: Add `generate_mask_bitmap` (public, compositing) with the resolver param**

Copy the body verbatim from `src-tauri/src/mask_generation.rs:1320-1390`, but change the signature to add `ai: Option<AiResolver>` and forward it in the `generate_sub_mask_bitmap` call:

```rust
pub fn generate_mask_bitmap(
    mask_def: &MaskDefinition,
    width: u32,
    height: u32,
    scale: f32,
    crop_offset: (f32, f32),
    warped_image: Option<&DynamicImage>,
    ai: Option<AiResolver>,
) -> Option<GrayImage> {
    if !mask_def.visible || mask_def.sub_masks.is_empty() {
        return None;
    }
    let mut final_mask = GrayImage::new(width, height);
    for sub_mask in &mask_def.sub_masks {
        if let Some(mut sub_bitmap) =
            generate_sub_mask_bitmap(sub_mask, width, height, scale, crop_offset, warped_image, ai)
        {
            // ... rest verbatim from src-tauri:1336-1389 (invert, opacity, mode match, final invert/opacity) ...
        }
    }
    Some(final_mask)
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check --manifest-path rapidraw-core/Cargo.toml`
Expected: `Finished`.

- [ ] **Step 4: Add a rasterizer self-check test**

Add to the bottom of `rapidraw-core/src/mask_generation.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn radial_mask() -> MaskDefinition {
        MaskDefinition {
            id: "m1".into(),
            name: "Radial".into(),
            visible: true,
            invert: false,
            opacity: 100.0,
            adjustments: json!({}),
            sub_masks: vec![SubMask {
                id: "s1".into(),
                mask_type: "radial".into(),
                visible: true,
                invert: false,
                opacity: 100.0,
                mode: SubMaskMode::Additive,
                parameters: json!({
                    "centerX": 50.0, "centerY": 50.0,
                    "radiusX": 40.0, "radiusY": 40.0,
                    "rotation": 0.0, "feather": 0.5
                }),
            }],
        }
    }

    #[test]
    fn radial_is_bright_center_dark_corner() {
        let bmp = generate_mask_bitmap(&radial_mask(), 100, 100, 1.0, (0.0, 0.0), None, None).unwrap();
        assert_eq!(bmp.get_pixel(50, 50)[0], 255, "center should be fully masked");
        assert_eq!(bmp.get_pixel(0, 0)[0], 0, "far corner should be unmasked");
    }

    #[test]
    fn invisible_mask_returns_none() {
        let mut m = radial_mask();
        m.visible = false;
        assert!(generate_mask_bitmap(&m, 100, 100, 1.0, (0.0, 0.0), None, None).is_none());
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --manifest-path rapidraw-core/Cargo.toml mask_generation`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add rapidraw-core/src/mask_generation.rs
git commit -m "feat(core): mask compositing + dispatch with AI resolver hook + tests"
```

### Task 4: Reduce src-tauri/mask_generation.rs to AI + re-exports + wrapper

**Files:**
- Modify: `src-tauri/src/mask_generation.rs`

- [ ] **Step 1: Re-export moved types from core; delete their local definitions**

At the top of `src-tauri/src/mask_generation.rs`, replace the moved type definitions (lines 18–188, the non-AI types) and the moved rasterizer functions (the ones listed in Tasks 2–3) with:

```rust
pub use rapidraw_core::mask_generation::{
    BrushMaskParameters, FlowMaskParameters, GrowFeatherParameters, LinearMaskParameters,
    MaskDefinition, RadialMaskParameters, SubMask, SubMaskMode,
};
```

Keep in this file: `AiPatchDefinition`, `PatchData`, `ParametricMaskParameters`, all `generate_ai_*` fns, `generate_ai_bitmap_from_full_mask`, `generate_ai_bitmap_from_base64`, `TransformParams` (and anything they use), `generate_mask_overlay`, and the `use crate::ai_processing::...` / `use crate::get_cached_full_warped_image;` imports.

- [ ] **Step 2: Add the AI resolver + a same-signature `generate_mask_bitmap` wrapper**

```rust
fn ai_sub_mask_resolver(
    sub: &SubMask,
    width: u32,
    height: u32,
    scale: f32,
    crop_offset: (f32, f32),
) -> Option<image::GrayImage> {
    match sub.mask_type.as_str() {
        "ai-subject" | "quick-eraser" => {
            generate_ai_subject_bitmap(&sub.parameters, width, height, scale, crop_offset)
        }
        "ai-foreground" => {
            generate_ai_foreground_bitmap(&sub.parameters, width, height, scale, crop_offset)
        }
        "ai-sky" => generate_ai_sky_bitmap(&sub.parameters, width, height, scale, crop_offset),
        "ai-depth" => generate_ai_depth_bitmap(&sub.parameters, width, height, scale, crop_offset),
        _ => None,
    }
}

/// Same signature the rest of src-tauri already calls; injects the AI resolver
/// so AI sub-masks keep working while non-AI rasterization lives in core.
pub fn generate_mask_bitmap(
    mask_def: &MaskDefinition,
    width: u32,
    height: u32,
    scale: f32,
    crop_offset: (f32, f32),
    warped_image: Option<&image::DynamicImage>,
) -> Option<image::GrayImage> {
    rapidraw_core::mask_generation::generate_mask_bitmap(
        mask_def,
        width,
        height,
        scale,
        crop_offset,
        warped_image,
        Some(&ai_sub_mask_resolver),
    )
}
```

- [ ] **Step 3: Verify the whole Tauri build compiles**

Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: `Finished`. Fix any leftover references to deleted private items (e.g. AI fns that called the moved `generate_*_bitmap` directly should call them via `rapidraw_core::mask_generation::` — but per the dispatch they shouldn't; resolve compiler errors as flagged).

- [ ] **Step 4: Confirm no behavioural change to existing callers**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: existing tests pass (no new failures introduced by the move).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/mask_generation.rs
git commit -m "refactor(tauri): use core mask rasterization; keep AI + same-signature wrapper"
```

---

# Phase P1 — Wire masks into the relm4 render path

### Task 5: `render()` accepts masks and applies them

**Files:**
- Modify: `rapidraw-core/src/render.rs`
- Test: `rapidraw-core/src/render.rs` (inline `#[cfg(test)]`)

- [ ] **Step 0: Make the JSON→MaskAdjustments parser reachable**

`get_mask_adjustments_from_json` is currently private. In `rapidraw-core/src/image_processing.rs:2165` change:

```rust
fn get_mask_adjustments_from_json(adj: &serde_json::Value) -> MaskAdjustments {
```
to
```rust
pub(crate) fn get_mask_adjustments_from_json(adj: &serde_json::Value) -> MaskAdjustments {
```
(`render.rs` is in the same crate; promote to `pub` later if a frontend needs it directly.)

- [ ] **Step 1: Extend the `render` signature and rasterize masks**

In `rapidraw-core/src/render.rs`, add imports and change `render` to take masks. Replace the `RenderRequest { ... mask_bitmaps: &[], ... }` block (lines 71–76) with rasterization at render resolution:

```rust
use crate::mask_generation::{generate_mask_bitmap, MaskDefinition};
use image::{GrayImage};
```

Add `masks: &[MaskDefinition]` as a parameter (after `adj`):

```rust
pub fn render(
    ctx: &GpuContext,
    base: &DynamicImage,
    adj: &AllAdjustments,
    masks: &[MaskDefinition],
    lut: Option<Arc<Lut>>,
    max_dim: Option<u32>,
) -> Result<DynamicImage, String> {
```

After `let (width, height) = base.dimensions();` and the `adj` copy (line 68-69), build the bitmaps + per-mask adjustments. `base` here is already geometry/crop-applied by the caller, and the original full-image dimension is needed for `scale`; for the uncropped path (P1 scope) `scale = width as f32 / original_width`. Since the caller passes the post-geometry base, we treat the post-geometry base as the coordinate space: mask coords from the UI are stored in that same post-geometry full-res space, so `scale = width / full_width` where `full_width` is the pre-downscale width. Capture it before the downscale:

```rust
    let full_width = base_full_dims.0; // see Step 2
```

Construct the request:

```rust
    let scale = if full_width > 0 { width as f32 / full_width as f32 } else { 1.0 };
    let mut mask_bitmaps: Vec<GrayImage> = Vec::new();
    let mut adj = *adj;
    for (i, m) in masks.iter().take(crate::image_processing::MAX_MASKS).enumerate() {
        if let Some(bmp) =
            generate_mask_bitmap(m, width, height, scale, (0.0, 0.0), Some(&base), None)
        {
            adj.mask_adjustments[i] =
                crate::image_processing::get_mask_adjustments_from_json(&m.adjustments);
            mask_bitmaps.push(bmp);
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
```

Remove the now-duplicated `let mut adj = *adj;` / `has_lut` lines (old 68-69) so `adj` is only shadowed once.

- [ ] **Step 2: Capture full (pre-downscale) width for scale**

At the top of `render`, before the `match max_dim` downscale block, capture the source dimensions:

```rust
    let base_full_dims = base.dimensions();
```

Then in Step 1, `full_width = base_full_dims.0`. (Masks coords are in the post-geometry full-res space; the only transform between that and the render bitmap is the preview downscale, so a single uniform `scale` and zero crop_offset are correct for the uncropped P1 path. Crop_offset for the cropped path is deferred to the P2/P3 UI plan.)

- [ ] **Step 3: Verify core compiles**

Run: `cargo check --manifest-path rapidraw-core/Cargo.toml`
Expected: `Finished` (callers will break — fixed in Task 6; this checks the crate lib alone compiles, which it does since `render` has no in-crate caller).

- [ ] **Step 4: Add an integration test: rendering with a mask changes pixels**

Add to `rapidraw-core/src/render.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::headless_context;
    use crate::image_processing::AllAdjustments;
    use crate::mask_generation::{MaskDefinition, SubMask, SubMaskMode};
    use image::{DynamicImage, RgbaImage};
    use serde_json::json;

    #[test]
    fn mask_with_exposure_changes_only_masked_region() {
        let Ok(ctx) = headless_context() else {
            eprintln!("no GPU; skipping");
            return; // ponytail: headless GPU may be absent in CI; skip rather than fail
        };
        let base = DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, image::Rgba([128, 128, 128, 255])));
        let mut adj = AllAdjustments::default();
        adj.mask_adjustments[0].exposure = 0.0; // overwritten by render() from json
        let mask = MaskDefinition {
            id: "m".into(), name: "m".into(), visible: true, invert: false, opacity: 100.0,
            adjustments: json!({ "exposure": 100.0 }),
            sub_masks: vec![SubMask {
                id: "s".into(), mask_type: "radial".into(), visible: true, invert: false,
                opacity: 100.0, mode: SubMaskMode::Additive,
                parameters: json!({ "centerX": 32.0, "centerY": 32.0, "radiusX": 16.0, "radiusY": 16.0, "rotation": 0.0, "feather": 0.2 }),
            }],
        };
        let out = render(&ctx, &base, &adj, std::slice::from_ref(&mask), None, None).unwrap().to_rgba8();
        let center = out.get_pixel(32, 32)[0];
        let corner = out.get_pixel(1, 1)[0];
        assert!(center > corner + 5, "masked center ({center}) should be brighter than corner ({corner})");
    }
}
```

- [ ] **Step 5: Run the test**

Run: `cargo test --manifest-path rapidraw-core/Cargo.toml render::`
Expected: PASS (or the skip message printed if no GPU is available).

- [ ] **Step 6: Commit**

```bash
git add rapidraw-core/src/render.rs
git commit -m "feat(core): render() applies masks (rasterize -> atlas + per-mask adjustments)"
```

### Task 6: relm4 session holds masks and passes them to render

**Files:**
- Modify: `rapidraw-relm4/src/state.rs`
- Modify: `rapidraw-relm4/src/main.rs:268-296` (RenderJob), `:573-628` (render thread)

- [ ] **Step 1: Add `masks` to the session state**

In `rapidraw-relm4/src/state.rs`, add the field next to `adjustments`:

```rust
use rapidraw_core::mask_generation::MaskDefinition;
// ... in the session struct:
    pub masks: Vec<MaskDefinition>,
```

And initialise it in the struct's constructor/`Default` (alongside `adjustments: AllAdjustments::default()`):

```rust
            masks: Vec::new(),
```

- [ ] **Step 2: Carry masks on the RenderJob preview/export variants**

In `rapidraw-relm4/src/main.rs`, add `masks: Vec<MaskDefinition>` to the `RenderJob::Preview` and `RenderJob::Export` variants (the two that call `render`). Add the import:

```rust
use rapidraw_core::mask_generation::MaskDefinition;
```

- [ ] **Step 3: Populate masks where RenderJobs are constructed**

At each site that builds `RenderJob::Preview { ... }` / `RenderJob::Export { ... }` (search `RenderJob::Preview {` and `RenderJob::Export {` in main.rs), add `masks: self.session.masks.clone(),`.

- [ ] **Step 4: Pass masks into `render` in the worker thread**

In `spawn_render_worker` (main.rs:573-628), destructure `masks` from each variant and pass it:

```rust
                    RenderJob::Export { base, adj, masks, lut, path, opts, geom } => {
                        let base = apply_geometry(&base, geom);
                        let res = rapidraw_core::render(&ctx, &base, &adj, &masks, lut, None)
                            .and_then(|out| encode_image(&out, &path, opts))
                            .map(|()| path);
                        let _ = cmd.send(CmdMsg::ExportDone(res));
                    }
```

and for the preview branch:

```rust
            if let Some(RenderJob::Preview { base, adj, masks, lut, dim, geom }) = latest_preview {
                let base = apply_geometry(&base, geom);
                match rapidraw_core::render(&ctx, &base, &adj, &masks, lut, Some(dim)) {
```

- [ ] **Step 5: Verify the relm4 app compiles**

Run: `cargo check --manifest-path rapidraw-relm4/Cargo.toml`
Expected: `Finished`. Fix any remaining `RenderJob` construction sites the compiler flags (it will name each missing `masks` field).

- [ ] **Step 6: Commit**

```bash
git add rapidraw-relm4/src/state.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): thread session masks through the render worker into render()"
```

### Task 7: Build the whole workspace + sanity run

- [ ] **Step 1: Build every crate**

Run: `cargo build --manifest-path rapidraw-core/Cargo.toml && cargo build --manifest-path rapidraw-relm4/Cargo.toml && cargo build --manifest-path src-tauri/Cargo.toml`
Expected: all `Finished`.

- [ ] **Step 2: Run all core tests**

Run: `cargo test --manifest-path rapidraw-core/Cargo.toml`
Expected: mask_generation + render tests pass.

- [ ] **Step 3: Manual smoke (optional, if a GPU/display is available)**

Run the relm4 app, open an image. With `session.masks` empty the preview must look exactly as before (no regression). Mask UI arrives in the next plan.

- [ ] **Step 4: Commit any fixups**

```bash
git add -A && git commit -m "chore(masks): workspace builds green with mask foundation"
```

---

## Deferred to the next plan (do NOT build here)

- **P2** Masks panel UI (list, add/delete/select, per-mask adjustments via existing slider tables → JSON, sub-mask list, mode/invert/opacity).
- **P3** Numeric geometry editors for color/luminance/brush + crop_offset handling for cropped images.
- **P4** Canvas overlay interaction (drag radial/linear, draw brush), sidecar persistence of masks.
- **P5** AI masks: extract ONNX inference from `src-tauri` into core, relm4 worker + SAM prompting (separate spec).

Each gets its own dated plan once this foundation is merged.

---

## Self-Review

- **Spec coverage:** P0 (move non-AI rasterization to core, AI resolver seam, src-tauri thin wrapper) = Tasks 1–4. P1 (relm4 render wiring: rasterize → atlas, `mask_count`, per-mask `MaskAdjustments` via `get_mask_adjustments_from_json`) = Tasks 5–7. Defaults-match test and full panel/canvas UI are explicitly deferred to the P2+ plan, consistent with the spec's phasing.
- **Placeholder scan:** the only `unimplemented!("paste verbatim")` is an explicit instruction to copy a named, line-referenced function body — not an unresolved TODO. No "add error handling"/"TBD" steps.
- **Type consistency:** `generate_mask_bitmap` (core) has the 7-arg signature with `ai: Option<AiResolver>` everywhere it's referenced; the src-tauri wrapper deliberately keeps the legacy 6-arg signature. `render` gains `masks: &[MaskDefinition]` in Task 5 and every caller is updated in Task 6. `MaskDefinition`/`SubMask`/`SubMaskMode` are defined once in core (Task 1) and re-exported from src-tauri (Task 4).
