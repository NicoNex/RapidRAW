# RapidRAW relm4 Native UI (Minimal Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native GTK4/relm4 RapidRAW frontend with one working loop — open folder → thumbnail grid → editor → global adjustments applied through the existing GPU engine → JPEG export — reusing the engine via a new tauri-free `rapidraw-core` crate.

**Architecture:** Extract the minimal image-engine slice from `src-tauri` into a new `rapidraw-core` crate with no Tauri dependencies, exposing `headless_context()`, `render()`, and `load_base_image()`. `src-tauri` is refactored to depend on `rapidraw-core` (behavior unchanged). A new `rapidraw-relm4` binary builds the GTK UI on top of `rapidraw-core`, rendering engine output (CPU readback RGBA) into a `gtk::Picture` via `gdk::MemoryTexture`.

**Tech Stack:** Rust, wgpu 29, image 0.25, GTK4, relm4 0.9, gdk4, pollster, rawler (RAW decode).

---

## Reference facts (verified against current code)

- `src-tauri` crate: package `RapidRAW`, lib name `rapidraw_lib`, crate-type includes `rlib`. Modules are private (`mod x;`).
- `GpuProcessor::new(context: GpuContext, max_w: u32, max_h: u32) -> Result<Self,String>` (`gpu_processing.rs:555`).
- `GpuProcessor::run(&self, input_view: &wgpu::TextureView, w: u32, h: u32, request: RenderRequest, skip_cpu_readback: bool, output_to_display: bool) -> Result<(Vec<u8>, u32, u32, u32, u32), String>` (`gpu_processing.rs:1076`). With `skip_cpu_readback=false` the first tuple element is RGBA8 pixels of size `out_w*out_h*4`.
- `RenderRequest<'a> { adjustments: AllAdjustments, mask_bitmaps: &'a [ImageBuffer<Luma<u8>,Vec<u8>>], lut: Option<Arc<Lut>>, roi: Option<Roi> }` (`gpu_processing.rs:24`).
- `GpuContext { device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>, limits: wgpu::Limits, display: Arc<Mutex<Option<WgpuDisplay>>> }` (`image_processing.rs:2324`).
- Input texture upload pattern: `to_rgba_f16(base)` (`gpu_processing.rs:484`) → `create_texture_with_data(..., Rgba16Float, ...)` → `create_view` (`gpu_processing.rs:1745-1769`).
- RAW decode: `develop_raw_image(file_bytes: &[u8], fast_demosaic: bool, highlight_compression: f32, linear_mode: String, cancel_token: Option<(Arc<AtomicUsize>,usize)>) -> Result<DynamicImage>` (`raw_processing.rs:15`).
- Adjustment JSON parse: `get_all_adjustments_from_json(...) -> AllAdjustments` (`image_processing.rs:2289`).
- Headless wgpu init on Linux = the existing compute-only path (`surface_opt = None`): `instance.request_adapter` → `request_device` (`gpu_processing.rs:198-243`).
- No root cargo workspace; crates are standalone and use path dependencies.

> **Coupling-cut rule (applies to every extraction task):** when a moved function takes `&tauri::State<AppState>` or `tauri::AppHandle`, replace that parameter with the concrete values it reads (a `&GpuProcessor`, `&wgpu::Device`, settings primitives). The AppState GPU-processor/image caches do NOT move to core — `rapidraw-core::render` owns a fresh `GpuProcessor` per call (or a caller-held one). If a function cannot be decoupled within the minimal loop, leave it in `src-tauri` and do not move it.

---

## Phase 0 — Scaffold `rapidraw-core` and make engine modules reachable

### Task 0.1: Create the `rapidraw-core` crate skeleton

**Files:**
- Create: `rapidraw-core/Cargo.toml`
- Create: `rapidraw-core/src/lib.rs`

- [ ] **Step 1: Create the crate manifest**

`rapidraw-core/Cargo.toml`:
```toml
[package]
name = "rapidraw-core"
version = "0.1.0"
edition = "2021"

[dependencies]
wgpu = "29.0"
image = { version = "0.25.10", features = ["jpeg", "png", "tiff", "webp"] }
pollster = "0.4"
bytemuck = { version = "1.25", features = ["derive"] }
half = "2"
log = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rawler = { git = "https://github.com/CyberTimon/RapidRAW-DngLab.git" }
```

- [ ] **Step 2: Create a placeholder lib root**

`rapidraw-core/src/lib.rs`:
```rust
//! rapidraw-core: tauri-free image engine extracted from src-tauri.
```

- [ ] **Step 3: Verify it builds**

