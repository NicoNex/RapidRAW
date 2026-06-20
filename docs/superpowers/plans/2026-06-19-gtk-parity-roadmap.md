# GTK (relm4) → Tauri Parity Roadmap

**Goal:** bring the relm4/libadwaita UI to feature parity with the Tauri/React UI
**without reimplementing core functionality.** Shared logic moves into
`rapidraw-core` and is reused by both frontends; Tauri-specific glue stays in
`src-tauri` as thin wrappers. Default values must match the React reference
(`src/utils/adjustments.ts` → `INITIAL_*` constants).

Use **libadwaita** widgets wherever they fit (`AdwPreferencesGroup`,
`AdwExpanderRow`, `AdwActionRow`, `AdwSpinRow`, `AdwComboRow`, `AdwToastOverlay`,
`AdwDialog`/`AdwAlertDialog`, `AdwViewStack` for the right-panel switcher).

## Architecture rule (do not break)

- `rapidraw-core` — shared engine. relm4 links **only** this.
- `src-tauri` (`rapidraw_lib`) — Tauri backend; AI + thin command wrappers.
- `rapidraw-relm4` — GTK frontend.

Any feature with shared logic: move the logic to core first, then wire both UIs.

## Current state (done)

- **Masks foundation (P0+P1):** mask data model + non-AI rasterizers live in
  `rapidraw-core/src/mask_generation.rs`. `render()` rasterizes masks → GPU
  atlas + per-mask adjustments. relm4 `Session` carries `Vec<MaskDefinition>`,
  threaded through the render worker. Engine already fully supports masks
  (atlas upload, per-mask shader, `get_mask_adjustments_from_json`).
  **Gap: no masks UI in relm4 yet** — `session.masks` is always empty.
- **relm4 panels present:** controls, crop, curves, colorwheel, scopes, meta,
  settings, library, sidecar. Basic export (save, no options panel).

## Relm4 vs React gap (inventory)

React right-panels: AIPanel, ControlsPanel ✅, CropPanel ✅, ExportPanel ❌,
MasksPanel ❌, MetadataPanel ✅, PresetsPanel ❌, SettingsPanel ✅.

React modals (all ❌ in relm4): Collage, Culling, Denoise, Hdr, ImportSettings,
LensCorrection, NegativeConversion, Panorama, Transform, CopyPasteSettings,
ConfigurePreset. Plus Community page.

97 Tauri commands define the full feature surface.

---

## Phases

### P2 — Masks UI: list + per-mask adjustments (non-AI) ✅ DONE

Wire the existing foundation to the screen. No canvas interaction yet.
Implemented in `rapidraw-relm4/src/masks.rs` (Masks tab, add/delete/select,
visibility/invert/opacity, scalar Basic/Color/Details/Effects sliders). Curves/
HSL/colour-grading per-mask sliders + sub-mask mode UI remain for a follow-up.

- New `masks` module + right-panel tab (`AdwViewStack` page).
- Mask list: `AdwPreferencesGroup` of `AdwExpanderRow` rows (one per
  `MaskDefinition`): visibility toggle, name, opacity, invert, delete.
- "Add mask" → menu of non-AI sub-mask types (radial, linear, brush, color,
  luminance, all). New `MaskDefinition` seeded with React defaults.
- Per-mask adjustments: reuse the existing controls widget bound to the mask's
  `adjustments` JSON (same sliders as global). Edits mutate `session.masks` →
  trigger re-render.
- Sub-mask mode (Additive/Subtractive/Intersect) selector per sub-mask.

Defaults source-of-truth: React `INITIAL_MASK_ADJUSTMENTS`.
Acceptance: add a radial mask, change exposure, see only the masked region change.

### P3 — Mask geometry via numeric controls ✅ DONE

Edit sub-mask geometry without the canvas (parity-incremental, easy first).
Implemented in `masks.rs::submask_editor` (AdwPreferencesGroup per sub-mask):

- Radial: centerX/Y, radiusX/Y, rotation, feather (`AdwSpinRow`). ✅
- Linear: start/end points, range. ✅
- Color/Luminance: `ParametricMaskParameters` (target, tolerance, grow, feather). ✅
- Compositing **Mode** combo (Additive/Subtractive/Intersect) per sub-mask. ✅
- Brush/Flow: canvas-driven — hint shown, editor deferred to P4.
- Defaults verified against React `SUB_MASK_CONFIG` (display/stored multiplier).

### P4 — Canvas interaction + persistence (partial)

- ✅ **Sidecar persistence**: `Edits.masks` (camelCase, Tauri-compatible) — masks
  survive reopen.
- ✅ **Undo/redo**: `HistEntry.masks`; mask mutations record + autosave.
- ✅ **Multi sub-mask management**: add/delete/visibility/invert per sub-mask.
- ✅ **Read-only canvas overlay**: draws selected radial/linear shapes, glued to
  the photo under zoom/pan (`editor.rs` mask overlay layer).
- ⬜ **Interactive canvas drag**: place/resize radial & linear, brush/flow stroke
  capture (make the overlay targetable + gesture handlers, like crop mode).
- ⬜ Live grayscale mask preview (tinted bitmap) — optional.

### P4b — Per-mask non-scalar adjustments ✅ DONE

Scalar Basic/Color/Details/Effects done (P2). Per-mask now also:
- ✅ Curves: `CurveEditor::with_sink` (seeded from JSON, emits `MaskCurve`),
  writes `adjustments.curves.<channel>` JSON (`masks.rs::build_mask_curves`).
- ✅ HSL mixer (`build_mask_hsl`).
- ✅ Color grading wheels (`build_mask_grading`).
Defaults verified against React `INITIAL_MASK_ADJUSTMENTS`.

### P5 — AI masks: extract inference to core

Coupling is shallow (10 refs, only `AppHandle` for `models_dir` + progress
emit). Move ONNX inference (`ort`) into `rapidraw-core` behind a feature/seam;
`src-tauri` keeps model-download + progress glue.

- Core: AI sub-mask resolver (SAM/subject, u2net/foreground, skyseg, depth).
  relm4 injects its own model-dir + progress channel via the existing
  `AiResolver` closure seam.
- relm4 AIPanel tab: generate subject/foreground/sky/depth masks, quick-eraser.
- Model download UX (libadwaita progress in an `AdwDialog`).
- Generative replace / inpaint (lama) — separate sub-task, heaviest.

### P6 — Export options panel

ExportPanel parity (`AdwPreferencesGroup`): format, quality, resize, keep
metadata, filename template, watermark. Core export already exists; this is UI +
wiring options into the existing export path.

### P7 — Presets

PresetsPanel + ConfigurePreset dialog. Presets are adjustment JSON (incl.
masks); load/apply/save. Mostly UI; serialization already in core.

### P8 — Tooling modals (incremental, by demand)

Each is a libadwaita `AdwDialog`/`AdwAlertDialog` over a core function. Move any
missing core logic out of `src-tauri` first. Order by user demand:

- Transform (rotate/flip/straighten) — likely cheap, do early.
- LensCorrection, NegativeConversion, Denoise, Hdr, Panorama, Collage.
- Culling + tagging, ImportSettings, CopyPasteSettings.
- Community page — lowest priority (network/social, not editing).

---

## Notes

- Don't reimplement core logic in relm4. If a feature's logic still lives in
  `src-tauri`, the first step of its phase is **move to core**.
- Every new control's default must match the React `INITIAL_*` constant. When in
  doubt, grep `src/utils/adjustments.ts`.
- Phases P2–P4 are the masks UI (the immediate next work); P5+ are the broader
  parity backlog.
