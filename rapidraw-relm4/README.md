# RapidRAW — native GTK4 / libadwaita frontend

A native desktop frontend for RapidRAW built with **relm4 0.9 + GTK4 + libadwaita**,
driving the tauri-free `rapidraw-core` image engine (wgpu GPU pipeline). It is an
alternative to the React/Tauri UI in `../src`.

See [`FEATURE_MAP.md`](FEATURE_MAP.md) for the live parity checklist against the
original UI (what's done / partial / missing).

## Build & run

```sh
cd rapidraw-relm4
cargo run --release
```

Needs the system GTK4 + libadwaita (≥1.4) dev libraries. Encoders pulled in for
export: `webp`, `jxl-encoder`, and `image` with `avif-native` (rav1e).

State lives under `$XDG_CONFIG_HOME/rapidraw-relm4/` (last folder, per-image
edits, ratings) and `$XDG_CACHE_HOME/rapidraw-relm4/thumbs/` (thumbnail cache).

## Architecture

Single relm4 `Component` (`AppModel`) plus a few plain owned widgets.

| Module | Responsibility |
|--------|----------------|
| `main.rs` | `AppModel`, messages, window/HeaderBar/NavigationView, render & thumbnail workers, export, persistence helpers |
| `editor.rs` | `EditorCanvas`: zoom/pan picture in a `Fixed` inside a `ScrolledWindow`, plus the interactive crop overlay layer |
| `controls.rs` | `AdjustPanel`: all adjustment sections (Basic/Color/Details/Effects/LUT) as `.card`s; HSL swatch mixer |
| `slider.rs` | Custom slider (centre-fill, gradient tracks, shift-fine, type-to-edit) + undo/reset registries |
| `curves.rs` | Tone-curve editor: point + parametric modes, copy/paste |
| `colorwheel.rs` | Colour-grading wheels (hue/sat disc + luminance) |
| `crop.rs` | Crop panel: aspect presets, rotate/flip/straighten |
| `scopes.rs` | Histogram / waveform / vectorscope |
| `library.rs` | Folder scan, raw filter, sort, search arrange; texture helper |
| `thumb.rs` | Thumbnail grid cell (factory) with star rating |
| `thumb_cache.rs` | On-disk thumbnail cache (hash of path+mtime+dim) |
| `meta.rs` | EXIF summary (kamadak-exif) |
| `sidecar.rs` | Per-image edit persistence (adjustments + geometry + LUT) |
| `settings.rs` | Preferences (`adw::PreferencesWindow`) |

### Key design decisions

- **Single render thread** owns the cached `GpuProcessor` (thread-local in
  `rapidraw_core::render`), so the shader compiles once per image size, not per
  frame. Preview jobs coalesce to the latest; exports always run.
- **Geometry** (rotate/flip/straighten/crop) is applied to the base image on the
  CPU in the render worker *before* the GPU pass (`apply_geometry`); cheap unless
  free-straighten is active.
- **Thumbnails** decode on a CPU-sized work-stealing thread pool, write a small
  JPEG to the cache, and short-circuit on the cache on reopen/filter/restart. A
  generation token cancels in-flight decodes when leaving the library.
- **Per-image edits** are reset *in place* on open (no panel rebuild) for
  fluidity, then saved edits are restored after decode. Adjustments are `Pod`, so
  their bytes are persisted directly in the sidecar.
- **Navigation** uses `adw::NavigationView` (library ⇄ editor) with automatic
  back button/gesture; each page has its own `HeaderBar` with a primary menu.

### UI conventions

Follows current GNOME/libadwaita patterns: one contextual `HeaderBar` per page
(`WindowTitle` shows filename + EXIF subtitle in the editor), a primary
`open-menu-symbolic` menu (Preferences, About), Nautilus-style search
(`SearchBar` toggled from the header, `Ctrl+F`) and a filter/sort `GMenu`,
`.linked` icon-button groups, toasts, and `.card`/`.osd`/`pill` styling.

## Keyboard

- `Ctrl+Z` / `Ctrl+Shift+Z` (or `Ctrl+Y`) — undo / redo
- `Ctrl+F` — search in the library
- `0`–`5` — star rating of the open image