Run: `cd rapidraw-core && cargo build`
Expected: compiles (downloads wgpu/rawler; may take minutes). PASS = no errors.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-core/Cargo.toml rapidraw-core/src/lib.rs
git commit -m "feat(core): scaffold rapidraw-core crate"
```

---

## Phase 1 — Move the already-clean modules into core

These modules have zero `tauri::` references and minimal `crate::` references: `formats`, `raw_processing`, `lut_processing`.

### Task 1.1: Move `formats`, `raw_processing`, `lut_processing` to core

**Files:**
- Create: `rapidraw-core/src/formats.rs` (from `src-tauri/src/formats.rs`)
- Create: `rapidraw-core/src/raw_processing.rs` (from `src-tauri/src/raw_processing.rs`)
- Create: `rapidraw-core/src/lut_processing.rs` (from `src-tauri/src/lut_processing.rs`)
- Modify: `rapidraw-core/src/lib.rs`

- [ ] **Step 1: Copy the three files into `rapidraw-core/src/`**

```bash
cp src-tauri/src/formats.rs rapidraw-core/src/formats.rs
cp src-tauri/src/raw_processing.rs rapidraw-core/src/raw_processing.rs
cp src-tauri/src/lut_processing.rs rapidraw-core/src/lut_processing.rs
```

- [ ] **Step 2: Declare them in the core lib root**

`rapidraw-core/src/lib.rs`:
```rust
//! rapidraw-core: tauri-free image engine extracted from src-tauri.

pub mod formats;
pub mod raw_processing;
pub mod lut_processing;
```

- [ ] **Step 3: Fix internal references**

In each copied file, replace any `crate::X` that refers to a sibling moved module with `crate::X` still valid in core (same module names), and any `crate::image_processing::Y` reference with a `use` that will resolve once Phase 2 lands. If a reference points to a module NOT being moved (e.g. `crate::app_settings`), inline the small constant or pass it as a parameter. Run the build to surface each one:

Run: `cd rapidraw-core && cargo build 2>&1 | grep -E "error\[|cannot find" | head`
Fix each reported unresolved path. Expected after fixes: only errors referencing `image_processing`/`gpu_processing` (added in Phase 2) remain, or clean build if none.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-core/src/formats.rs rapidraw-core/src/raw_processing.rs rapidraw-core/src/lut_processing.rs rapidraw-core/src/lib.rs
git commit -m "feat(core): move formats, raw_processing, lut_processing into core"
```

---

## Phase 2 — Move the compute kernels and cut tauri coupling

### Task 2.1: Move `image_processing` into core, decoupled

**Files:**
- Create: `rapidraw-core/src/image_processing.rs` (from `src-tauri/src/image_processing.rs`)
- Modify: `rapidraw-core/src/lib.rs`

- [ ] **Step 1: Copy the file**

```bash
cp src-tauri/src/image_processing.rs rapidraw-core/src/image_processing.rs
```

- [ ] **Step 2: Declare module**

Add to `rapidraw-core/src/lib.rs`:
```rust
pub mod image_processing;
```

- [ ] **Step 3: Cut tauri coupling**

In `rapidraw-core/src/image_processing.rs`:
- Remove the `GpuContext` field `display: Arc<Mutex<Option<WgpuDisplay>>>` ONLY if `WgpuDisplay` is not moved; instead keep `WgpuDisplay` (it lives in `gpu_processing`, moved in Task 2.2) so leave `GpuContext` intact.
- For `resolve_tonemapper_override(settings: &crate::AppSettings, is_raw: bool)` and `resolve_tonemapper_override_from_handle(...)`: replace with a single `pub fn resolve_tonemapper_override(setting: Option<u32>, is_raw: bool) -> Option<u32>` that takes the already-resolved setting value instead of `AppSettings`/handle. Keep the inner logic.
- For `get_all_adjustments_from_json` and `get_geometry_params_from_json`: these take `serde_json::Value` — keep as-is (no tauri).
- Remove any `use tauri::...` lines and any `#[tauri::command]` attributes (there should be none here; delete if present).

Run: `cd rapidraw-core && cargo build 2>&1 | grep -E "error" | head -30`
Fix each error by the coupling-cut rule. Expected remaining errors reference `gpu_processing` only (added next).

- [ ] **Step 4: Commit**

```bash
git add rapidraw-core/src/image_processing.rs rapidraw-core/src/lib.rs
git commit -m "feat(core): move image_processing into core, drop AppSettings coupling"
```

### Task 2.2: Move `gpu_processing` into core, decoupled (kernels only)

**Files:**
- Create: `rapidraw-core/src/gpu_processing.rs` (from `src-tauri/src/gpu_processing.rs`)
- Modify: `rapidraw-core/src/lib.rs`

- [ ] **Step 1: Copy the file**

```bash
cp src-tauri/src/gpu_processing.rs rapidraw-core/src/gpu_processing.rs
```

- [ ] **Step 2: Declare module**

Add to `rapidraw-core/src/lib.rs`:
```rust
pub mod gpu_processing;
```

- [ ] **Step 3: Remove the tauri-coupled orchestration functions**

Delete from the copied `rapidraw-core/src/gpu_processing.rs`:
- `get_or_init_gpu_context` (references `tauri::State`, `tauri::AppHandle`, `AppState`, `app_settings`).
- `process_and_get_dynamic_image`, `process_and_get_dynamic_image_with_analytics`, and `process_and_get_dynamic_image_inner` (reference `tauri::State<AppState>`, `state.gpu_processor`, `state.gpu_image_cache`).

