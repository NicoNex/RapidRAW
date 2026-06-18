# RapidRAW — original editor feature map vs. relm4 port

Tracks every feature of the original React/Tauri UI (`src/`) against the native
GTK4 + libadwaita port (`rapidraw-relm4/`). Update the **Status** column as
things land.

Legend: ✅ done · ⚠️ partial · ❌ missing · ➖ N/A (won't port)

---

## Right-side panel switcher (original: `RightPanelSwitcher.tsx`)

The original right rail switches between several panels. We currently show only
the adjustments panel, always.

| Panel | Status | Notes |
|------|--------|-------|
| Adjustments (sliders) | ✅ | `controls.rs` |
| Crop | ✅ | `crop.rs` + right-rail Edit/Crop switcher + interactive crop rectangle on canvas |
| Masks | ❌ | AI/radial/linear/brush masks; big |
| AI (inpaint/generative) | ❌ | needs model backend; likely ➖ for now |
| Presets | ❌ | save/apply adjustment presets |
| Export | ✅ | dialog with all formats (JPEG/PNG/TIFF/WebP/JPEG XL/AVIF/CUBE LUT) + per-format settings; ⚠️ no watermark/filename-template/keep-metadata/preset list |
| Metadata (EXIF) | ❌ | read + show EXIF |

**TODO:** add a panel switcher (right-aligned icon rail like the original) so
Adjustments / Crop / (later) Masks etc. are selectable.

---

## Adjustments

### Basic (`adjustments/Basic.tsx`)
| Control | Status |
|--------|--------|
| Exposure, Contrast, Highlights, Shadows, Whites, Blacks | ✅ `controls.rs BASIC` |

### Curves (`adjustments/Curves.tsx`)
| Feature | Status | Notes |
|--------|--------|-------|
| Point curve editor (Luma/R/G/B) | ✅ | `curves.rs` |
| **Parametric curve** (highlights/lights/darks/shadows + black/white level) | ✅ | `curves.rs` Point/Parametric mode toggle; splits fixed at 25/50/75 ⚠️ |
| Copy/paste curve | ✅ | `curves.rs` per-channel clipboard (points + parametric) |

### Color (`adjustments/Color.tsx`)
| Control | Status | Notes |
|--------|--------|-------|
| White Balance: Temperature, Tint | ✅ | gradient tracks ✅ (`slider.rs`) |
| Presence: Vibrance, Saturation, Hue | ✅ | Hue gradient track ✅ |
| Color Grading wheels (Shadows/Midtones/Highlights/Global) | ✅ | `colorwheel.rs` |
| Color Grading: Blending, Balance | ✅ | |
| HSL 8 bands (Hue/Sat/Lum) | ✅ | Hue + Sat + Lum gradient tracks ✅ (sat/lum follow live band hue/sat) |
| Calibration (shadow tint, R/G/B hue+sat) | ✅ | |

### Details (`adjustments/Details.tsx`)
| Control | Status |
|--------|--------|
| Sharpness, Sharpness Threshold, Clarity, Dehaze, Structure, Centre, Luma NR, Color NR, Chromatic Aberration R/C + B/Y | ✅ |

### Effects (`adjustments/Effects.tsx`)
| Control | Status |
|--------|--------|
| Glow, Halation, Light Flares | ✅ |
| Vignette (Amount/Midpoint/Roundness/Feather) | ✅ |
| Grain (Amount/Size/Roughness) | ✅ |

---

## Slider behaviour (`ui/Slider.tsx`)
| Feature | Status | Notes |
|--------|--------|-------|
| Fill from **default** position (centre-origin for bipolar) | ✅ | `slider.rs` |
| Gradient tracks (temp/tint/hue/HSL hue+sat+lum) | ✅ | colours from `styles.css` |
| Reset: dedicated button + double-click + label-click | ✅ | `slider.rs` |
| Drag, wheel (forwarded to scroll panel) | ✅ | |
| Shift = fine adjust | ✅ | |
| Click value to type exact number | ✅ | |

---

## Crop panel (`panel/right/CropPanel.tsx`) — ✅ (`crop.rs`, overlay in `editor.rs`)
- Aspect presets: Free, 1:1, 5:4, 4:3, 3:2, 16:9, 21:9, 65:24 — ✅ (constrains the rectangle). Original/swap-orientation ❌
- Rotation (90° steps): RotateCw / RotateCcw — ✅
- Flip Horizontal / Vertical — ✅
- Straighten (angle slider) — ✅
- Crop rectangle drag handles on canvas — ✅ (move + 4 corners + 4 edges, aspect-locked corners)
- Rule-of-thirds grid + dimmed exterior — ✅

Crop is interactive: entering the Crop panel shows the full image with an overlay
(`EditorCanvas::enter_crop`); the rect is committed to `Geometry.crop` on leaving.
Geometry applied to the base (CPU) in the render worker before GPU render
(`apply_geometry`). ⚠️ Remaining: aspect lock on edge (not corner) drags;
geometry not in undo history; "Reset crop" also clears rotate/flip and the panel
toggles don't visually reset.

---

## Editor toolbar (`panel/editor/EditorToolbar.tsx`)
| Feature | Status | Notes |
|--------|--------|-------|
| Back to library | ✅ | |
| **Undo / Redo** (Ctrl+Z / Ctrl+Shift+Z, Ctrl+Y) | ✅ | `main.rs` history stack; ⚠️ colour-wheel/curve UI widgets don't re-sync on undo (render is correct) |
| Adjustments history dropdown | ❌ | |
| **Show original** (before/after toggle) | ✅ | toolbar toggle, preview-size original |
| Fullscreen | ❌ | |
| EXIF readout (shutter/aperture/iso/focal/date) | ✅ | `meta.rs` (kamadak-exif), shown right in toolbar |
| Copy / paste settings between photos | ❌ | |

---

## Library / home (`panel/MainLibrary.tsx`, `hooks/useSortedLibrary.ts`)
| Feature | Status | Notes |
|--------|--------|-------|
| Folder open + thumbnail grid | ✅ | `main.rs`, `thumb.rs` |
| **Continue session** (reopen last folder) | ✅ | last folder persisted to `$XDG_CONFIG_HOME/rapidraw-relm4/last_folder`; button on welcome |
| **Splash / default photo** on home | ✅ | embedded `splash-grey.jpg` welcome screen (`.osd` card, brand, Open/Continue) |
| **Raw filter**: All / Raw only / Non-raw only / Prefer raw | ✅ | `library.rs arrange`; toolbar DropDown |
| Sort (name/date) | ✅ | name + date newest/oldest; toolbar DropDown |
| Sort by rating/EXIF | ❌ | needs ratings/EXIF index |
| Star ratings | ❌ | needs a persisted store |
| Search | ❌ | |
| Folder tree sidebar | ❌ | |

---

## Other panels / managers (mostly ➖ or later)
- Masks (AI + manual) — ❌, large
- Presets manager + community presets — ❌
- Metadata panel — ❌
- AI inpaint / generative — ➖ (needs model backend)
- Settings — ✅ (`settings.rs`): background, preview/thumb size, reset-on-open

---

## Active request queue (2026-06-18 batch)
1. ✅ Window no longer grows on zoom / stays manually resizable (`editor.rs`).
2. ✅ Reliable slider reset (double-click + label-click).
3. ✅ Slider centre/default-origin fill (no misleading half-fill).
4. ✅ Gradient tracks: temp, tint, presence hue, HSL band hue. ⚠️ HSL sat/lum still plain.
5. ❌ Default/splash photo on the home screen.
6. ❌ Continue session (reopen last folder).
7. ❌ Library raw filter (all / raw only / non-raw / prefer raw).
8. ✅ Curves: parametric mode.
9. ❌ Crop panel/section (right-rail switcher) — **next**.
10. ✅ Undo/redo (Ctrl+Z / Ctrl+Shift+Z) + show-original toggle.
11. ✅ This file.
12. ✅ Slider: shift=fine, click-value-to-type, dedicated reset button, HSL sat/lum gradients.
13. ✅ EXIF readout in editor toolbar.
14. ✅ Show-original icon swap; nicer Paned resize grip.
15. ✅ Copy/paste curves.
16. ✅ Library: splash welcome, continue session, raw filter, sort.
17. ✅ Slider look tuned to Adwaita accent.
18. ✅ Crop panel + Edit/Crop switcher + interactive crop rectangle (move/corners/edges, thirds grid, dimmed exterior).
19. ✅ Per-image edit memory (`sidecar.rs`): adjustments + geometry + LUT persisted per image, restored on reopen. Setting "Reset adjustments on open" (default off) forces a fresh start.
20. ✅ Slider gradient tracks no longer overlaid by the accent fill.
21. ✅ Crop drag no longer pans the preview; "Reset crop" resets the panel controls too.
22. ✅ Welcome screen redesigned (full-bleed splash + scrim + pill buttons, no boxed card).
23. ✅ Manual value entry (Enter) no longer scrolls the panel.
24. ✅ Edit/Crop moved to top tabs (icons) instead of a side rail.
25. ✓ Vignette verified identical to the original (scales + shader match); no change needed.
