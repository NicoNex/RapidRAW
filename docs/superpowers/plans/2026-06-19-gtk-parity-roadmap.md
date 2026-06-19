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

### P3 — Mask geometry via numeric controls

Edit sub-mask geometry without the canvas (parity-incremental, easy first).

- Radial: centerX/Y, radiusX/Y, rotation, feather (`AdwSpinRow`).
- Linear: start/end points, range.
- Color/Luminance: `ParametricMaskParameters` (tolerance, feather, range).
- Brush/Flow: list of lines is canvas-driven — defer the editor to P4, but allow
  clear/delete here.

### P4 — Canvas interaction + persistence

- Overlay drawing on the image canvas: drag to place/resize radial & linear,
  brush/flow stroke capture. (relm4 `gtk::DrawingArea` over the preview.)
- Live mask overlay preview (show the grayscale mask tinted).
- Sidecar persistence: masks already serialize (camelCase JSON contract matches
  Tauri sidecars). Verify round-trip load/save in relm4 `sidecar` module.

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