Keep: `Roi`, `RenderRequest`, `DisplayTransform`, `WgpuDisplay`, `GpuProcessor` (`new`, `run`, all its private helpers), `to_rgba_f16`, and any free helpers used by `GpuProcessor`.

- [ ] **Step 4: Make required-by-driver items public**

In `rapidraw-core/src/gpu_processing.rs` change `fn to_rgba_f16` to `pub fn to_rgba_f16`. Ensure `GpuProcessor`, `GpuProcessor::new`, `GpuProcessor::run`, `RenderRequest`, `Roi` are `pub`.

Run: `cd rapidraw-core && cargo build 2>&1 | grep -E "error" | head -30`
Fix unresolved `crate::` paths (point them at the moved core modules). Expected: clean build.

- [ ] **Step 5: Run test to verify the crate compiles**

Run: `cd rapidraw-core && cargo build`
Expected: PASS, no errors.

- [ ] **Step 6: Commit**

```bash
git add rapidraw-core/src/gpu_processing.rs rapidraw-core/src/lib.rs
git commit -m "feat(core): move GpuProcessor into core, drop tauri orchestration wrappers"
```

### Task 2.3: Move image decode helpers into core

**Files:**
- Create: `rapidraw-core/src/image_loader.rs`
- Modify: `rapidraw-core/src/lib.rs`

- [ ] **Step 1: Copy only the tauri-free decode functions**

Create `rapidraw-core/src/image_loader.rs` containing copies of these functions from `src-tauri/src/image_loader.rs`: `load_base_image_from_bytes`, `load_image_with_orientation`, `composite_patches_on_image` only if referenced by those; do NOT copy `is_image_cached`, `load_and_composite`, or any function taking `tauri::State`. For each helper that those depend on, copy it too. Add the module header `use image::DynamicImage;` etc as the originals require.

- [ ] **Step 2: Declare module**

Add to `rapidraw-core/src/lib.rs`:
```rust
pub mod image_loader;
```

- [ ] **Step 3: Build and fix references**

Run: `cd rapidraw-core && cargo build 2>&1 | grep -E "error" | head`
Fix unresolved paths per the coupling-cut rule. Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-core/src/image_loader.rs rapidraw-core/src/lib.rs
git commit -m "feat(core): move tauri-free image decode helpers into core"
```

---

## Phase 3 — Core public API: headless context, render, load_base_image (TDD)

### Task 3.1: `headless_context()`

**Files:**
- Create: `rapidraw-core/src/context.rs`
- Modify: `rapidraw-core/src/lib.rs`

- [ ] **Step 1: Write the headless context constructor**

`rapidraw-core/src/context.rs`:
```rust
use std::sync::{Arc, Mutex};
use crate::image_processing::GpuContext;

/// Build a compute-only wgpu GpuContext with no surface/window (no Tauri).
pub fn headless_context() -> Result<GpuContext, String> {
    let instance = wgpu::Instance::new(
        &wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
    );

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        ..Default::default()
    }))
    .map_err(|e| format!("Failed to find a wgpu adapter: {e}"))?;

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
```

> If the `wgpu::Instance::new` / `request_device` field set differs from this wgpu 29 build, copy the exact field set from `src-tauri/src/gpu_processing.rs:198-243` (the verified working call), removing only the surface/flag-path code.

- [ ] **Step 2: Declare module**

Add to `rapidraw-core/src/lib.rs`:
```rust
mod context;
pub use context::headless_context;
```

- [ ] **Step 3: Build**

Run: `cd rapidraw-core && cargo build`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-core/src/context.rs rapidraw-core/src/lib.rs
git commit -m "feat(core): headless_context() builds compute-only GpuContext"
```

### Task 3.2: `render()` driver (TDD)

**Files:**
- Create: `rapidraw-core/src/render.rs`
- Modify: `rapidraw-core/src/lib.rs`
- Test: `rapidraw-core/tests/render_smoke.rs`

- [ ] **Step 1: Write the failing smoke test**

`rapidraw-core/tests/render_smoke.rs`:
```rust
use image::{DynamicImage, RgbImage};
use rapidraw_core::{headless_context, render};
use rapidraw_core::image_processing::AllAdjustments;

fn mean_luma(img: &DynamicImage) -> f32 {
    let rgb = img.to_rgb8();
    let mut sum = 0.0f64;
    for p in rgb.pixels() {
        sum += (0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64);
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
```

> If `AllAdjustments`/`GlobalAdjustments` do not derive `Default`, add `#[derive(Default)]` (or a `Default` impl matching `Adjustments::default()` neutral values) in `image_processing.rs` as part of this step, and add `pub use image_processing::AllAdjustments;` is not required since the test uses the full path.

- [ ] **Step 2: Run test to verify it fails**

Run: `cd rapidraw-core && cargo test --test render_smoke`
Expected: FAIL — `render` not found / does not compile.

- [ ] **Step 3: Implement `render()`**

