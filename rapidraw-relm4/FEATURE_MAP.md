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
| Crop | ❌ | aspect presets, rotation, flip, straighten — see Crop below |
| Masks | ❌ | AI/radial/linear/brush masks; big |
| AI (inpaint/generative) | ❌ | needs model backend; likely ➖ for now |
| Presets | ❌ | save/apply adjustment presets |
| Export | ⚠️ | export works (dialog); not a panel, no preset list |
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
| **Parametric curve** (highlights/lights/darks/shadows + black/white level + splits) | ❌ | per-channel `ParametricCurveSettings`; original blends parametric into the point curve |
| Copy/paste curve | ❌ | |

### Color (`adjustments/Color.tsx`)
| Control | Status | Notes |
|--------|--------|-------|
| White Balance: Temperature, Tint | ✅ | gradient tracks ✅ (`slider.rs`) |
| Presence: Vibrance, Saturation, Hue | ✅ | Hue gradient track ✅ |
| Color Grading wheels (Shadows/Midtones/Highlights/Global) | ✅ | `colorwheel.rs` |
| Color Grading: Blending, Balance | ✅ | |
| HSL 8 bands (Hue/Sat/Lum) | ✅ | Hue band gradient ✅; Sat/Lum gradients ⚠️ (still plain — they depend on live hue/sat, need CSS-var-style dynamic ramp) |
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
| Gradient tracks (temp/tint/hue/HSL-hue) | ✅ | colours copied from `styles.css`; HSL sat/lum still plain ⚠️ |
| Double-click / label-click to reset | ✅ | `slider.rs` |
| Drag, wheel (forwarded to scroll panel) | ✅ | |
| Shift = fine adjust | ❌ | |
| Click value to type exact number | ❌ | |

---

## Crop panel (`panel/right/CropPanel.tsx`) — ❌ all missing
- Aspect presets: Free, Original, 1:1, 5:4, 4:3, 3:2, 16:9, 21:9, 65:24 (+ swap orientation)
- Rotation (90° steps): RotateCw / RotateCcw
- Flip Horizontal / Vertical
- Straighten (angle slider, live rotation overlay)
- Crop rectangle drag handles on canvas
- Grid overlays (rule of thirds, etc.)

Engine: `rapidraw-core` adjustments already carry `aspectRatio, rotation,
flipHorizontal, flipVertical, orientationSteps` — wire UI to them.

---

## Editor toolbar (`panel/editor/EditorToolbar.tsx`)
| Feature | Status | Notes |
|--------|--------|-------|
| Back to library | ✅ | |
| **Undo / Redo** (Ctrl+Z / Ctrl+Shift+Z) | ❌ | snapshot the adjustments struct on each change |
| Adjustments history dropdown | ❌ | |
| **Show original** (before/after toggle) | ❌ | render base (or default adjustments) while held/toggled |
| Fullscreen | ❌ | |
| EXIF readout (shutter/aperture/iso/focal/date) | ❌ | |
| Copy / paste settings between photos | ❌ | |

---

## Library / home (`panel/MainLibrary.tsx`, `hooks/useSortedLibrary.ts`)
| Feature | Status | Notes |
|--------|--------|-------|
| Folder open + thumbnail grid | ✅ | `main.rs`, `thumb.rs` |
| **Continue session** (reopen last folder) | ❌ | persist last folder path, button on home |
| **Splash / default photo** on home | ❌ | `public/splash-*.jpg` (light/grey/dark) as welcome background |
| **Raw filter**: All / Raw only / Non-raw only / Prefer raw | ❌ | `RawStatus` enum in original |
| Sort (name/date/rating/EXIF) | ❌ | |
| Star ratings | ❌ | |
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
8. ❌ Curves: parametric mode.
9. ❌ Crop panel/section (right-rail selectable).
10. ❌ Undo/redo (Ctrl+Z / Ctrl+Shift+Z) + show-original toggle.
11. ✅ This file.
