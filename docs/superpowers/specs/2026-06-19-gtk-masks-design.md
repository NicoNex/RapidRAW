# GTK Masks — Design Spec

Date: 2026-06-19
Branch: `feat/masks`

## Goal

Bring the relm4/GTK UI (`rapidraw-relm4`) to parity with the Tauri/React UI for
**local adjustment masks**. Per the project rule ([[project_gtk_parity]]): do not
reimplement shared logic — move it into `rapidraw-core` and reuse it from both
frontends. Tauri-specific glue stays in `src-tauri` as thin wrappers.

## Non-goals (this spec)

- **AI masks** (subject / sky / depth / foreground) and AI patches. These need the
  ONNX inference subsystem (`ort`, model download/cache, SAM prompting) extracted
  from `src-tauri` into core. Separate spec/phase. The data model leaves room for
  them but no AI code is built here.
- Generative replace / inpainting (LaMa), denoise — same reason, later.

## What already exists (no rebuild)

- **Engine supports masks fully.** `rapidraw-core::gpu_processing::run()` accepts
  `mask_bitmaps: &[ImageBuffer<Luma<u8>>]`, builds the mask atlas texture
  (`gpu_processing.rs:798–853`), and the shader applies per-mask adjustments
  (`shader.wgsl:1498+`).
- **Per-mask adjustment struct + parser in core.** `MaskAdjustments` (32-slot array
  in `AllAdjustments`, `image_processing.rs:1355`) and
  `get_mask_adjustments_from_json(&Value) -> MaskAdjustments`
  (`image_processing.rs:2165`). relm4 can build each mask's `MaskAdjustments`
  straight from a JSON `adjustments` value — for free.
- **relm4 render thread already carries `AllAdjustments`** (`main.rs:268`, RenderJob)
  and owns a cached `GpuProcessor`. It currently passes no masks.

## What is missing / what we build

### A. Move non-AI mask rasterization into core
`src-tauri/src/mask_generation.rs` (1511 lines) holds the data model + rasterizers.
The **non-AI** parts move into a new `rapidraw-core::mask_generation` module:

- Types: `SubMaskMode`, `SubMask`, `MaskDefinition`, `GrowFeatherParameters`,
  `RadialMaskParameters`, `LinearMaskParameters`, `BrushMaskParameters`/`BrushLine`,
  `FlowMaskParameters`/`FlowLine`, `ParametricMaskParameters`, `Point`.
- Rasterizers: `generate_radial_bitmap`, `generate_linear_bitmap`,
  `generate_brush_bitmap`, `generate_flow_bitmap`, `generate_color_bitmap`,
  `generate_luminance_bitmap`, `generate_all_bitmap`, the stroke helpers
  (`render_stroke_layer_parallel`, `stroke_bounds`, `grayscale_dilate/erode`,
  `apply_grow_and_feather`), `generate_sub_mask_bitmap`, `generate_mask_bitmap`.
- AI rasterizers (`generate_ai_*`) **stay in `src-tauri`** for now. `generate_sub_mask_bitmap`
  in core returns `None` (or skips) for `ai-*` sub-mask types; `src-tauri` keeps its
  own wrapper that handles those by delegating to core for non-AI and to its AI path
  for AI types. (Rasterization deps `image`, `imageproc`, `rayon`, `base64`, `serde_json`
  are already in `rapidraw-core/Cargo.toml` — P0 adds no new dependencies.)

`src-tauri/mask_generation.rs` becomes a thin re-export + AI-only layer. React/Tauri
behaviour unchanged — same functions, new home. `generate_mask_overlay` (a
`#[tauri::command]`) stays in `src-tauri`.

### B. relm4 mask state + render wiring
- Add `masks: Vec<MaskDefinition>` to the relm4 session/editor state. The active image's
  masks persist via the existing sidecar (extend it to store the masks JSON alongside the
  `GlobalAdjustments` bytes).
- In the render thread: for each visible mask, call `generate_mask_bitmap(scale, crop_offset, …)`
  at the current preview dimensions → `Vec<mask_bitmaps>`; set `adj.mask_count` and
  `adj.mask_adjustments[i] = get_mask_adjustments_from_json(&mask.adjustments)`; pass
  `mask_bitmaps` into `GpuProcessor.run`. Rasterize on the render thread (depends on
  preview scale); cache per (mask, dims) to keep slider drags smooth.