`rapidraw-core/src/render.rs`:
```rust
use image::{DynamicImage, GenericImageView, RgbaImage};
use wgpu::util::{DeviceExt, TextureDataOrder};

use crate::gpu_processing::{to_rgba_f16, GpuProcessor, RenderRequest};
use crate::image_processing::{AllAdjustments, GpuContext};

/// Render `base` through the GPU pipeline with `adj`. Optionally downscale the
/// longest edge to `max_dim` first (for fast previews). Returns an RGBA8 image.
pub fn render(
    ctx: &GpuContext,
    base: &DynamicImage,
    adj: &AllAdjustments,
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
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
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

    let processor = GpuProcessor::new(ctx.clone(), width, height)?;
    let request = RenderRequest {
        adjustments: adj.clone(),
        mask_bitmaps: &[],
        lut: None,
        roi: None,
    };

    let (pixels, out_w, out_h, _x, _y) =
        processor.run(&input_view, width, height, request, false, false)?;

    let img = RgbaImage::from_raw(out_w, out_h, pixels)
        .ok_or_else(|| "readback buffer size mismatch".to_string())?;
    Ok(DynamicImage::ImageRgba8(img))
}
```

> `GpuContext` must be `Clone`. If it is not, add `#[derive(Clone)]` to its definition in `image_processing.rs` (all fields are `Arc`/`wgpu::Limits` which are `Clone`).
> If `AllAdjustments`/`GlobalAdjustments` are not `Clone`, add `#[derive(Clone)]`.

- [ ] **Step 4: Declare module**

Add to `rapidraw-core/src/lib.rs`:
```rust
mod render;
pub use render::render;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd rapidraw-core && cargo test --test render_smoke`
Expected: PASS (or the explicit "no GPU adapter; skipping" early return on headless CI).

- [ ] **Step 6: Commit**

```bash
git add rapidraw-core/src/render.rs rapidraw-core/src/lib.rs rapidraw-core/tests/render_smoke.rs rapidraw-core/src/image_processing.rs
git commit -m "feat(core): render() drives GpuProcessor without AppState; smoke test"
```

### Task 3.3: `load_base_image()` (format dispatch incl. RAW)

**Files:**
- Modify: `rapidraw-core/src/image_loader.rs`
- Modify: `rapidraw-core/src/lib.rs`

- [ ] **Step 1: Add the dispatching loader**

Append to `rapidraw-core/src/image_loader.rs`:
```rust
use std::path::Path;
use image::DynamicImage;
use crate::formats::is_raw_file;
use crate::raw_processing::develop_raw_image;
use crate::image_processing::apply_cpu_default_raw_processing;

/// Decode any supported image (standard or RAW) into a display-ready base image.
pub fn load_base_image(path: &Path) -> Result<DynamicImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if is_raw_file(path) {
        let mut img = develop_raw_image(&bytes, false, 0.0, "auto".to_string(), None)
            .map_err(|e| e.to_string())?;
        apply_cpu_default_raw_processing(&mut img);
        Ok(img)
    } else {
        // Honor EXIF orientation for standard formats.
        load_image_with_orientation(&bytes).map_err(|e| e.to_string())
    }
}
```

> Confirm `is_raw_file`'s signature — if it takes `&str`/`String` rather than `&Path`, pass `path.to_string_lossy().as_ref()` / `&path.display().to_string()` accordingly. Confirm `develop_raw_image`'s `linear_mode` accepted values from `raw_processing.rs` and use the engine's default (search how `lib.rs` calls it; copy that exact argument set).
> Confirm `load_image_with_orientation`'s real signature (bytes vs path) and adapt the call.

- [ ] **Step 2: Export it**

Add to `rapidraw-core/src/lib.rs`:
```rust
pub use image_loader::load_base_image;
```

- [ ] **Step 3: Build**

Run: `cd rapidraw-core && cargo build`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-core/src/image_loader.rs rapidraw-core/src/lib.rs
git commit -m "feat(core): load_base_image dispatches RAW and standard formats"
```

---

## Phase 4 — Refactor `src-tauri` to depend on `rapidraw-core`

Goal: one engine implementation. `src-tauri` deletes its moved module bodies and re-exports/calls core, keeping all commands and behavior intact.

### Task 4.1: Add core dependency and re-export moved modules

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/lib.rs`
- Delete: `src-tauri/src/formats.rs`, `src-tauri/src/lut_processing.rs`, `src-tauri/src/raw_processing.rs`

- [ ] **Step 1: Add the path dependency**

In `src-tauri/Cargo.toml` `[dependencies]` add:
```toml
rapidraw-core = { path = "../rapidraw-core" }
```

- [ ] **Step 2: Replace moved module declarations with re-exports**

In `src-tauri/src/lib.rs`, replace the lines `mod formats;`, `mod lut_processing;`, `mod raw_processing;` with:
```rust
use rapidraw_core::{formats, lut_processing, raw_processing};
```
Delete the three now-duplicated source files:
```bash
git rm src-tauri/src/formats.rs src-tauri/src/lut_processing.rs src-tauri/src/raw_processing.rs
```

