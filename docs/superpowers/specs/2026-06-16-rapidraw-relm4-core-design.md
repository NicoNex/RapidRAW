# RapidRAW relm4 native UI — minimal usable core

Date: 2026-06-16
Status: Design approved, pending spec review

## Goal

Replace the webview frontend with a native GTK4 UI built on [relm4](https://github.com/Relm4/Relm4),
reusing the existing Rust image engine. This spec covers the **minimal usable core** only — one
complete working loop:

> open folder → thumbnail grid → open image in editor → global adjustment sliders applied through
> the existing GPU engine → export JPEG.

RAW decode is included (RapidRAW is a RAW editor). Masks, crop UI, AI, panorama, culling, presets,
and multi-format export are out of scope here and get their own later specs.

The existing `src-gpui/` GPUI port is **superseded** by this relm4 track. It is left in place
untouched for now; it will be removed once relm4 reaches parity (separate decision, not this spec).

## Non-goals

- No feature parity with the React frontend in this spec.
- No masks / AI / panorama / culling / presets.
- No crop/geometry UI (the engine geometry code moves to core but stays unwired).
- No removal of the Tauri app or `src-gpui`.
- No GL/wgpu-into-GTK interop (CPU readback path only — see Rendering bridge).

## Crate layout

```
rapidraw-core/      NEW   tauri-free image engine (extracted from src-tauri)
src-tauri/          EXISTS refactored to depend on rapidraw-core
rapidraw-relm4/     NEW   GTK4/relm4 UI binary, depends on rapidraw-core
```

Both `src-tauri` and `rapidraw-relm4` depend on `rapidraw-core` via path dependency. The Tauri app
keeps building and shipping unchanged in behavior; it just sources the engine from the new crate.

## rapidraw-core (minimal slice only)

Only the modules the minimal loop needs are extracted. Everything else stays in `src-tauri`.

**Moved as-is (already tauri-free):**
- `formats` (`is_raw_file`, extension tables)
- `raw_processing` (RAW decode)
- `lut_processing` (needed transitively by the GPU pipeline types)

**Moved, with tauri coupling cut from the compute kernels** (each has only 3–9 `tauri::State<AppState>`
references today, all in orchestration wrappers, not the kernels):
- `gpu_processing` — `GpuProcessor`, `GpuProcessor::new`, `GpuProcessor::run`, `RenderRequest`, `Roi`,
  `GpuContext` (and its wgpu device/queue/limits).
- `image_processing` — `AllAdjustments`, `GlobalAdjustments`, `MaskAdjustments` (kept for struct
  completeness, unused in core loop), adjustment JSON parsing (`get_all_adjustments_from_json`,
  `get_geometry_params_from_json`), geometry warp/crop/orientation helpers,
  `apply_cpu_default_raw_processing`, sRGB/linear conversions, tonemap helpers.
- `image_loader` — image decode functions (`load_base_image_from_bytes`, `load_image_with_orientation`).
  The `tauri::State`-coupled cache helpers (e.g. `is_image_cached`) stay in `src-tauri`.

**New in core:**
- `headless_context() -> Result<GpuContext, String>` — builds a wgpu `Device`/`Queue`/`Limits`
  with no surface, no `AppHandle`, no `AppState`. Lifted from `get_or_init_gpu_context` minus the
  window/surface and crash-flag-file code.
- `render(ctx: &GpuContext, base: &DynamicImage, adj: &AllAdjustments, max_dim: Option<u32>)
  -> Result<DynamicImage, String>` — thin driver that replaces the AppState-cached
  `process_and_get_dynamic_image` wrapper: optionally downscales `base`, uploads it to an input
  texture, constructs a `GpuProcessor` sized to the image, calls `run` with a
  `RenderRequest { adjustments: adj, mask_bitmaps: &[], lut: None, roi: None }`, and returns the
  resulting `DynamicImage`. No caching layer (the relm4 app caches at its own level if needed).
- `load_base_image(path: &Path) -> Result<DynamicImage, String>` — format-dispatching loader:
  RAW (`is_raw_file`) → `raw_processing` decode + `apply_cpu_default_raw_processing` + orientation;
  otherwise standard `image` decode + orientation. Returns a display-ready linear/sRGB
  `DynamicImage` matching what the engine expects as `base_image`.

**Stays in src-tauri (not moved):** `cache_utils`, `AppState`, `app_settings`, `app_state`, all
`#[tauri::command]`s, masks/AI/panorama/culling/export-processing/tagging. The existing tauri
wrappers are rewritten to call `rapidraw_core::render` / `load_base_image` instead of holding the
logic inline, so there is one engine implementation.

### Extraction method (how the coupling is cut)

For each moved kernel function that currently takes `&tauri::State<AppState>`:
- The GPU processor parameters it actually reads (gpu_processor handle, limits) become plain
  function arguments owned by the caller. In core, `render` owns a `GpuProcessor` directly instead
  of pulling it from `AppState.gpu_processor`.
- Settings-derived values (e.g. tonemapper override from `AppSettings`) become explicit parameters
  with sane defaults; src-tauri passes its real settings, the relm4 app passes defaults.
- Anything that cannot be cleanly decoupled for the minimal loop is left in src-tauri and simply not
  part of core.

## rapidraw-relm4 (UI)

Standard relm4 / Elm architecture. One root `AppModel`, child components communicating by messages.

**Components:**
- `FolderTree` — left panel. Native folder picker via `gtk::FileDialog`; expandable directory tree
  (`gtk::ListView`/`TreeListModel` or a simple nested model). Emits `FolderSelected(PathBuf)`.
- `Library` — center grid. `relm4::factory::FactoryVecDeque<Thumb>` inside a `gtk::FlowBox`.
  Thumbnails decoded on background `Command`s via `rapidraw_core::load_base_image` (downscaled),
  progressive fill as each completes. Single click selects; double click emits `OpenInEditor(PathBuf)`.
- `Editor` — center, replaces the grid when an image is open. A `gtk::Picture` displaying a
  `gdk::MemoryTexture`. Scroll = zoom, drag = pan (mirrors the existing src-gpui editor math).
  Receives rendered-preview textures from the render Command.
- `Controls` — right panel. Global adjustment sliders: exposure, contrast, highlights, shadows,
  whites, blacks, clarity, vibrance, saturation, temperature, tint, plus detail (sharpness, noise
  reduction, luminance NR) and effects (vignette, grain, dehaze, texture). Each `gtk::Scale` change
  updates `AppModel.adjustments` and triggers a debounced `Render`.
- Export — `gtk::FileDialog` save dialog → core renders full-res → encode JPEG to chosen path.

**State (`AppModel`):**
- `gpu_context: GpuContext` — one, created at startup, lives for the app.
- `current_folder: Option<PathBuf>`, `images: Vec<…>`, `active_path: Option<PathBuf>`.
- `base_image: Option<DynamicImage>` — decoded once when an image opens.
- `adjustments: AllAdjustments` — source of truth for the sliders.
- View state: zoom, pan offset, panel sizes.

## Data flow

```
slider change → AppModel.adjustments updated → debounced Render message
  → relm4 Command (worker thread):
       rapidraw_core::render(&ctx, &base_image, &adjustments, Some(preview_dim))
  → returns DynamicImage (RGBA)
  → main thread: build gdk::MemoryTexture from bytes → Editor's gtk::Picture
```

Opening an image: `OpenInEditor(path)` → Command decodes `load_base_image(path)` → store as
`base_image` → trigger an initial `Render` with current adjustments.

Export: full-res `render(..., None)` on a Command → `image` crate JPEG encode → save.

## Rendering bridge

CPU-readback only. `GpuProcessor::run` already returns `Vec<u8>` RGBA via `skip_cpu_readback = false`.
Those bytes go into a `gdk::MemoryTexture` (`gdk::MemoryFormat::R8g8b8a8`) shown in a `gtk::Picture`.

`// ponytail: full-res re-upload per edit via CPU readback; switch to GLArea + dmabuf/zero-copy if
preview latency hurts.` The engine renders downscaled previews, so for the minimal loop this is
adequate. Preview dimension chosen to fit the editor viewport.

## Threading

GPU render and image decode run on relm4 `Command`s (worker threads / tokio) so the UI thread never
blocks during slider drags or folder loads. Slider-driven renders are debounced (coalesce rapid
changes to the latest) so the worker is not flooded.

## Error handling

- Decode failure, GPU init failure, or render failure surface as a status-bar / toast message; the
  UI never panics.
- GPU-over-limit: the engine already returns the unprocessed image rather than crashing; the UI
  shows it as-is.
- Missing/unreadable folders: empty grid with a message, no crash.

## Testing

- `rapidraw-core`: one smoke test for `render()` — a solid mid-gray image + an exposure bump → assert
  output mean luminance increased by roughly the expected amount. Asserts the GPU path runs end to
  end. (Skips gracefully if no GPU adapter is available in CI.)
- `rapidraw-relm4`: manual run verification (open folder, adjust, export). No UI test harness in this
  spec.

## Risks / open ceilings

- **wgpu version skew:** core must use one wgpu version; both `src-tauri` and `rapidraw-relm4` inherit
  it. If GTK/relm4 pulls a conflicting wgpu transitively, pin via core.
- **GTK4 dev libraries** must be present on the build host (gtk4, gdk-pixbuf). Documented in the
  relm4 crate README.
- **Decoupling scope creep:** extracting `gpu_processing`/`image_processing` may pull in more shared
  helpers than expected. Mitigation: move only what the minimal loop links; leave the rest in
  src-tauri behind its existing wrappers.

## Future specs (not now)

RAW tuning UI, masks, crop/geometry UI, AI, panorama, culling, presets, multi-format export,
removal of `src-gpui`, GL zero-copy rendering bridge.