- `color`/`luminance` masks need the warped image (`MaskDefinition::requires_warped_image`);
  the render thread already has it — pass it through.

### C. relm4 Masks panel UI (mirrors `src/components/panel/right/Masks.tsx`)
- A right-rail Masks section (new entry in the panel switcher next to adjustments/crop).
- Mask list: add / delete / duplicate / select / rename / visibility toggle / invert /
  opacity. "Add mask" → choose type (radial, linear, brush, color, luminance).
- Selected mask → sub-mask list (each: type, mode Additive/Subtractive/Intersect,
  visible, invert, opacity) + the per-mask **adjustments** sub-panel.
- Per-mask adjustments reuse the existing `controls.rs` slider tables, but bound to the
  mask's adjustment values serialized to a JSON `Value` (consumed by
  `get_mask_adjustments_from_json`). Only the subset present in `MaskAdjustments`
  (no vignette/grain/etc.) is shown — matching the React mask panel.

### D. Sub-mask geometry editing — numeric first, canvas second (incremental)
Per the incremental directive (easy → hard):
1. **Numeric editing** for every sub-mask type (fields/sliders): radial (center, radius
   x/y, rotation, feather), linear (start/end, range), color/luminance (target, tolerance,
   range, grow, feather), brush (size, feather — strokes come from canvas only).
2. **Canvas interaction** on the image overlay: drag radial ellipse + handles, drag linear
   endpoints, freehand brush strokes (capture `points` into `BrushLine`). relm4's editor
   already has a `gtk::DrawingArea` overlay (`editor.rs:447 apply`) to build on.

## Defaults (must match reference)

Source of truth = React `src/utils/adjustments.ts` `INITIAL_MASK_ADJUSTMENTS` /
`INITIAL_*` and the Rust `default_*` fns / `Default` impls in `mask_generation.rs`
(e.g. `opacity 100`, linear `range 50`, brush `feather 0.5`, flow `flow 10`,
parametric `tolerance 20`/`feather 35`). These two already mirror each other. We encode
the defaults once in core and add a test asserting the relm4-built defaults equal the
React constants (extracted as a small fixture). Same approach already used for the global
Blending=50 fix.

## Phasing (each phase shippable + testable)

- **P0** — Move non-AI mask generation into `rapidraw-core`; `src-tauri` re-exports;
  confirm Tauri build + React still render masks identically.
- **P1** — relm4 render wiring: rasterize masks → atlas → engine. Prove with a mask loaded
  from a sidecar (no UI yet): image renders with the mask applied.
- **P2** — Masks panel: list + per-mask adjustments (numeric), sub-mask list with
  mode/invert/opacity. Radial + linear types.
- **P3** — Numeric geometry for color + luminance + brush params.
- **P4** — Canvas interaction (drag radial/linear, draw brush).
- **P5** — *(separate spec)* AI masks: extract inference to core, relm4 worker + SAM prompting.

## Parallelization

- P0 is a prerequisite for everything (blocks).
- After P0: panel UI scaffolding (C), render wiring (B), and per-type numeric editors (D1)
  are largely independent and can proceed in parallel.
- Canvas interaction (D2) per geometry type (radial / linear / brush) are independent of
  each other.

## Testing

- Core rasterizer unit tests: small fixed-size bitmaps for radial/linear/brush/color/
  luminance with `assert`s on known pixels (centre = 255, far corner = 0, feather
  monotonic). One self-check per rasterizer.
- JSON round-trip: `MaskDefinition` serialize→deserialize stable; matches a captured
  React-emitted mask JSON fixture.
- Defaults-match test: relm4 default mask adjustments == React `INITIAL_MASK_ADJUSTMENTS`.

## Risks / open points

- **Rasterization cost on the render thread.** Brush/flow over full preview can be slow;
  mitigate with per-(mask,dims) bitmap cache + dirty flags. `requires_warped_image` masks
  re-rasterize when geometry changes only.
- **Sidecar format change.** Adding masks JSON to the relm4 sidecar — keep
  backward-compatible (absent = no masks).
- **AI seam.** `generate_sub_mask_bitmap` must cleanly fork AI vs non-AI so core never
  references `ai_processing`. Verified shallow: only `generate_ai_*` touch AI.