- [ ] **Step 3: Build src-tauri**

Run: `cd src-tauri && cargo build 2>&1 | grep -E "error" | head -30`
Fix path references (`crate::formats::X` → still valid via the `use` re-export; if any code used `crate::raw_processing` it now resolves through the `use`). Expected: builds, OR errors only from `image_processing`/`gpu_processing` still being local (handled next task).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/lib.rs
git commit -m "refactor(tauri): source formats/lut/raw from rapidraw-core"
```

### Task 4.2: Re-point `image_processing`, `gpu_processing`, `image_loader` to core

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/gpu_processing.rs` (keep ONLY the tauri orchestration wrappers)
- Delete: `src-tauri/src/image_processing.rs` (moved); keep a thin shim if other modules reference private items
- Modify: `src-tauri/src/image_loader.rs` (keep only tauri-coupled fns)

- [ ] **Step 1: Reduce `src-tauri/src/gpu_processing.rs` to the orchestration layer**

Delete from `src-tauri/src/gpu_processing.rs` everything now living in core (`GpuProcessor`, `RenderRequest`, `Roi`, `WgpuDisplay`, `DisplayTransform`, `to_rgba_f16`). Keep `get_or_init_gpu_context`, `process_and_get_dynamic_image*`. At the top add:
```rust
use rapidraw_core::gpu_processing::{to_rgba_f16, GpuProcessor, RenderRequest, Roi};
use rapidraw_core::image_processing::GpuContext;
```
Update `process_and_get_dynamic_image_inner` to construct `GpuProcessor` from `rapidraw_core` and build `RenderRequest` with the core type (identical fields). The AppState GPU caches stay here.

- [ ] **Step 2: Re-export `image_processing` and `image_loader` decode from core**

In `src-tauri/src/lib.rs`:
```rust
use rapidraw_core::image_processing;
```
Delete `src-tauri/src/image_processing.rs`:
```bash
git rm src-tauri/src/image_processing.rs
```
Keep `src-tauri/src/image_loader.rs` but delete the functions moved to core and re-export them:
```rust
pub use rapidraw_core::image_loader::{load_base_image_from_bytes, load_image_with_orientation};
```

- [ ] **Step 3: Build the whole tauri app**

Run: `cd src-tauri && cargo build 2>&1 | tail -20`
Fix remaining unresolved paths (`crate::image_processing::X` → `image_processing::X` via the `use`; `crate::gpu_processing::GpuProcessor` → `rapidraw_core::gpu_processing::GpuProcessor`). Expected: PASS.

- [ ] **Step 4: Smoke-run the existing Tauri app to confirm no behavior regression**

Run: `cd src-tauri && cargo build --release 2>&1 | tail -5`
Expected: PASS. (Full `tauri dev` launch optional; build success is the gate for this plan.)

- [ ] **Step 5: Commit**

```bash
git add -A src-tauri
git commit -m "refactor(tauri): use rapidraw-core engine, keep orchestration wrappers"
```

---

## Phase 5 — Scaffold `rapidraw-relm4`

### Task 5.1: Create the relm4 binary crate with a blank window

**Files:**
- Create: `rapidraw-relm4/Cargo.toml`
- Create: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Manifest**

`rapidraw-relm4/Cargo.toml`:
```toml
[package]
name = "rapidraw-relm4"
version = "0.1.0"
edition = "2021"

[dependencies]
rapidraw-core = { path = "../rapidraw-core" }
relm4 = "0.9"
gtk = { version = "0.9", package = "gtk4" }
gdk = { version = "0.9", package = "gdk4" }
image = "0.25.10"
log = "0.4"
env_logger = "0.11"
```

- [ ] **Step 2: Minimal relm4 app showing an empty window**

`rapidraw-relm4/src/main.rs`:
```rust
use relm4::prelude::*;
use gtk::prelude::*;

struct AppModel;

#[relm4::component]
impl SimpleComponent for AppModel {
    type Init = ();
    type Input = ();
    type Output = ();

    view! {
        gtk::Window {
            set_title: Some("RapidRAW"),
            set_default_size: (1440, 900),
            gtk::Label { set_label: "RapidRAW (relm4) — scaffold" },
        }
    }

    fn init(_: (), root: Self::Root, _sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let model = AppModel;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }
}

fn main() {
    env_logger::init();
    let app = RelmApp::new("com.rapidraw.relm4");
    app.run::<AppModel>(());
}
```

- [ ] **Step 3: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: an empty GTK window titled "RapidRAW" appears. (Requires gtk4 dev libs installed.)

- [ ] **Step 4: Commit**

```bash
git add rapidraw-relm4/Cargo.toml rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): scaffold relm4 app with blank window"
```

### Task 5.2: Hold a shared `GpuContext` and app state

**Files:**
- Create: `rapidraw-relm4/src/state.rs`
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Define shared state**

