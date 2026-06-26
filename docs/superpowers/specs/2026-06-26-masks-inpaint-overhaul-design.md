# Masks & Inpaint panel overhaul (relm4) — design

Date: 2026-06-26
Branch: `feat/inpaint` (continue) or new `feat/masks-overhaul`

## Goal

Bring the relm4 Masks and Inpaint right-rail panels up to parity with the
original Tauri React UI (`src/components/panel/right/`) in layout and UX. Two
named asks drive this:

1. The Masks panel should use the same **card-grid create buttons** as the
   Inpaint panel (currently a flat "Add mask" dropdown).
2. The Inpaint panel must let you **draw a region on the canvas to trigger the
   AI** (machinery is wired but was never run in the GTK app — likely a runtime
   bug).

Scope: "asks + UX parity pass", not full pixel-faithful port.

## Reference (Tauri source)

- `src/components/panel/right/Masks.tsx` — type tables: `MASK_PANEL_CREATION_TYPES`,
  `OTHERS_MASK_TYPES`, `AI_PANEL_CREATION_TYPES`, `MASK_ICON_MAP`, name helpers.
- `src/components/panel/right/MasksPanel.tsx` — masks layout, container rows,
  context menu (rename/duplicate/duplicate+invert/copy/paste/delete), settings.
- `src/components/panel/right/AIPanel.tsx` — inpaint create grid, BrushTools
  (size/feather + Add/Erase segmented), generate flow.

## relm4 files touched

- `rapidraw-relm4/src/masks.rs` — panel UI (rebuild).
- `rapidraw-relm4/src/inpaint.rs` — panel UI.
- `rapidraw-relm4/src/main.rs` — `AppMsg` + handlers + `AppModel` state.
- `rapidraw-relm4/src/editor.rs` — canvas draw/arm (only if Phase 2 needs it).

## In scope

- Masks card-grid create UI (3-col) + "Others" popover.
- Grid-when-empty / list-when-populated, "Add new mask" row, reset-all header button.
- Per-type icons on sub-mask rows (reuse the relm4-icons mapping).
- Inpaint canvas drawing fix (diagnose + fix at runtime).
- Brush Add/Erase as a segmented toggle (match Tauri).
- Inline rename of mask containers (double-click / edit affordance).
- Copy / paste mask + duplicate + duplicate-and-invert (in-app, via model state).

## Out of scope (deliberate, YAGNI)

DnD drag-reorder, waveform/analytics panel, cloud connection status, paste-
adjustments-only, sub-mask copy/paste. Add later if wanted.

## Phases

### Phase 1 — Masks create-grid + list restructure (do first; pure UI)

- Add `MASK_CREATE_GRID: &[(label, type, icon)]` and `OTHERS_TYPES` tables in
  `masks.rs`, mirroring `MASK_PANEL_CREATION_TYPES` / `OTHERS_MASK_TYPES`.
  Icons via the existing relm4-icons set (extend `icons.toml` if a glyph is
  missing; reuse `inpaint::tool_icon` style).
- New `create_grid(sender)` in `masks.rs` modeled on `inpaint::create_grid`, 3
  columns. The "Others" card opens a `gtk::Popover` listing `OTHERS_TYPES`,
  each emitting `AddMask(ty)`.
- `MasksPanel::rebuild`:
  - masks empty → "Create New Mask" heading + grid (replaces `add_menu`).
  - masks present → "Masks" heading + container list + an "Add new mask" row
    (a `gtk::MenuButton` whose popover contains the same `create_grid`).
- Header row above the body: panel title + a **reset-all-masks** button
  (`RotateCcw`/trash icon) emitting a new `AppMsg::ResetAllMasks`.
- Keep the existing selected-mask detail card (adjustments/HSL/curves/grading/
  sub-mask editors) unchanged.

### Phase 2 — Inpaint canvas drawing fix (needs the running app)

- User runs the GTK app, selects an inpaint create tool (e.g. Brush / Subject),
  attempts to draw on the canvas, reports behaviour (no stroke? no box? panic in
  stderr? nothing armed?).
- Diagnose from report + `editor.rs` arm/draw code (`set_mask_draw`, `ArmPick`,
  `ArmPaint`, `AddBrushStroke`, the `GestureDrag` handlers) and `main.rs`
  `AddSubMask` auto-arm branch (main.rs ~2776).
- Fix the specific defect. Likely candidates: arm flag not reaching the canvas,
  `edit_patch` routing, draw gesture pre-empted by pan, or overlay not redrawn.
- Verify: drawing a region then Generate produces a patch result.

### Phase 3 — UX parity polish

- Sub-mask rows: per-type icon prefix + tighter row layout (match Tauri's nested
  sub-mask item).
- Brush controls: replace the size/feather SpinRows' Add/Erase switches with a
  2-button **segmented toggle** (Add | Erase), Tauri-style; keep size/feather.
- Inline **rename**: double-click a container name swaps the label for a
  `gtk::Entry`; commit on Enter/focus-out via new `AppMsg::RenameMask(i, String)`.
- Container **context menu** (right-click row): Rename, Duplicate,
  Duplicate & Invert, Copy mask, Paste mask, Delete. Backed by:
  - `AppModel.copied_mask: Option<MaskDefinition>`.
  - `AppMsg::CopyMask(i)`, `PasteMask`, `DuplicateMask(i)`,
    `DuplicateMaskInvert(i)`. Duplicate clones the `MaskDefinition` with fresh
    `next_id` for container + every sub-mask (mirror Tauri `cloneMaskContainerData`);
    invert flips `invert`.

## Data flow

All mutations go through existing `AppMsg` → `AppModel` → `rebuild_active` →
`RequestRender` plumbing. New messages follow the same pattern; new state
(`copied_mask`) lives on `AppModel`, not persisted to the sidecar.

## Testing

- `masks.rs` already has unit tests for default geometry; add a `clone_mask`
  test asserting fresh IDs + preserved adjustments (and inverted flag for the
  invert variant). No GTK harness — UI verified by the user running the app.
- Phase 2 verified manually (user runs, confirms draw→generate works).

## Risks

- Phase 2 is a runtime unknown; can't be fully designed until the symptom is
  observed. Treated as diagnose-then-fix, not a pre-planned change.
- Icon glyphs: some mask types may lack a bundled relm4-icon; fall back to a
  near match or a symbolic GTK icon rather than adding many new SVGs.