`rapidraw-relm4/src/state.rs`:
```rust
use std::path::PathBuf;
use std::sync::Arc;
use image::DynamicImage;
use rapidraw_core::image_processing::{AllAdjustments, GpuContext};

#[derive(Clone)]
pub struct Engine {
    pub ctx: Arc<GpuContext>,
}

pub struct Session {
    pub current_folder: Option<PathBuf>,
    pub active_path: Option<PathBuf>,
    pub base_image: Option<Arc<DynamicImage>>,
    pub adjustments: AllAdjustments,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            current_folder: None,
            active_path: None,
            base_image: None,
            adjustments: AllAdjustments::default(),
        }
    }
}
```

- [ ] **Step 2: Initialize the engine at startup**

In `rapidraw-relm4/src/main.rs`, before `app.run`, build the context once and pass it as `Init`:
```rust
mod state;
use state::Engine;
use std::sync::Arc;

// change AppModel::Init to Engine and store it in the model
```
Update `AppModel` to `struct AppModel { engine: Engine, session: state::Session }`, set `type Init = Engine;`, and in `main`:
```rust
let ctx = rapidraw_core::headless_context().expect("gpu init");
let engine = Engine { ctx: Arc::new(ctx) };
let app = RelmApp::new("com.rapidraw.relm4");
app.run::<AppModel>(engine);
```

- [ ] **Step 3: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: window still opens; GPU context initializes without panic (check log line or no crash).

- [ ] **Step 4: Commit**

```bash
git add rapidraw-relm4/src/state.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): init shared GpuContext and session state"
```

---

## Phase 6 — Folder open + tree

### Task 6.1: Folder picker and current folder

**Files:**
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Add an "Open Folder" button wired to gtk::FileDialog**

Add to the `view!` a header with a button; add an input message:
```rust
#[derive(Debug)]
enum AppMsg {
    OpenFolderDialog,
    FolderChosen(std::path::PathBuf),
}
```
In `update`, on `OpenFolderDialog` open `gtk::FileDialog::new().select_folder(...)`, and on success send `FolderChosen(path)`; on `FolderChosen` store it in `session.current_folder` and trigger library load (Phase 7). Use relm4's async command or the dialog's callback with `sender.input(...)`.

```rust
fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
    match msg {
        AppMsg::OpenFolderDialog => {
            let dialog = gtk::FileDialog::builder().title("Select folder").build();
            let sender = sender.clone();
            dialog.select_folder(None::<&gtk::Window>, gtk::gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        sender.input(AppMsg::FolderChosen(path));
                    }
                }
            });
        }
        AppMsg::FolderChosen(path) => {
            self.session.current_folder = Some(path);
            // Phase 7: kick off scan
        }
    }
}
```

- [ ] **Step 2: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: clicking "Open Folder" shows the native folder chooser; selecting a folder stores it (verify via a log line printing the chosen path).

- [ ] **Step 3: Commit**

```bash
git add rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): native folder picker sets current folder"
```

> The folder *tree* (nested expand/collapse) is a thin enhancement; for the minimal loop a single chosen folder is sufficient. Defer the recursive tree widget — `// ponytail: single-folder open covers the core loop; add nested TreeListModel only if users need subfolder browsing.`

---

## Phase 7 — Library grid with progressive thumbnails

### Task 7.1: Scan folder and list image files

**Files:**
- Create: `rapidraw-relm4/src/library.rs`
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Directory scan (reuse extension table)**

`rapidraw-relm4/src/library.rs`:
```rust
use std::path::{Path, PathBuf};

const EXT: &[&str] = &[
    "jpg","jpeg","png","tiff","tif","webp",
    "raw","arw","cr2","cr3","nef","orf","raf","dng","rw2","pef","srw","3fr","mef",
];

pub fn scan_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| p.extension()
            .and_then(|x| x.to_str())
            .map(|x| EXT.contains(&x.to_lowercase().as_str()))
            .unwrap_or(false))
        .collect();
    v.sort();
    v
}
```

- [ ] **Step 2: Call scan on FolderChosen**

In `main.rs` `update`, on `AppMsg::FolderChosen`, set `self.images = library::scan_dir(&path)` (add `images: Vec<PathBuf>` to `AppModel`). Log the count.

- [ ] **Step 3: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: choosing a folder logs "N images".

- [ ] **Step 4: Commit**

```bash
git add rapidraw-relm4/src/library.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): scan folder for image files"
```

### Task 7.2: Thumbnail factory grid with background decode

**Files:**
- Modify: `rapidraw-relm4/src/library.rs`
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Helper to build a gdk texture from a DynamicImage**

Add to `rapidraw-relm4/src/library.rs`:
```rust
use gtk::gdk;
use gtk::glib::Bytes;
use image::{DynamicImage, GenericImageView};

pub fn texture_from_image(img: &DynamicImage) -> gdk::MemoryTexture {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let bytes = Bytes::from(rgba.as_raw());
    gdk::MemoryTexture::new(
        w as i32,
        h as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        (w * 4) as usize,
    )
}
```

- [ ] **Step 2: Add a FlowBox + FactoryVecDeque of thumbnails**

In `main.rs`, add a `FactoryVecDeque<Thumb>` where each `Thumb { path, texture: Option<gdk::MemoryTexture> }` renders a `gtk::Picture` (or `gtk::Image` from texture) plus filename. On `FolderChosen`, clear and push one `Thumb` per scanned path with `texture: None`.

- [ ] **Step 3: Decode thumbnails on background commands, progressively**

For each image path, spawn a relm4 `Command` (`sender.command` / `relm4::spawn`) that calls `rapidraw_core::load_base_image(&path)` then downscales to ~300px and builds the texture off-thread; on completion send `AppMsg::ThumbReady(index, texture)` which updates that factory item. Decode must NOT build the gdk texture on the worker thread (GTK objects are main-thread only) — return the decoded+resized `DynamicImage` (or raw RGBA `Vec<u8>` + dims) from the worker, and build `gdk::MemoryTexture` in `update` on the main thread.

```rust
// worker returns (usize, image::RgbaImage)
// update: AppMsg::ThumbReady(i, rgba) => build texture, set on factory item
```

- [ ] **Step 4: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: choose a folder → grid fills with thumbnails progressively, UI stays responsive.

- [ ] **Step 5: Commit**

```bash
git add rapidraw-relm4/src/library.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): progressive thumbnail grid via background decode"
```

### Task 7.3: Double-click opens image in editor

**Files:**
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Emit OpenInEditor on activate**

Add `AppMsg::OpenInEditor(PathBuf)` from the FlowBox `child-activated` signal. In `update`, decode the full base image on a background command (`load_base_image`), store `session.base_image`, set `session.active_path`, switch a `gtk::Stack` from "library" to "editor" page, and trigger an initial render (Phase 9).

- [ ] **Step 2: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: double-click a thumbnail → view switches to the (empty for now) editor page; base image decodes (log dimensions).

- [ ] **Step 3: Commit**

```bash
git add rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): open image in editor on activation"
```

---

## Phase 8 — Editor canvas (Picture + zoom/pan)

### Task 8.1: Display the base image in a Picture

**Files:**
- Create: `rapidraw-relm4/src/editor.rs`
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Editor page with a gtk::Picture**

`rapidraw-relm4/src/editor.rs` defines a relm4 component (or inline widget) holding a `gtk::Picture` with `set_can_shrink(true)` and `set_content_fit(gtk::ContentFit::Contain)`. Expose a method/message `SetTexture(gdk::MemoryTexture)` that calls `picture.set_paintable(Some(&texture))`.

- [ ] **Step 2: On OpenInEditor, show the un-adjusted base immediately**

After decoding base in Phase 7.3, build a texture (`library::texture_from_image`) and set it on the editor Picture so the user sees the image before adjustments render.

- [ ] **Step 3: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: opening an image shows it in the editor page, scaled to fit.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-relm4/src/editor.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): editor displays base image in gtk::Picture"
```

### Task 8.2: Zoom and pan

**Files:**
- Modify: `rapidraw-relm4/src/editor.rs`

- [ ] **Step 1: Wrap the Picture for zoom/pan**

Use a `gtk::ScrolledWindow` with a `gtk::Fixed`/`gtk::Picture`, or attach a `gtk::EventControllerScroll` (zoom = scale factor on the Picture via `set_width/height_request` or a `gtk::AspectFrame` scale) and a `gtk::GestureDrag` for pan (translate via `ScrolledWindow` adjustments). Mirror the zoom clamp `[0.05, 20.0]` from the src-gpui editor.

- [ ] **Step 2: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: scroll zooms the image, drag pans it.

- [ ] **Step 3: Commit**

```bash
git add rapidraw-relm4/src/editor.rs
git commit -m "feat(relm4): editor zoom and pan"
```

> `// ponytail: ScrolledWindow-based zoom is the cheapest path; revisit only if pixel-accurate 1:1 zoom is needed.`

---

## Phase 9 — Adjustment controls + debounced render

### Task 9.1: Build the slider panel

**Files:**
- Create: `rapidraw-relm4/src/controls.rs`
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Map sliders to GlobalAdjustments fields**

Inspect `GlobalAdjustments` in `rapidraw-core/src/image_processing.rs` and list the f32 fields to expose: exposure, contrast, highlights, shadows, whites, blacks, clarity, vibrance, saturation, temperature, tint, sharpness, luma/color noise reduction, vignette, grain, dehaze, structure/texture. For each, create a `gtk::Scale` with the engine's documented range (copy ranges from `src-gpui/src/views/controls.rs:63-82` which already encodes them).

- [ ] **Step 2: Emit AdjustmentChanged on value-changed**

```rust
#[derive(Debug, Clone)]
pub enum Adjust { Exposure(f32), Contrast(f32), /* ... one per field ... */ }
```
Each `gtk::Scale` `connect_value_changed` sends `AppMsg::Adjust(Adjust::Field(v))`. In `update`, write the value into `self.session.adjustments.global.<field>` then send `AppMsg::RequestRender`.

- [ ] **Step 3: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: sliders render in the right panel; moving one logs the updated adjustment value.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-relm4/src/controls.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): adjustment slider panel updates session adjustments"
```

### Task 9.2: Debounced GPU render to preview texture

**Files:**
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Debounce RequestRender**

On `AppMsg::RequestRender`, set a "dirty" flag and (re)start a short timer (e.g. `glib::timeout_add_local_once(Duration::from_millis(80), ...)`) that sends `AppMsg::DoRender`. Restarting the timer on each change coalesces rapid slider movement.

- [ ] **Step 2: Render on a background command, return RGBA to main thread**

On `AppMsg::DoRender`, clone `engine.ctx`, `session.base_image` (Arc), and `session.adjustments`, then run a command:
```rust
let ctx = self.engine.ctx.clone();
let base = self.session.base_image.clone();
let adj = self.session.adjustments.clone();
sender.oneshot_command(async move {
    let base = base?;
    let out = rapidraw_core::render(&ctx, &base, &adj, Some(2048)).ok()?;
    Some(out.to_rgba8()) // RgbaImage is Send; gdk texture built on main thread
});
// map command output -> AppMsg::RenderReady(rgba)
```
On `AppMsg::RenderReady(rgba)` (main thread) build `gdk::MemoryTexture` and set it on the editor Picture.

> `GpuContext` is `Arc`-wrapped (`device`/`queue` are `Arc`), so it is `Send + Sync` and safe to use from the command. `GpuProcessor` is created inside `render` per call, so no cross-thread GPU object sharing issues.
> `// ponytail: build a new GpuProcessor per render call; cache one keyed by max dimensions if slider latency is too high.`

- [ ] **Step 3: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: open an image, move Exposure → the preview updates with the adjustment applied through the engine, UI stays responsive during drags.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): debounced GPU render feeds editor preview"
```

---

## Phase 10 — Export JPEG

### Task 10.1: Full-res render + save dialog

**Files:**
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Export button → save dialog → full-res render → encode**

Add an "Export" button sending `AppMsg::ExportDialog`. In `update`, open `gtk::FileDialog::save(...)`; on a chosen path send `AppMsg::ExportTo(path)`. On `ExportTo`, run a background command:
```rust
let ctx = self.engine.ctx.clone();
let base = self.session.base_image.clone();
let adj = self.session.adjustments.clone();
sender.oneshot_command(async move {
    let base = base?;
    let out = rapidraw_core::render(&ctx, &base, &adj, None).ok()?; // full res
    out.to_rgb8().save_with_format(&path, image::ImageFormat::Jpeg).ok()?;
    Some(())
});
```
Show a status-bar message on success/failure.

- [ ] **Step 2: Build and run**

Run: `cd rapidraw-relm4 && cargo run`
Expected: adjust an image, Export, choose a path → a JPEG with the adjustments baked in is written; open it externally to confirm.

- [ ] **Step 3: Commit**

```bash
git add rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): export full-res adjusted JPEG"
```

---

## Phase 11 — Mark `src-gpui` superseded

### Task 11.1: Note supersession (no deletion)

**Files:**
- Create: `src-gpui/SUPERSEDED.md`

- [ ] **Step 1: Add the note**

`src-gpui/SUPERSEDED.md`:
```markdown
# Superseded

This GPUI port is superseded by the relm4 native UI in `rapidraw-relm4/`.
Kept for reference until relm4 reaches parity. Do not add features here.
See `docs/superpowers/specs/2026-06-16-rapidraw-relm4-core-design.md`.
```

- [ ] **Step 2: Commit**

```bash
git add src-gpui/SUPERSEDED.md
git commit -m "docs: mark src-gpui as superseded by relm4 UI"
```

---

## Final verification

- [ ] `cd rapidraw-core && cargo test` — render smoke test passes (or skips with no adapter).
- [ ] `cd src-tauri && cargo build` — existing Tauri app still builds with engine sourced from core.
- [ ] `cd rapidraw-relm4 && cargo run` — full loop works: open folder → grid → open image → move sliders (engine-applied preview) → export JPEG.

---

## Self-review notes (coverage vs spec)

- Crate layout (core/tauri/relm4): Phases 0, 4, 5. ✓
- Core minimal slice + headless_context + render + load_base_image: Phases 1–3. ✓
- Coupling-cut rule for kernels: stated globally + per task. ✓
- RAW decode included: Task 3.3. ✓
- relm4 components (FolderTree/Library/Editor/Controls/Export): Phases 6–10. ✓
- CPU-readback → gdk::MemoryTexture bridge: Tasks 7.2, 9.2. ✓
- Worker-thread render + debounce: Task 9.2. ✓
- Error handling as status messages, no panics: Tasks 9.2, 10.1 (status-bar). ✓
- Testing (core smoke test, UI manual): Task 3.2 + Final verification. ✓
- src-gpui superseded, not deleted: Phase 11. ✓
- Deferred items (masks/AI/crop/panorama/presets/folder-tree-nesting): explicitly out of scope; folder nesting marked ponytail-deferred in Phase 6. ✓
