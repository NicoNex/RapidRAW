use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use image::{DynamicImage, GenericImageView, RgbaImage};
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;

mod ai_masks;
mod colorwheel;
mod controls;
mod crop;
mod curves;
mod editor;
mod library;
mod masks;
mod meta;
mod scopes;
mod settings;
mod sidecar;
mod slider;
mod state;
mod thumb;
mod sidebar;
mod stars;
mod thumb_cache;
use controls::AdjustPanel;
use sidebar::{Sidebar, SidebarIn, SidebarOut};
use stars::{Stars, StarsMsg, StarsOut};
use masks::MasksPanel;
use curves::Channel;
use editor::EditorCanvas;
use rapidraw_core::image_processing::{GlobalAdjustments, Point};
use rapidraw_core::mask_generation::MaskDefinition;
use rapidraw_core::lut_processing::{parse_lut_file, Lut};
use scopes::Scopes;
use settings::Settings;
use state::{Engine, Session};
use thumb::{Thumb, ThumbMsg, ThumbOut};

/// Debounce window (ms) for coalescing rapid slider drags into one render.
/// Small: the render thread also coalesces, and the cached GpuProcessor makes
/// each render cheap, so a short debounce keeps the preview responsive.
const RENDER_DEBOUNCE_MS: u64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Jpeg,
    Png,
    Tiff,
    Webp,
    Jxl,
    Avif,
    /// Bake the current look into a .cube LUT (routed to the LUT export path).
    CubeLut,
}

impl ExportFormat {
    fn ext(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Png => "png",
            ExportFormat::Tiff => "tiff",
            ExportFormat::Webp => "webp",
            ExportFormat::Jxl => "jxl",
            ExportFormat::Avif => "avif",
            ExportFormat::CubeLut => "cube",
        }
    }

    /// Formats with a meaningful quality slider.
    fn has_quality(self) -> bool {
        matches!(self, ExportFormat::Jpeg | ExportFormat::Webp | ExportFormat::Jxl)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeMode {
    LongEdge,
    Width,
    Height,
}

/// Resize on export: target `value` px for `mode`, optionally never upscaling.
#[derive(Debug, Clone, Copy)]
pub struct Resize {
    pub mode: ResizeMode,
    pub value: u32,
    pub dont_enlarge: bool,
}

/// Output options for export.
#[derive(Debug, Clone, Copy)]
pub struct ExportOpts {
    pub format: ExportFormat,
    /// JPEG quality 1..=100 (ignored for PNG/TIFF).
    pub quality: u8,
    pub resize: Option<Resize>,
}

/// A slider change: a setter that writes one `GlobalAdjustments` field plus the
/// new value. Using a fn pointer keeps the field list entirely in `controls.rs`
/// (no enum + match to keep in sync).
#[derive(Clone, Copy)]
pub struct Adjust {
    pub set: fn(&mut GlobalAdjustments, f32),
    pub value: f32,
}

impl std::fmt::Debug for Adjust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Adjust({})", self.value)
    }
}

#[derive(Debug)]
enum AppMsg {
    OpenFolderDialog,
    FolderChosen(PathBuf),
    OpenInEditor(PathBuf),
    /// A slider moved: write the value into the adjustment stack.
    Adjust(Adjust),
    /// Ask for a (debounced) preview re-render.
    RequestRender,
    /// Debounce timer fired: launch the render if `gen` is still current (stale
    /// timers from superseded requests no-op).
    DoRender(u64),
    /// Export button: open the export-options dialog.
    ExportDialog,
    /// Options were chosen: open the save dialog for them.
    ExportConfigured(ExportOpts),
    /// A save path was chosen: full-res render + encode to it.
    ExportTo(PathBuf, ExportOpts),
    /// LUT section: open a .cube/.3dl file picker.
    LoadLut,
    /// A LUT file was chosen: parse and apply it.
    LutChosen(PathBuf),
    /// Remove the active LUT.
    ClearLut,
    /// Export the current look as a .cube LUT: open the save dialog.
    ExportLutDialog,
    /// A path was chosen for the .cube export.
    ExportLutTo(PathBuf),
    /// Return from the editor to the thumbnail grid.
    ShowLibrary,
    /// Open the settings window.
    OpenSettings,
    /// Settings changed in the settings window.
    SettingsChanged(Settings),
    /// A tone curve changed: channel + points (x,y in 0..255).
    CurveChanged(Channel, Vec<(f32, f32)>),
    /// Debounced: commit the current adjustment state to the undo history if
    /// `gen` is still current (stale timers no-op).
    CommitHistory(u64),
    /// Undo / redo the adjustment history (Ctrl+Z / Ctrl+Shift+Z).
    Undo,
    Redo,
    /// Toggle the before/after view (show the unedited original).
    ToggleOriginal,
    /// Clipping indicator toggled (from the scopes panel).
    ToggleClipping(bool),
    /// Reopen the last folder from a previous session.
    ContinueSession,
    /// Sidebar picked a sub-folder to show in the grid (does NOT change the tree root).
    ShowFolder(PathBuf),
    /// Library raw-status filter changed.
    FilterChanged(library::RawFilter),
    /// Library sort order changed.
    SortChanged(library::SortBy),
    /// Library name search changed.
    SearchChanged(String),
    /// Crop / geometry controls.
    CropAspect(f32),
    RotateCw,
    RotateCcw,
    FlipH(bool),
    FlipV(bool),
    Straighten(f32),
    CropSwapOrient,
    CropReset,
    /// Right-rail switcher: show the adjustments panel / the crop panel.
    ShowAdjustPanel,
    ShowCropPanel,
    ShowMasksPanel,
    /// Masks panel actions.
    AddMask(&'static str),
    SelectMask(Option<usize>),
    DeleteMask(usize),
    ToggleMaskVisible(usize),
    ToggleMaskInvert(usize),
    SetMaskOpacity(usize, f64),
    /// Set one scalar key in mask `index`'s adjustments JSON.
    MaskAdjust {
        index: usize,
        key: &'static str,
        value: f64,
    },
    /// Set a color-grading wheel for mask `index`, zone `zone`
    /// (shadows/midtones/highlights/global): hue°, sat 0..1, lum -100..100.
    MaskGrade {
        index: usize,
        zone: &'static str,
        hue: f64,
        sat: f64,
        lum: f64,
    },
    /// Set a color-grading scalar (blending/balance) for mask `index`.
    MaskGradeScalar {
        index: usize,
        key: &'static str,
        value: f64,
    },
    /// Set an HSL component for mask `index`: band (reds/oranges/...), comp
    /// (hue/saturation/luminance), UI value -100..100.
    MaskHsl {
        index: usize,
        band: &'static str,
        comp: &'static str,
        value: f64,
    },
    /// Replace a tone-curve channel's points for mask `index`, writing
    /// `adjustments.curves.<channel>` JSON (points in 0..255).
    MaskCurve {
        index: usize,
        channel: Channel,
        points: Vec<(f32, f32)>,
    },
    /// Set one geometry key in a sub-mask's parameters JSON.
    SetSubMaskParam {
        mask: usize,
        sub: usize,
        key: &'static str,
        value: f64,
    },
    /// Set a sub-mask's compositing mode (0=Additive,1=Subtractive,2=Intersect).
    SetSubMaskMode {
        mask: usize,
        sub: usize,
        mode: u32,
    },
    /// A mask handle was dragged on the canvas: write the edited geometry back to
    /// the selected mask's sub-mask (`shape.sub`).
    EditMaskGeom(editor::MaskShape),
    /// Brush radius (image px) for painting brush/flow sub-masks.
    SetBrushSize(f64),
    /// Brush edge feather (UI 0..100) for painted strokes.
    SetBrushFeather(f64),
    /// Toggle the brush between paint and erase.
    SetBrushErase(bool),
    /// Arm/disarm canvas painting into sub-mask index (within the selected mask).
    ArmPaint(Option<usize>),
    /// A finished brush stroke: normalized points to append to sub-mask `sub`.
    AddBrushStroke {
        sub: usize,
        points: Vec<(f64, f64)>,
        erase: bool,
    },
    /// Clear all painted strokes from sub-mask `sub`.
    ClearStrokes(usize),
    /// Run AI inference for sub-mask `sub` (within the selected mask) and store
    /// the resulting mask. Type drives which model runs.
    GenerateAiMask(usize),
    /// Arm/disarm canvas picking for sub-mask `sub` (point for color/luminance,
    /// box for ai-subject).
    ArmPick(Option<usize>),
    /// Show/hide the mask coverage overlay (hidden while the pointer is over the
    /// mask editing controls).
    SetMaskOverlayShown(bool),
    /// A completed canvas pick (normalized image coords) for sub-mask `sub`.
    PickResult {
        sub: usize,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    /// Add another sub-mask of `ty` to container `mask`.
    AddSubMask(usize, &'static str),
    DeleteSubMask {
        mask: usize,
        sub: usize,
    },
    ToggleSubMaskVisible {
        mask: usize,
        sub: usize,
    },
    ToggleSubMaskInvert {
        mask: usize,
        sub: usize,
    },
    /// Editor toolbar: copy the current edit settings, paste onto this image.
    CopySettings,
    PasteSettings,
    /// Toggle window fullscreen.
    ToggleFullscreen,
    /// Set the active image's star rating (0..5).
    RateActive(u8),
    /// A thumbnail's star strip was clicked: set (or toggle-off) that path's rating.
    RateThumb(PathBuf, u8),
    /// Open the About window.
    ShowAbout,
    /// Show images from an album in the grid.
    ShowAlbum(Vec<String>),
    /// Create a new album with the given name.
    AlbumNew(String),
    /// Rename an album.
    AlbumRename { id: String, name: String },
    /// Delete an album.
    AlbumDelete(String),
}

/// Copied edit settings (toolbar copy/paste between photos).
#[derive(Clone)]
struct SettingsClip {
    global: GlobalAdjustments,
    geom: Geometry,
    lut: Option<Arc<Lut>>,
    lut_path: Option<PathBuf>,
    vals: Vec<f64>,
}

/// Crop / geometry transforms applied to the base image (CPU) before GPU render.
#[derive(Clone, Copy)]
struct Geometry {
    /// 90° rotation steps (0..3).
    orientation_steps: u8,
    flip_h: bool,
    flip_v: bool,
    /// Free straighten angle, degrees.
    straighten: f32,
    /// Crop rectangle, normalized (x, y, w, h) in image space; None = full.
    crop: Option<[f32; 4]>,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            orientation_steps: 0,
            flip_h: false,
            flip_v: false,
            straighten: 0.0,
            crop: None,
        }
    }
}

impl Geometry {
    /// True if no transform is active (so the worker can skip all of it).
    fn is_identity(&self) -> bool {
        self.orientation_steps == 0
            && !self.flip_h
            && !self.flip_v
            && self.straighten == 0.0
            && self.crop.is_none()
    }
}

/// Apply geometry to `base`. Cheap for rotate/flip/crop; only free straighten
/// (arbitrary-angle resample) is costly, and only when set.
fn apply_geometry(base: &DynamicImage, g: Geometry) -> DynamicImage {
    use rapidraw_core::image_processing::{apply_coarse_rotation, apply_rotation};
    if g.is_identity() {
        return base.clone();
    }
    let mut img = apply_coarse_rotation(base, g.orientation_steps).into_owned();
    if g.flip_h {
        img = img.fliph();
    }
    if g.flip_v {
        img = img.flipv();
    }
    if g.straighten != 0.0 {
        img = apply_rotation(&img, g.straighten).into_owned();
    }
    if let Some([rx, ry, rw, rh]) = g.crop {
        use image::GenericImageView;
        let (w, h) = img.dimensions();
        let x = (rx.clamp(0.0, 1.0) * w as f32) as u32;
        let y = (ry.clamp(0.0, 1.0) * h as f32) as u32;
        let cw = (rw.clamp(0.0, 1.0) * w as f32).round().max(1.0) as u32;
        let ch = (rh.clamp(0.0, 1.0) * h as f32).round().max(1.0) as u32;
        let cw = cw.min(w.saturating_sub(x)).max(1);
        let ch = ch.min(h.saturating_sub(y)).max(1);
        img = img.crop_imm(x, y, cw, ch);
    }
    img
}

/// Rasterize the mask's coverage as a red translucent overlay, mirroring the
/// preview render path (geometry applied, downscaled to `preview_dim`, same
/// `scale`/resolver as `core::render`). Returns premultiplied BGRA bytes (cairo
/// ARGB32 native order) + dims, or None for an empty/invisible mask.
fn compute_mask_overlay(
    base: &DynamicImage,
    geom: Geometry,
    mask_def: &MaskDefinition,
    preview_dim: u32,
) -> Option<(Vec<u8>, i32, i32)> {
    use image::GenericImageView;
    let warped = apply_geometry(base, geom);
    let full_w = warped.width().max(1);
    let b = if warped.width().max(warped.height()) > preview_dim {
        warped.resize(preview_dim, preview_dim, image::imageops::FilterType::Triangle)
    } else {
        warped.clone()
    };
    let (w, h) = b.dimensions();
    let scale = w as f32 / full_w as f32;
    let gray = rapidraw_core::mask_generation::generate_mask_bitmap(
        mask_def,
        w,
        h,
        scale,
        (0.0, 0.0),
        Some(&b),
        Some(&rapidraw_core::ai::ai_sub_mask_resolver),
    )?;
    // Tint red at half the coverage intensity (matches the original overlay),
    // premultiplied: red channel = alpha, so bytes are [B=0, G=0, R=a, A=a].
    let mut out = vec![0u8; (w * h * 4) as usize];
    for (i, px) in gray.pixels().enumerate() {
        let a = (px[0] as f32 * 0.5) as u8;
        let o = i * 4;
        out[o + 2] = a; // R
        out[o + 3] = a; // A
    }
    Some((out, w as i32, h as i32))
}

/// One undo/redo step: the full engine state plus the slider UI values needed to
/// restore the panel.
#[derive(Clone)]
struct HistEntry {
    adj: rapidraw_core::image_processing::AllAdjustments,
    lut: Option<Arc<Lut>>,
    vals: Vec<f64>,
    masks: Vec<rapidraw_core::mask_generation::MaskDefinition>,
}

/// Work sent to the persistent render thread. Keeping a single long-lived
/// thread lets the GpuProcessor (and its compiled shader) be reused across
/// renders instead of rebuilt per frame.
enum RenderJob {
    Preview {
        base: Arc<DynamicImage>,
        adj: Box<rapidraw_core::image_processing::AllAdjustments>,
        masks: Vec<MaskDefinition>,
        lut: Option<Arc<Lut>>,
        dim: u32,
        geom: Geometry,
    },
    Export {
        base: Arc<DynamicImage>,
        adj: Box<rapidraw_core::image_processing::AllAdjustments>,
        masks: Vec<MaskDefinition>,
        lut: Option<Arc<Lut>>,
        path: PathBuf,
        opts: ExportOpts,
        geom: Geometry,
    },
    /// Bake the current look into a .cube LUT file.
    ExportLut {
        adj: Box<rapidraw_core::image_processing::AllAdjustments>,
        lut: Option<Arc<Lut>>,
        path: PathBuf,
    },
}

#[derive(Debug)]
enum CmdMsg {
    /// A worker finished decoding+downscaling a thumbnail. Carries the factory
    /// index and the raw RGBA pixels (the gdk texture is built on the main thread).
    /// `(generation, index, pixels)`. Stale generations are ignored so paused
    /// thumbnail jobs (left the library) don't update the grid.
    ThumbReady(usize, usize, RgbaImage),
    /// A worker finished decoding the full base image for the editor.
    BaseReady(PathBuf, DynamicImage),
    /// A worker finished a preview render. Carries the RGBA pixels (the gdk
    /// texture is built on the main thread).
    RenderReady(RgbaImage),
    /// A worker finished a full-res export: Ok(path) or Err(message).
    ExportDone(Result<PathBuf, String>),
    /// An AI inference job finished for `(mask, sub)`: Ok(base64 PNG) or Err.
    AiMaskReady {
        mask: usize,
        sub: usize,
        result: Result<String, String>,
    },
    /// A mask-coverage overlay finished rasterizing. `gen` guards against stale
    /// (superseded) jobs; `data` is `(premult BGRA, w, h)` or None to clear.
    MaskPreviewReady {
        gen: u64,
        data: Option<(Vec<u8>, i32, i32)>,
    },
}

/// Run `f` on a dedicated OS thread and deliver its `CmdMsg` to `update_cmd`.
/// Used for user-facing work (open image, preview render, export) so it never
/// queues behind the flood of background thumbnail-decode tasks on relm4's
/// shared command pool.
fn spawn_bg<F>(sender: &ComponentSender<AppModel>, f: F)
where
    F: FnOnce() -> CmdMsg + Send + 'static,
{
    let tx = sender.command_sender().clone();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
}

struct AppModel {
    session: Session,
    images: Vec<PathBuf>,
    /// Mirror of `images`, shared into the FlowBox child-activated closure so a
    /// clicked child index can be mapped back to its path.
    images_shared: Rc<RefCell<Vec<PathBuf>>>,
    thumbs: FactoryVecDeque<Thumb>,
    /// Editor-page canvas (Picture + zoom/pan), owned by the model. Its root
    /// widget is appended to the Stack's "editor" page in `init`.
    canvas: EditorCanvas,
    /// Right-side adjustment slider panel, appended next to the canvas in `init`.
    panel: AdjustPanel,
    /// The right column (scopes + panel); kept so the panel can be rebuilt
    /// (reset) when a new image opens.
    right_col: gtk::Box,
    /// The "Edit" tab button, so opening an image can reset the switcher to it.
    edit_btn: gtk::ToggleButton,
    /// Preview scopes (histogram/waveform/vectorscope) above the panel.
    scopes: Scopes,
    /// Overlay for transient status toasts (export done, LUT loaded, …).
    toasts: adw::ToastOverlay,
    /// Render debounce generation: bumped per `RequestRender`; a fired timer
    /// only renders if its captured generation still matches (coalesces drags
    /// without removing glib sources, which panics if already fired).
    render_gen: u64,
    /// User settings (preview/thumbnail size, editor background).
    settings: Settings,
    /// Channel to the persistent render thread.
    render_tx: std::sync::mpsc::Sender<RenderJob>,
    /// Thumbnail-decode generation. Bumped to cancel in-flight decodes (queued
    /// jobs check it and skip), e.g. when leaving the library for the editor.
    thumb_gen: Arc<AtomicUsize>,
    /// Which thumbnails have decoded, so returning to the library only resumes
    /// the missing ones.
    thumb_loaded: Vec<bool>,
    /// Undo/redo stack of adjustment states; `hist_idx` points at the current one.
    history: Vec<HistEntry>,
    hist_idx: usize,
    /// History debounce generation (same scheme as `render_gen`).
    hist_gen: u64,
    /// Mask-overlay rasterization generation (latest job wins; stale jobs drop).
    mpreview_gen: u64,
    /// While true (during undo/redo restore), changes don't record history.
    suppress_history: bool,
    /// Last processed preview texture (for toggling back from "show original").
    last_tex: Option<gdk::MemoryTexture>,
    /// The unedited image at preview size (for "show original").
    original_tex: Option<gdk::MemoryTexture>,
    /// Whether the before/after view is currently showing the original.
    showing_original: bool,
    /// Clipping indicator on: show `clip_tex` (blown/crushed pixels tinted).
    clipping: bool,
    clip_tex: Option<gdk::MemoryTexture>,
    /// Last preview RGBA, kept so the clip overlay can be (re)built on toggle.
    last_rgba: Option<RgbaImage>,
    /// Header bar title widget (filename as title, EXIF as subtitle).
    win_title: adw::WindowTitle,
    /// All images scanned from the current folder (before filter/sort).
    all_images: Vec<PathBuf>,
    raw_filter: library::RawFilter,
    sort_by: library::SortBy,
    search: String,
    /// Last folder from a previous session (for "Continue session").
    last_folder: Option<PathBuf>,
    /// Crop/geometry transforms applied before the GPU render.
    geom: Geometry,
    /// Crop panel (right-rail "Crop" section).
    crop: crop::CropPanel,
    /// Masks panel (right-rail "Masks" section).
    masks_panel: MasksPanel,
    /// Index of the mask whose adjustments are shown in the masks panel.
    selected_mask: Option<usize>,
    /// Brush radius (image px) for painting brush/flow sub-masks.
    brush_size: f64,
    /// Brush edge feather (UI 0..100); stored as 0..1 per stroke.
    brush_feather: f64,
    /// Brush erases instead of painting when true.
    brush_erase: bool,
    /// Sub-mask index currently armed for canvas painting (within selected mask).
    paint_sub: Option<usize>,
    /// Switches the right column between adjustments / crop / masks panels.
    content_stack: gtk::Stack,
    /// True while the crop panel is active (canvas shows the crop overlay; the
    /// preview is rendered uncropped so the overlay can be adjusted).
    crop_active: bool,
    /// Desired crop aspect (output w/h); 0 = free.
    crop_aspect: f32,
    /// Path of the active .cube LUT (for persisting per-image edits).
    lut_path: Option<PathBuf>,
    /// Copied edit settings (toolbar copy/paste).
    settings_clip: Option<SettingsClip>,
    /// Star ratings per image (0..5), persisted to config.
    ratings: HashMap<PathBuf, u8>,
    /// Album tree (persisted to config).
    albums: Vec<rapidraw_core::albums::AlbumItem>,
    /// Sidebar folder tree component.
    sidebar: Controller<Sidebar>,
    /// Star rating widget shown in the editor header bar.
    editor_stars: Controller<Stars>,
}

impl AppModel {
    /// Restart the history-commit debounce timer.
    fn schedule_history(&mut self, sender: &ComponentSender<AppModel>) {
        if self.suppress_history {
            return;
        }
        self.hist_gen = self.hist_gen.wrapping_add(1);
        let gen = self.hist_gen;
        let sender = sender.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || {
            sender.input(AppMsg::CommitHistory(gen))
        });
    }

    /// Native aspect (w/h) of the current image, accounting for 90° rotation.
    fn native_aspect(&self) -> f32 {
        use image::GenericImageView;
        let Some(base) = &self.session.base_image else {
            return 1.0;
        };
        let (w, h) = base.dimensions();
        let (w, h) = if self.geom.orientation_steps % 2 == 1 {
            (h, w)
        } else {
            (w, h)
        };
        if h == 0 {
            1.0
        } else {
            w as f32 / h as f32
        }
    }

    /// Persist the active image's edits (adjustments + geometry + LUT) so
    /// reopening it restores them.
    fn save_edits(&self) {
        let Some(path) = self.session.active_path.clone() else {
            return;
        };
        let e = sidecar::Edits {
            global: bytemuck::bytes_of(&self.session.adjustments.global).to_vec(),
            vals: self.panel.snapshot(),
            orientation_steps: self.geom.orientation_steps,
            flip_h: self.geom.flip_h,
            flip_v: self.geom.flip_v,
            straighten: self.geom.straighten,
            crop: self.geom.crop,
            lut: self
                .lut_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            masks: self.session.masks.clone(),
        };
        sidecar::save(&path, &e);
    }

    /// Push the selected mask's drawable shapes to the canvas overlay (only while
    /// the Masks tab is active); hides it otherwise.
    fn refresh_mask_overlay(&self) {
        let on = self
            .content_stack
            .visible_child_name()
            .map(|s| s == "masks")
            .unwrap_or(false);
        let shapes = match (on, self.selected_mask, self.session.base_image.as_ref()) {
            (true, Some(i), Some(base)) => {
                use image::GenericImageView;
                let (w, h) = base.dimensions();
                self.session
                    .masks
                    .get(i)
                    .map(|m| masks::overlay_shapes(m, w as f64, h as f64))
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        };
        self.canvas.set_mask_overlay(shapes, on);
    }

    /// Recompute the selected mask's red coverage overlay on a worker (debounced
    /// via a generation token). Clears it when the Masks tab is off or nothing is
    /// selected. Called from the render debounce point + selection/tab changes.
    fn refresh_mask_preview(&mut self, sender: &ComponentSender<AppModel>) {
        let on = self
            .content_stack
            .visible_child_name()
            .map(|s| s == "masks")
            .unwrap_or(false);
        let sel = self.selected_mask.filter(|&i| i < self.session.masks.len());
        let (Some(i), Some(base), true) = (sel, self.session.base_image.clone(), on) else {
            self.canvas.set_mask_preview(None);
            return;
        };
        let mask_def = self.session.masks[i].clone();
        let geom = self.geom;
        let preview_dim = self.settings.preview_dim;
        self.mpreview_gen = self.mpreview_gen.wrapping_add(1);
        let gen = self.mpreview_gen;
        spawn_bg(sender, move || {
            let data = compute_mask_overlay(&base, geom, &mask_def, preview_dim);
            CmdMsg::MaskPreviewReady { gen, data }
        });
    }

    /// Full (pre-preview) image dimensions as f64, if an image is open.
    fn image_dims(&self) -> Option<(f64, f64)> {
        use image::GenericImageView;
        self.session
            .base_image
            .as_ref()
            .map(|b| b.dimensions())
            .map(|(w, h)| (w as f64, h as f64))
    }

    /// Texture to display now: original (before/after) > clipping overlay > edited.
    fn active_tex(&self) -> Option<gdk::MemoryTexture> {
        if self.showing_original {
            self.original_tex.clone()
        } else if self.clipping {
            self.clip_tex.clone().or_else(|| self.last_tex.clone())
        } else {
            self.last_tex.clone()
        }
    }

    /// Update the canvas to the active texture (preserving zoom/pan).
    fn show_active_tex(&self) {
        if let Some(tex) = self.active_tex() {
            self.canvas.update_texture(&tex);
        }
    }

    /// Re-filter/-sort `all_images` into `images` and rebuild the thumbnail grid.
    fn apply_library(&mut self, sender: &ComponentSender<AppModel>) {
        self.images = library::arrange(
            &self.all_images,
            self.raw_filter,
            self.sort_by,
            &self.search,
            &self.ratings,
        );
        *self.images_shared.borrow_mut() = self.images.clone();

        let mut guard = self.thumbs.guard();
        guard.clear();
        for p in &self.images {
            let r = self.ratings.get(p).copied().unwrap_or(0);
            guard.push_back((p.clone(), r));
        }
        drop(guard);

        self.thumb_loaded = vec![false; self.images.len()];
        let gen = self.thumb_gen.fetch_add(1, Ordering::Relaxed) + 1;
        dispatch_thumbs(
            sender,
            &self.thumb_gen,
            gen,
            self.settings.thumb_dim,
            &self.images,
            0..self.images.len(),
        );
    }

    /// Persist albums to disk and push the updated list to the sidebar.
    fn persist_albums(&mut self) {
        if let Some(p) = albums_file() {
            let _ = rapidraw_core::albums::save_albums(&p, &mut self.albums);
        }
        self.sidebar.emit(SidebarIn::SetAlbums(self.albums.clone()));
    }

    /// Apply the history entry at `hist_idx`: set engine state, restore the
    /// panel UI, and re-render. Does not record new history.
    fn apply_history(&mut self, sender: &ComponentSender<AppModel>) {
        let entry = self.history[self.hist_idx].clone();
        self.session.adjustments = entry.adj;
        self.session.lut = entry.lut;
        self.session.masks = entry.masks;
        self.selected_mask = self
            .selected_mask
            .filter(|&i| i < self.session.masks.len());
        self.masks_panel
            .rebuild(&self.session.masks, self.selected_mask, sender);
        self.suppress_history = true;
        self.panel.restore(&entry.vals);
        self.panel.sync(&self.session.adjustments.global);
        self.suppress_history = false;
        if self.showing_original {
            self.showing_original = false;
        }
        sender.input(AppMsg::RequestRender);
    }
}

/// Spawn background decode jobs for thumbnails at `indices` under `gen`; each
/// job skips its work if `gen` is stale (the user left the library).
fn dispatch_thumbs(
    sender: &ComponentSender<AppModel>,
    gen_tok: &Arc<AtomicUsize>,
    gen: usize,
    thumb_dim: u32,
    images: &[PathBuf],
    indices: impl IntoIterator<Item = usize>,
) {
    // Build the work list, then chew through it on a pool of OS threads sized to
    // the CPU. A shared atomic cursor work-steals; each thread bails the moment
    // the generation token moves (left the library / changed filter), so no
    // wasted RAW decodes. Real threads (not the async runtime) because decoding
    // is heavy and CPU-bound.
    let jobs: Arc<Vec<(usize, PathBuf)>> =
        Arc::new(indices.into_iter().map(|i| (i, images[i].clone())).collect());
    if jobs.is_empty() {
        return;
    }
    let cursor = Arc::new(AtomicUsize::new(0));
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(jobs.len());

    for _ in 0..n {
        let jobs = jobs.clone();
        let cursor = cursor.clone();
        let tok = gen_tok.clone();
        let cmd = sender.command_sender().clone();
        std::thread::spawn(move || loop {
            if tok.load(Ordering::Relaxed) != gen {
                break; // cancelled
            }
            let idx = cursor.fetch_add(1, Ordering::Relaxed);
            let Some((i, p)) = jobs.get(idx) else {
                break; // queue drained
            };
            let rgba = thumb_cache::load(p, thumb_dim).unwrap_or_else(|| {
                match rapidraw_core::load_base_image(p) {
                    Ok(img) => {
                        let (w, h) = img.dimensions();
                        let scaled = if w.max(h) > thumb_dim {
                            img.resize(thumb_dim, thumb_dim, image::imageops::FilterType::Triangle)
                        } else {
                            img
                        };
                        let rgba = scaled.to_rgba8();
                        thumb_cache::save(p, thumb_dim, &rgba);
                        rgba
                    }
                    Err(e) => {
                        log::warn!("thumb decode failed for {}: {e}", p.display());
                        RgbaImage::new(1, 1)
                    }
                }
            });
            let _ = cmd.send(CmdMsg::ThumbReady(gen, *i, rgba));
        });
    }
}

/// Spawn the single long-lived render thread. It owns the GpuProcessor cache
/// (via the thread-local in `rapidraw_core::render`), so the shader compiles
/// once per image size rather than every frame.
fn spawn_render_worker(
    ctx: Arc<rapidraw_core::image_processing::GpuContext>,
    sender: ComponentSender<AppModel>,
) -> std::sync::mpsc::Sender<RenderJob> {
    let (tx, rx) = std::sync::mpsc::channel::<RenderJob>();
    let cmd = sender.command_sender().clone();
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            // Drain everything pending so a burst of slider updates collapses to
            // the latest preview (older previews are stale); exports always run.
            let mut latest_preview = None;
            let mut jobs = vec![first];
            jobs.extend(rx.try_iter());
            for job in jobs {
                match job {
                    RenderJob::Preview { .. } => latest_preview = Some(job),
                    RenderJob::Export {
                        base,
                        adj,
                        masks,
                        lut,
                        path,
                        opts,
                        geom,
                    } => {
                        let base = apply_geometry(&base, geom);
                        let res = rapidraw_core::render(
                            &ctx,
                            &base,
                            &adj,
                            &masks,
                            lut,
                            None,
                            Some(&rapidraw_core::ai::ai_sub_mask_resolver),
                        )
                        .and_then(|out| encode_image(&out, &path, opts))
                            .map(|()| path);
                        let _ = cmd.send(CmdMsg::ExportDone(res));
                    }
                    RenderJob::ExportLut { adj, lut, path } => {
                        let res = export_lut(&ctx, &adj, lut, &path).map(|()| path);
                        let _ = cmd.send(CmdMsg::ExportDone(res));
                    }
                }
            }
            if let Some(RenderJob::Preview {
                base,
                adj,
                masks,
                lut,
                dim,
                geom,
            }) = latest_preview
            {
                let base = apply_geometry(&base, geom);
                match rapidraw_core::render(
                    &ctx,
                    &base,
                    &adj,
                    &masks,
                    lut,
                    Some(dim),
                    Some(&rapidraw_core::ai::ai_sub_mask_resolver),
                ) {
                    Ok(out) => {
                        let _ = cmd.send(CmdMsg::RenderReady(out.to_rgba8()));
                    }
                    Err(e) => log::warn!("preview render failed: {e}"),
                }
            }
        }
    });
    tx
}

#[relm4::component]
impl Component for AppModel {
    type Init = Engine;
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = CmdMsg;

    view! {
        adw::ApplicationWindow {
            set_title: Some("RapidRAW"),
            set_default_size: (1440, 900),

            #[wrap(Some)]
            #[name = "toast_overlay"]
            set_content = &adw::ToastOverlay {
                #[wrap(Some)]
                #[name = "split"]
                set_child = &gtk::Paned {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_position: 220,
                    set_resize_start_child: false,
                    set_shrink_start_child: false,
                    #[wrap(Some)]
                    #[name = "sidebar_slot"]
                    set_start_child = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                    },
                    #[wrap(Some)]
                    #[name = "nav"]
                    set_end_child = &adw::NavigationView {
                    // ----- Library page -----
                    add = &adw::NavigationPage {
                        set_tag: Some("library"),
                        set_title: "RapidRAW",
                        #[wrap(Some)]
                        set_child = &adw::ToolbarView {
                            add_top_bar = &adw::HeaderBar {
                                #[name = "sidebar_toggle_lib"]
                                pack_start = &gtk::ToggleButton {
                                    set_icon_name: "sidebar-show-symbolic",
                                    set_tooltip_text: Some("Toggle sidebar"),
                                    set_active: false,
                                },
                                pack_start = &gtk::Button {
                                    set_label: "Open Folder",
                                    connect_clicked => AppMsg::OpenFolderDialog,
                                },
                                #[name = "menu_lib"]
                                pack_end = &gtk::MenuButton {
                                    set_icon_name: "open-menu-symbolic",
                                    set_tooltip_text: Some("Main menu"),
                                    set_primary: true,
                                },
                                #[name = "library_right"]
                                pack_end = &gtk::Box {
                                    set_spacing: 6,
                                    #[name = "filter_menu"]
                                    gtk::MenuButton {
                                        set_icon_name: "view-more-symbolic",
                                        set_tooltip_text: Some("Filter & sort"),
                                    },
                                    #[name = "search_btn"]
                                    gtk::ToggleButton {
                                        set_icon_name: "system-search-symbolic",
                                        set_tooltip_text: Some("Search"),
                                    },
                                },
                            },
                            #[wrap(Some)]
                            set_content = &gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                #[name = "lib_stack"]
                                gtk::Stack {
                                    set_vexpand: true,
                                    set_hexpand: true,
                                    add_named[Some("grid")] = &gtk::Box {
                                        set_orientation: gtk::Orientation::Vertical,
                                        #[name = "search_bar"]
                                        gtk::SearchBar {},
                                        gtk::ScrolledWindow {
                                            set_vexpand: true,
                                            set_hscrollbar_policy: gtk::PolicyType::Never,
                                            #[local_ref]
                                            flow_box -> gtk::FlowBox {
                                                set_valign: gtk::Align::Start,
                                                set_selection_mode: gtk::SelectionMode::Single,
                                                set_homogeneous: true,
                                                set_column_spacing: 8,
                                                set_row_spacing: 8,
                                                set_margin_all: 8,
                                                connect_child_activated[sender, images] => move |_, child| {
                                                    let idx = child.index();
                                                    if idx >= 0 {
                                                        if let Some(path) = images.borrow().get(idx as usize) {
                                                            sender.input(AppMsg::OpenInEditor(path.clone()));
                                                        }
                                                    }
                                                },
                                            },
                                        },
                                    },
                                },
                            },
                        },
                    },

                    // ----- Editor page (pushed on open; back button is automatic) -----
                    add = &adw::NavigationPage {
                        set_tag: Some("editor"),
                        set_title: "Editor",
                        #[wrap(Some)]
                        set_child = &adw::ToolbarView {
                            add_top_bar = &adw::HeaderBar {
                                #[wrap(Some)]
                                #[name = "win_title"]
                                set_title_widget = &adw::WindowTitle {
                                    set_title: "RapidRAW",
                                },
                                #[name = "sidebar_toggle_ed"]
                                pack_start = &gtk::ToggleButton {
                                    set_icon_name: "sidebar-show-symbolic",
                                    set_tooltip_text: Some("Toggle sidebar"),
                                    set_active: false,
                                },
                                pack_start = &gtk::Box {
                                    add_css_class: "linked",
                                    gtk::Button {
                                        set_icon_name: "edit-undo-symbolic",
                                        set_tooltip_text: Some("Undo (Ctrl+Z)"),
                                        connect_clicked => AppMsg::Undo,
                                    },
                                    gtk::Button {
                                        set_icon_name: "edit-redo-symbolic",
                                        set_tooltip_text: Some("Redo (Ctrl+Shift+Z)"),
                                        connect_clicked => AppMsg::Redo,
                                    },
                                },
                                #[name = "editor_stars_slot"]
                                pack_start = &gtk::Box {},
                                #[name = "menu_ed"]
                                pack_end = &gtk::MenuButton {
                                    set_icon_name: "open-menu-symbolic",
                                    set_tooltip_text: Some("Main menu"),
                                    set_primary: true,
                                },
                                pack_end = &gtk::Button {
                                    set_label: "Export",
                                    add_css_class: "suggested-action",
                                    connect_clicked => AppMsg::ExportDialog,
                                },
                                pack_end = &gtk::Box {
                                    add_css_class: "linked",
                                    #[name = "orig_btn"]
                                    gtk::ToggleButton {
                                        set_icon_name: "view-reveal-symbolic",
                                        set_tooltip_text: Some("Show original"),
                                        connect_toggled => AppMsg::ToggleOriginal,
                                    },
                                    gtk::Button {
                                        set_icon_name: "edit-copy-symbolic",
                                        set_tooltip_text: Some("Copy settings"),
                                        connect_clicked => AppMsg::CopySettings,
                                    },
                                    gtk::Button {
                                        set_icon_name: "edit-paste-symbolic",
                                        set_tooltip_text: Some("Paste settings"),
                                        connect_clicked => AppMsg::PasteSettings,
                                    },
                                    gtk::Button {
                                        set_icon_name: "view-fullscreen-symbolic",
                                        set_tooltip_text: Some("Fullscreen"),
                                        connect_clicked => AppMsg::ToggleFullscreen,
                                    },
                                },
                            },
                            #[wrap(Some)]
                            #[name = "editor_page"]
                            set_content = &gtk::Paned {
                                set_vexpand: true,
                                set_orientation: gtk::Orientation::Horizontal,
                                set_wide_handle: true,
                            },
                        },
                    },
                },
                },
            },
        }
    }

    fn init(
        engine: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let thumbs = FactoryVecDeque::builder()
            .launch(gtk::FlowBox::default())
            .forward(sender.input_sender(), |out| match out {
                ThumbOut::Rate(path, n) => AppMsg::RateThumb(path, n),
            });

        let render_tx = spawn_render_worker(engine.ctx.clone(), sender.clone());

        let albums = albums_file()
            .map(|p| rapidraw_core::albums::load_albums(&p))
            .unwrap_or_default();

        let sidebar = Sidebar::builder()
            .launch(())
            .forward(sender.input_sender(), |out| match out {
                SidebarOut::SelectFolder(p) => AppMsg::ShowFolder(p),
                SidebarOut::AddRootFolder => AppMsg::OpenFolderDialog,
                SidebarOut::SelectAlbum(images) => AppMsg::ShowAlbum(images),
                SidebarOut::NewAlbum(name) => AppMsg::AlbumNew(name),
                SidebarOut::RenameAlbum { id, name } => AppMsg::AlbumRename { id, name },
                SidebarOut::DeleteAlbum(id) => AppMsg::AlbumDelete(id),
            });

        let editor_stars = Stars::builder()
            .launch(0)
            .forward(sender.input_sender(), |out| match out {
                StarsOut::Changed(n) => AppMsg::RateActive(n),
            });

        let model = AppModel {
            session: Session::default(),
            images: Vec::new(),
            images_shared: Rc::new(RefCell::new(Vec::new())),
            thumbs,
            canvas: EditorCanvas::new(),
            panel: AdjustPanel::new(&sender),
            right_col: gtk::Box::new(gtk::Orientation::Vertical, 4),
            edit_btn: gtk::ToggleButton::new(), // replaced by the real tab below
            scopes: Scopes::new(),
            toasts: adw::ToastOverlay::new(), // replaced by the view's overlay below
            render_gen: 0,
            settings: load_settings(),
            render_tx,
            thumb_gen: Arc::new(AtomicUsize::new(0)),
            thumb_loaded: Vec::new(),
            history: Vec::new(),
            hist_idx: 0,
            hist_gen: 0,
            mpreview_gen: 0,
            suppress_history: false,
            last_tex: None,
            original_tex: None,
            showing_original: false,
            clipping: false,
            clip_tex: None,
            last_rgba: None,
            win_title: adw::WindowTitle::new("RapidRAW", ""),
            all_images: Vec::new(),
            raw_filter: library::RawFilter::All,
            sort_by: library::SortBy::Name,
            search: String::new(),
            last_folder: load_last_folder(),
            geom: Geometry::default(),
            crop: crop::CropPanel::new(&sender),
            masks_panel: MasksPanel::new(&sender),
            selected_mask: None,
            brush_size: 50.0,
            brush_feather: 50.0,
            brush_erase: false,
            paint_sub: None,
            content_stack: gtk::Stack::new(),
            crop_active: false,
            crop_aspect: 0.0,
            lut_path: None,
            settings_clip: None,
            ratings: load_ratings(),
            albums,
            sidebar,
            editor_stars,
        };
        // Seed the engine struct with the UI defaults (e.g. vignette midpoint/
        // feather = 50) so effects behave like the original at zero amount.
        let mut model = model;
        controls::init_defaults(&mut model.session.adjustments.global);

        let flow_box = model.thumbs.widget();
        let images = model.images_shared.clone();
        let widgets = view_output!();
        install_app_css();
        model.toasts = widgets.toast_overlay.clone();
        model.win_title = widgets.win_title.clone();

        // Primary menu (Preferences / About) as a proper GMenu on both pages.
        let app_actions = gtk::gio::SimpleActionGroup::new();
        let act_prefs = gtk::gio::SimpleAction::new("preferences", None);
        {
            let sender = sender.clone();
            act_prefs.connect_activate(move |_, _| sender.input(AppMsg::OpenSettings));
        }
        let act_about = gtk::gio::SimpleAction::new("about", None);
        {
            let sender = sender.clone();
            act_about.connect_activate(move |_, _| sender.input(AppMsg::ShowAbout));
        }
        app_actions.add_action(&act_prefs);
        app_actions.add_action(&act_about);
        root.insert_action_group("app", Some(&app_actions));
        let menu = gtk::gio::Menu::new();
        menu.append(Some("Preferences"), Some("app.preferences"));
        menu.append(Some("About RapidRAW"), Some("app.about"));
        widgets.menu_lib.set_menu_model(Some(&menu));
        widgets.menu_ed.set_menu_model(Some(&menu));

        // Start on the library page; the editor page is pushed on open.
        widgets.nav.replace_with_tags(&["library"]);
        {
            let sender = sender.clone();
            widgets.nav.connect_popped(move |_, _| sender.input(AppMsg::ShowLibrary));
        }

        // Filter & sort: a proper popover MENU with stateful radio actions.
        // (DropDowns nested in a popover fight the grab and won't dismiss.)
        let act_filter = gtk::gio::SimpleAction::new_stateful(
            "filter",
            Some(gtk::glib::VariantTy::STRING),
            &gtk::glib::Variant::from("all"),
        );
        {
            let sender = sender.clone();
            act_filter.connect_activate(move |a, p| {
                if let Some(p) = p {
                    a.set_state(p);
                    let f = match p.str().unwrap_or("all") {
                        "raw" => library::RawFilter::RawOnly,
                        "nonraw" => library::RawFilter::NonRawOnly,
                        "prefer" => library::RawFilter::PreferRaw,
                        _ => library::RawFilter::All,
                    };
                    sender.input(AppMsg::FilterChanged(f));
                }
            });
        }
        let act_sort = gtk::gio::SimpleAction::new_stateful(
            "sort",
            Some(gtk::glib::VariantTy::STRING),
            &gtk::glib::Variant::from("name"),
        );
        {
            let sender = sender.clone();
            act_sort.connect_activate(move |a, p| {
                if let Some(p) = p {
                    a.set_state(p);
                    let s = match p.str().unwrap_or("name") {
                        "new" => library::SortBy::DateNewest,
                        "old" => library::SortBy::DateOldest,
                        "rating" => library::SortBy::RatingDesc,
                        _ => library::SortBy::Name,
                    };
                    sender.input(AppMsg::SortChanged(s));
                }
            });
        }
        app_actions.add_action(&act_filter);
        app_actions.add_action(&act_sort);

        let fs_menu = gtk::gio::Menu::new();
        let f_sec = gtk::gio::Menu::new();
        f_sec.append(Some("All"), Some("app.filter::all"));
        f_sec.append(Some("Raw only"), Some("app.filter::raw"));
        f_sec.append(Some("Non-raw only"), Some("app.filter::nonraw"));
        f_sec.append(Some("Prefer raw"), Some("app.filter::prefer"));
        fs_menu.append_section(Some("Filter"), &f_sec);
        let s_sec = gtk::gio::Menu::new();
        s_sec.append(Some("Name"), Some("app.sort::name"));
        s_sec.append(Some("Newest"), Some("app.sort::new"));
        s_sec.append(Some("Oldest"), Some("app.sort::old"));
        s_sec.append(Some("Rating"), Some("app.sort::rating"));
        fs_menu.append_section(Some("Sort"), &s_sec);
        widgets.filter_menu.set_menu_model(Some(&fs_menu));

        // Search: a SearchEntry in the drop-down SearchBar, toggled by the header
        // search button (GNOME/Nautilus pattern). Entry is wide/centred.
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search filename…"));
        search.set_hexpand(true);
        search.set_max_width_chars(60);
        {
            let sender = sender.clone();
            search.connect_search_changed(move |e| {
                sender.input(AppMsg::SearchChanged(e.text().to_string()));
            });
        }
        widgets.search_bar.set_child(Some(&search));
        widgets.search_bar.connect_entry(&search);
        widgets
            .search_btn
            .bind_property("active", &widgets.search_bar, "search-mode-enabled")
            .bidirectional()
            .sync_create()
            .build();

        // Welcome screen: full-bleed splash, a soft scrim for contrast, brand +
        // pill buttons centred (no boxed card).
        let welcome = gtk::Overlay::new();
        if let Some(tex) = splash_texture() {
            let pic = gtk::Picture::for_paintable(&tex);
            pic.set_content_fit(gtk::ContentFit::Cover);
            welcome.set_child(Some(&pic));
        }
        let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scrim.add_css_class("welcome-scrim");
        scrim.set_hexpand(true);
        scrim.set_vexpand(true);
        welcome.add_overlay(&scrim);

        let center = gtk::Box::new(gtk::Orientation::Vertical, 16);
        center.set_halign(gtk::Align::Center);
        center.set_valign(gtk::Align::Center);
        let brand = gtk::Label::new(Some("RapidRAW"));
        brand.add_css_class("welcome-title");
        let btns = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        btns.set_halign(gtk::Align::Center);
        let open_btn = gtk::Button::with_label("Open Folder");
        open_btn.add_css_class("pill");
        open_btn.add_css_class("suggested-action");
        {
            let sender = sender.clone();
            open_btn.connect_clicked(move |_| sender.input(AppMsg::OpenFolderDialog));
        }
        let cont_btn = gtk::Button::with_label("Continue session");
        cont_btn.add_css_class("pill");
        cont_btn.set_visible(model.last_folder.is_some());
        {
            let sender = sender.clone();
            cont_btn.connect_clicked(move |_| sender.input(AppMsg::ContinueSession));
        }
        btns.append(&open_btn);
        btns.append(&cont_btn);
        center.append(&brand);
        center.append(&btns);
        welcome.add_overlay(&center);
        widgets.lib_stack.add_named(&welcome, Some("welcome"));
        widgets.lib_stack.set_visible_child_name("welcome");

        // Undo/redo keyboard shortcuts (Ctrl+Z / Ctrl+Shift+Z, plus Ctrl+Y).
        let key = gtk::EventControllerKey::new();
        {
            let sender = sender.clone();
            let root_w = root.clone();
            let nav = widgets.nav.clone();
            let search_btn = widgets.search_btn.clone();
            let search_entry = search.clone();
            key.connect_key_pressed(move |_, keyval, _, state| {
                if !state.contains(gdk::ModifierType::CONTROL_MASK) {
                    // 0..5 set the star rating — unless a text field has focus
                    // (so typing a slider value isn't hijacked).
                    let typing = gtk::prelude::GtkWindowExt::focus(&root_w)
                        .map_or(false, |w| w.is::<gtk::Text>());
                    if !typing {
                        if let Some(d @ '0'..='5') = keyval.to_unicode() {
                            sender.input(AppMsg::RateActive(d as u8 - b'0'));
                            return glib::Propagation::Stop;
                        }
                    }
                    return glib::Propagation::Proceed;
                }
                let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
                match keyval.to_lower() {
                    gdk::Key::z => {
                        sender.input(if shift { AppMsg::Redo } else { AppMsg::Undo });
                        glib::Propagation::Stop
                    }
                    gdk::Key::y => {
                        sender.input(AppMsg::Redo);
                        glib::Propagation::Stop
                    }
                    // Ctrl+F: open library search (library page only).
                    gdk::Key::f => {
                        let in_lib = nav
                            .visible_page()
                            .and_then(|p| p.tag())
                            .map_or(true, |t| t != "editor");
                        if in_lib {
                            search_btn.set_active(true);
                            search_entry.grab_focus();
                            return glib::Propagation::Stop;
                        }
                        glib::Propagation::Proceed
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        root.add_controller(key);
        // Editor page: canvas on the left; right column = scopes on top of the
        // adjustment panel. A Paned divider keeps the panel at a fixed,
        // mouse-resizable width that the photo zoom never disturbs.
        // Right column = scopes on top of a Stack switching adjustments <-> crop.
        model.content_stack.set_vexpand(true);
        model
            .content_stack
            .add_named(model.panel.root(), Some("adjust"));
        model
            .content_stack
            .add_named(model.crop.root(), Some("crop"));
        model
            .content_stack
            .add_named(model.masks_panel.root(), Some("masks"));
        model.content_stack.set_visible_child_name("adjust");
        // Fixed, comfortable panel width. The Paned sizes the end child to this
        // natural width at any window size (the canvas absorbs the rest), so the
        // panel never grabs half a maximized window. The user can still drag.
        model.right_col.set_width_request(370);

        // Top tabs (Edit / Crop), a centred linked toggle group, above the panel.
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs.add_css_class("linked");
        tabs.set_halign(gtk::Align::Center);
        tabs.set_margin_top(6);
        tabs.set_margin_bottom(2);
        // Text labels: Adwaita has no crop/adjust symbolic icons, so icon names
        // render as the "missing image" placeholder. Tabs read fine as text.
        let adj_btn = gtk::ToggleButton::with_label("Edit");
        adj_btn.set_tooltip_text(Some("Adjustments"));
        adj_btn.set_active(true);
        let crop_btn = gtk::ToggleButton::with_label("Crop");
        crop_btn.set_tooltip_text(Some("Crop & geometry"));
        crop_btn.set_group(Some(&adj_btn));
        let masks_btn = gtk::ToggleButton::with_label("Masks");
        masks_btn.set_tooltip_text(Some("Masks"));
        masks_btn.set_group(Some(&adj_btn));
        {
            let sender = sender.clone();
            adj_btn.connect_toggled(move |b| {
                if b.is_active() {
                    sender.input(AppMsg::ShowAdjustPanel);
                }
            });
        }
        {
            let sender = sender.clone();
            crop_btn.connect_toggled(move |b| {
                if b.is_active() {
                    sender.input(AppMsg::ShowCropPanel);
                }
            });
        }
        {
            let sender = sender.clone();
            masks_btn.connect_toggled(move |b| {
                if b.is_active() {
                    sender.input(AppMsg::ShowMasksPanel);
                }
            });
        }
        tabs.append(&adj_btn);
        tabs.append(&crop_btn);
        tabs.append(&masks_btn);
        model.edit_btn = adj_btn.clone();

        model.right_col.append(&tabs);
        model.right_col.append(model.scopes.root());
        model.right_col.append(&model.content_stack);

        let paned = &widgets.editor_page;
        paned.set_start_child(Some(model.canvas.root()));
        paned.set_end_child(Some(&model.right_col));
        // Start (canvas) absorbs window resizes and may shrink below its child's
        // size (clipped); the panel keeps its width unless the user drags.
        paned.set_resize_start_child(true);
        paned.set_shrink_start_child(true);
        paned.set_resize_end_child(false);
        paned.set_shrink_end_child(false);
        // No absolute position: the divider follows the end child's natural width
        // (370) regardless of window size, and the user can drag to override.

        // Canvas mask-handle drags feed back into the model as geometry edits.
        {
            let sender = sender.clone();
            model
                .canvas
                .set_mask_editor(move |shape| sender.input(AppMsg::EditMaskGeom(shape)));
        }
        // Brush/flow strokes painted on the canvas append to the sub-mask.
        {
            let sender = sender.clone();
            model.canvas.set_paint_sink(move |sub, points, erase| {
                sender.input(AppMsg::AddBrushStroke { sub, points, erase })
            });
        }
        // Canvas point/box picks feed parametric (color/luminance) and ai-subject
        // masks.
        {
            let sender = sender.clone();
            model.canvas.set_pick_sink(move |sub, x1, y1, x2, y2| {
                sender.input(AppMsg::PickResult { sub, x1, y1, x2, y2 })
            });
        }
        // Scopes clipping toggle feeds back into the model.
        {
            let sender = sender.clone();
            model
                .scopes
                .set_clip_toggle(move |on| sender.input(AppMsg::ToggleClipping(on)));
        }
        {
            let sb = model.sidebar.widget().clone();
            let other = widgets.sidebar_toggle_ed.clone();
            widgets.sidebar_toggle_lib.connect_toggled(move |b| {
                sb.set_visible(b.is_active());
                if other.is_active() != b.is_active() {
                    other.set_active(b.is_active());
                }
            });
        }
        {
            let sb = model.sidebar.widget().clone();
            let other = widgets.sidebar_toggle_lib.clone();
            widgets.sidebar_toggle_ed.connect_toggled(move |b| {
                sb.set_visible(b.is_active());
                if other.is_active() != b.is_active() {
                    other.set_active(b.is_active());
                }
            });
        }
        widgets.split.set_start_child(Some(model.sidebar.widget()));
        // Hidden until a folder is opened (no sidebar on the welcome/splash screen).
        model.sidebar.widget().set_visible(false);
        widgets.editor_stars_slot.append(model.editor_stars.widget());
        model.sidebar.emit(SidebarIn::SetAlbums(model.albums.clone()));
        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: Self::Input,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        match msg {
            AppMsg::OpenFolderDialog => {
                let dialog = gtk::FileDialog::builder().title("Select folder").build();
                let parent = root.clone();
                let sender = sender.clone();
                dialog.select_folder(Some(&parent), gtk::gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res {
                        if let Some(path) = file.path() {
                            sender.input(AppMsg::FolderChosen(path));
                        }
                    }
                });
            }
            AppMsg::FolderChosen(path) => {
                log::info!("Folder chosen: {}", path.display());
                self.all_images = library::scan_dir(&path);
                log::info!("{} images", self.all_images.len());
                save_last_folder(&path);
                self.last_folder = Some(path.clone());
                self.session.current_folder = Some(path);
                self.sidebar.emit(SidebarIn::SetRoot(self.session.current_folder.clone()));
                widgets.lib_stack.set_visible_child_name("grid");
                // Reveal the sidebar now that there's a folder to navigate.
                self.sidebar.widget().set_visible(true);
                widgets.sidebar_toggle_lib.set_active(true);
                widgets.sidebar_toggle_ed.set_active(true);
                self.apply_library(&sender);
            }
            AppMsg::ContinueSession => {
                if let Some(p) = self.last_folder.clone() {
                    sender.input(AppMsg::FolderChosen(p));
                }
            }
            AppMsg::ShowFolder(dir) => {
                self.all_images = library::scan_dir(&dir);
                self.apply_library(&sender);
            }
            AppMsg::FilterChanged(f) => {
                self.raw_filter = f;
                self.apply_library(&sender);
            }
            AppMsg::SortChanged(s) => {
                self.sort_by = s;
                self.apply_library(&sender);
            }
            AppMsg::SearchChanged(q) => {
                self.search = q;
                self.apply_library(&sender);
            }
            AppMsg::CropAspect(a) => {
                // -1 = "Original": the image's native aspect (after 90° rotation).
                let aspect = if a < 0.0 { self.native_aspect() } else { a };
                self.crop_aspect = aspect;
                if self.crop_active {
                    self.canvas.set_crop_aspect(aspect as f64);
                }
            }
            AppMsg::CropSwapOrient => {
                if self.crop_aspect > 0.0 {
                    self.crop_aspect = 1.0 / self.crop_aspect;
                    if self.crop_active {
                        self.canvas.set_crop_aspect(self.crop_aspect as f64);
                    }
                }
            }
            AppMsg::RotateCw => {
                self.geom.orientation_steps = (self.geom.orientation_steps + 1) % 4;
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::RotateCcw => {
                self.geom.orientation_steps = (self.geom.orientation_steps + 3) % 4;
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::FlipH(b) => {
                self.geom.flip_h = b;
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::FlipV(b) => {
                self.geom.flip_v = b;
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::Straighten(v) => {
                self.geom.straighten = v;
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::CropReset => {
                self.geom = Geometry::default();
                self.crop_aspect = 0.0;
                self.canvas.reset_crop();
                // Rebuild the crop panel so its toggles/sliders reflect the reset.
                let fresh = crop::CropPanel::new(&sender);
                self.content_stack.remove(self.crop.root());
                self.content_stack.add_named(fresh.root(), Some("crop"));
                self.crop = fresh;
                if self.crop_active {
                    self.content_stack.set_visible_child_name("crop");
                }
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::ShowAdjustPanel => {
                self.content_stack.set_visible_child_name("adjust");
                // Scopes show in Edit (hidden in Crop, like the reference).
                self.scopes.root().set_visible(true);
                // Commit the interactive crop on leaving crop mode.
                if self.crop_active {
                    self.crop_active = false;
                    let (x, y, w, h) = self.canvas.exit_crop();
                    self.geom.crop = if w >= 0.999 && h >= 0.999 && x <= 0.001 && y <= 0.001 {
                        None
                    } else {
                        Some([x as f32, y as f32, w as f32, h as f32])
                    };
                    sender.input(AppMsg::RequestRender);
                }
                self.refresh_mask_preview(&sender); // clears (Masks tab off)
            }
            AppMsg::ShowCropPanel => {
                self.content_stack.set_visible_child_name("crop");
                // Crop hides the scopes (matches the reference UI).
                self.scopes.root().set_visible(false);
                self.crop_active = true;
                // Show the full (uncropped) image with the crop overlay.
                self.canvas.enter_crop(self.crop_aspect as f64);
                sender.input(AppMsg::RequestRender);
                self.refresh_mask_preview(&sender); // clears (Masks tab off)
            }
            AppMsg::ShowMasksPanel => {
                self.content_stack.set_visible_child_name("masks");
                // Masks shows the scopes (like Edit; only Crop hides them).
                self.scopes.root().set_visible(true);
                // Leaving crop mode commits the interactive crop (mirror ShowAdjustPanel).
                if self.crop_active {
                    self.crop_active = false;
                    let (x, y, w, h) = self.canvas.exit_crop();
                    self.geom.crop = if w >= 0.999 && h >= 0.999 && x <= 0.001 && y <= 0.001 {
                        None
                    } else {
                        Some([x as f32, y as f32, w as f32, h as f32])
                    };
                    sender.input(AppMsg::RequestRender);
                }
                self.masks_panel
                    .rebuild(&self.session.masks, self.selected_mask, &sender);
                self.refresh_mask_preview(&sender);
            }
            AppMsg::AddMask(ty) => {
                // Default sub-mask geometry needs the full image size.
                let (w, h) = self
                    .session
                    .base_image
                    .as_ref()
                    .map(|b| {
                        use image::GenericImageView;
                        let (w, h) = b.dimensions();
                        (w as f32, h as f32)
                    })
                    .unwrap_or((1000.0, 1000.0));
                let label = masks::MASK_TYPES
                    .iter()
                    .find(|(_, t)| *t == ty)
                    .map(|(l, _)| *l)
                    .unwrap_or(ty);
                self.session.masks.push(masks::new_mask(label, ty, w, h));
                self.selected_mask = Some(self.session.masks.len() - 1);
                self.masks_panel
                    .rebuild(&self.session.masks, self.selected_mask, &sender);
                // Radial/linear: arm "draw to place" so a drag on the image
                // defines the new mask's geometry (sub-mask 0 of the container).
                if matches!(ty, "radial" | "linear") {
                    self.canvas.set_mask_draw(Some((0, ty == "radial")));
                }
                self.schedule_history(&sender);
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::SelectMask(idx) => {
                self.selected_mask = idx;
                // Disarm painting/picking (they target the prior selection).
                if self.paint_sub.take().is_some() {
                    self.canvas.set_paint(None);
                }
                self.canvas.set_pick(None);
                self.canvas.set_mask_draw(None);
                self.masks_panel
                    .rebuild(&self.session.masks, self.selected_mask, &sender);
                self.refresh_mask_preview(&sender);
            }
            AppMsg::DeleteMask(i) => {
                if i < self.session.masks.len() {
                    self.session.masks.remove(i);
                    // Keep the selection valid after removal.
                    self.selected_mask = match self.selected_mask {
                        Some(s) if s == i => None,
                        Some(s) if s > i => Some(s - 1),
                        other => other,
                    };
                    self.masks_panel
                        .rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::ToggleMaskVisible(i) => {
                if let Some(m) = self.session.masks.get_mut(i) {
                    m.visible = !m.visible;
                    self.masks_panel
                        .rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::ToggleMaskInvert(i) => {
                if let Some(m) = self.session.masks.get_mut(i) {
                    m.invert = !m.invert;
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::SetMaskOpacity(i, v) => {
                if let Some(m) = self.session.masks.get_mut(i) {
                    m.opacity = v as f32;
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::MaskAdjust { index, key, value } => {
                if let Some(m) = self.session.masks.get_mut(index) {
                    if let Some(obj) = m.adjustments.as_object_mut() {
                        obj.insert(key.to_string(), serde_json::json!(value));
                    }
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::MaskGrade { index, zone, hue, sat, lum } => {
                if let Some(m) = self.session.masks.get_mut(index) {
                    // adjustments.colorGrading.<zone> = { hue, saturation, luminance }
                    // (UI units; the engine divides per SCALES.)
                    let cg = mask_cg_obj(&mut m.adjustments);
                    cg.insert(
                        zone.to_string(),
                        serde_json::json!({
                            "hue": hue,
                            "saturation": sat * 100.0,
                            "luminance": lum,
                        }),
                    );
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::MaskGradeScalar { index, key, value } => {
                if let Some(m) = self.session.masks.get_mut(index) {
                    mask_cg_obj(&mut m.adjustments).insert(key.to_string(), serde_json::json!(value));
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::MaskHsl { index, band, comp, value } => {
                if let Some(m) = self.session.masks.get_mut(index) {
                    mask_nested(&mut m.adjustments, "hsl", band)
                        .insert(comp.to_string(), serde_json::json!(value));
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::MaskCurve { index, channel, points } => {
                if let Some(m) = self.session.masks.get_mut(index) {
                    let key = match channel {
                        Channel::Luma => "luma",
                        Channel::Red => "red",
                        Channel::Green => "green",
                        Channel::Blue => "blue",
                    };
                    let arr: Vec<serde_json::Value> = points
                        .iter()
                        .map(|&(x, y)| serde_json::json!({ "x": x, "y": y }))
                        .collect();
                    mask_nested_1(&mut m.adjustments, "curves").insert(key.to_string(), arr.into());
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::SetSubMaskParam {
                mask,
                sub,
                key,
                value,
            } => {
                if let Some(sm) = self
                    .session
                    .masks
                    .get_mut(mask)
                    .and_then(|m| m.sub_masks.get_mut(sub))
                {
                    if !sm.parameters.is_object() {
                        sm.parameters = serde_json::json!({});
                    }
                    if let Some(obj) = sm.parameters.as_object_mut() {
                        obj.insert(key.to_string(), serde_json::json!(value));
                    }
                    // No rebuild: the spin row already shows the value.
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::SetSubMaskMode { mask, sub, mode } => {
                if let Some(sm) = self
                    .session
                    .masks
                    .get_mut(mask)
                    .and_then(|m| m.sub_masks.get_mut(sub))
                {
                    sm.mode = masks::mode_from_index(mode);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::EditMaskGeom(shape) => {
                use image::GenericImageView;
                let Some((w, h)) = self
                    .session
                    .base_image
                    .as_ref()
                    .map(|b| b.dimensions())
                    .map(|(w, h)| (w as f64, h as f64))
                else {
                    return;
                };
                // Denormalize into full-res pixels and write only the dragged
                // keys (feather/rotation/range stay as set in the spin rows).
                let (sub, keys): (usize, Vec<(&str, f64)>) = match shape {
                    editor::MaskShape::Radial { sub, cx, cy, rx, ry, .. } => (
                        sub,
                        vec![
                            ("centerX", cx * w),
                            ("centerY", cy * h),
                            ("radiusX", rx * w),
                            ("radiusY", ry * h),
                        ],
                    ),
                    editor::MaskShape::Linear { sub, x1, y1, x2, y2, .. } => (
                        sub,
                        vec![
                            ("startX", x1 * w),
                            ("startY", y1 * h),
                            ("endX", x2 * w),
                            ("endY", y2 * h),
                        ],
                    ),
                };
                if let Some(sm) = self
                    .session
                    .masks
                    .get_mut(self.selected_mask.unwrap_or(usize::MAX))
                    .and_then(|m| m.sub_masks.get_mut(sub))
                {
                    if let Some(obj) = sm.parameters.as_object_mut() {
                        for (k, v) in keys {
                            obj.insert(k.to_string(), serde_json::json!(v));
                        }
                    }
                    // ponytail: spin rows stay stale until the panel rebuilds
                    // (reselect); the overlay updates live. Rebuild on every drag
                    // tick would thrash the panel.
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::SetBrushSize(px) => {
                self.brush_size = px.max(1.0);
                // Re-arm so the live brush preview tracks the new size.
                if let Some(sub) = self.paint_sub {
                    sender.input(AppMsg::ArmPaint(Some(sub)));
                }
            }
            AppMsg::SetBrushFeather(v) => {
                self.brush_feather = v.clamp(0.0, 100.0);
            }
            AppMsg::SetBrushErase(on) => {
                self.brush_erase = on;
                // Re-arm so the live preview + stroke tool reflect the new mode.
                if let Some(sub) = self.paint_sub {
                    sender.input(AppMsg::ArmPaint(Some(sub)));
                }
            }
            AppMsg::ArmPaint(sub) => {
                self.paint_sub = sub;
                let erase = self.brush_erase;
                let arm = sub.and_then(|s| {
                    self.image_dims().map(|(w, _)| (s, self.brush_size / w, erase))
                });
                self.canvas.set_paint(arm);
            }
            AppMsg::AddBrushStroke { sub, points, erase } => {
                let Some((w, h)) = self.image_dims() else { return };
                if let Some(m) = self.session.masks.get_mut(self.selected_mask.unwrap_or(usize::MAX)) {
                    if let Some(sm) = m.sub_masks.get_mut(sub) {
                        let is_flow = sm.mask_type == "flow";
                        let pts: Vec<serde_json::Value> = points
                            .iter()
                            .map(|(x, y)| serde_json::json!({ "x": x * w, "y": y * h }))
                            .collect();
                        let mut line = serde_json::json!({
                            "tool": if erase { "eraser" } else { "brush" },
                            "brushSize": self.brush_size,
                            "feather": self.brush_feather / 100.0,
                            "points": pts,
                        });
                        if is_flow {
                            // Flow lines carry a per-line flow (default matches core).
                            line["flow"] = serde_json::json!(10.0);
                        }
                        let obj = sm.parameters.as_object_mut();
                        if let Some(obj) = obj {
                            obj.entry("lines")
                                .or_insert_with(|| serde_json::json!([]));
                            if let Some(arr) = obj["lines"].as_array_mut() {
                                arr.push(line);
                            }
                        }
                        self.schedule_history(&sender);
                        sender.input(AppMsg::RequestRender);
                    }
                }
            }
            AppMsg::ClearStrokes(sub) => {
                if let Some(sm) = self
                    .session
                    .masks
                    .get_mut(self.selected_mask.unwrap_or(usize::MAX))
                    .and_then(|m| m.sub_masks.get_mut(sub))
                {
                    if let Some(obj) = sm.parameters.as_object_mut() {
                        obj.insert("lines".into(), serde_json::json!([]));
                    }
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::GenerateAiMask(sub) => {
                let Some(mi) = self.selected_mask else { return };
                let Some(base) = self.session.base_image.clone() else { return };
                let Some(sm) = self
                    .session
                    .masks
                    .get(mi)
                    .and_then(|m| m.sub_masks.get(sub))
                else {
                    return;
                };
                let mask_type = sm.mask_type.clone();
                let params = sm.parameters.clone();
                let geom = self.geom;
                // Persistent status while inference runs (first run also downloads
                // the models, which can take a while).
                self.win_title.set_subtitle("Generating AI mask…");
                spawn_bg(&sender, move || {
                    let img = apply_geometry(&base, geom);
                    let (w, h) = {
                        use image::GenericImageView;
                        let (w, h) = img.dimensions();
                        (w as f64, h as f64)
                    };
                    let result = match ai_masks::Kind::from_sub(&mask_type, &params, w, h) {
                        Some(kind) => ai_masks::generate(kind, &img),
                        None => Err("not an AI mask type".to_string()),
                    };
                    CmdMsg::AiMaskReady {
                        mask: mi,
                        sub,
                        result,
                    }
                });
            }
            AppMsg::SetMaskOverlayShown(shown) => {
                self.canvas.set_mask_preview_visible(shown);
            }
            AppMsg::ArmPick(sub) => {
                let arm = sub.and_then(|s| {
                    let ty = self
                        .session
                        .masks
                        .get(self.selected_mask.unwrap_or(usize::MAX))
                        .and_then(|m| m.sub_masks.get(s))
                        .map(|sm| sm.mask_type.as_str());
                    ty.map(|t| (s, matches!(t, "ai-subject" | "quick-eraser")))
                });
                self.canvas.set_pick(arm);
            }
            AppMsg::PickResult { sub, x1, y1, x2, y2 } => {
                let Some((w, h)) = self.image_dims() else { return };
                let Some(mi) = self.selected_mask else { return };
                let Some(sm) = self
                    .session
                    .masks
                    .get_mut(mi)
                    .and_then(|m| m.sub_masks.get_mut(sub))
                else {
                    return;
                };
                let is_subject = matches!(sm.mask_type.as_str(), "ai-subject" | "quick-eraser");
                if let Some(obj) = sm.parameters.as_object_mut() {
                    if is_subject {
                        obj.insert("startX".into(), serde_json::json!(x1 * w));
                        obj.insert("startY".into(), serde_json::json!(y1 * h));
                        obj.insert("endX".into(), serde_json::json!(x2 * w));
                        obj.insert("endY".into(), serde_json::json!(y2 * h));
                    } else {
                        obj.insert("targetX".into(), serde_json::json!(x1 * w));
                        obj.insert("targetY".into(), serde_json::json!(y1 * h));
                    }
                }
                if is_subject {
                    // Box drawn -> re-run SAM with the new prompt.
                    sender.input(AppMsg::GenerateAiMask(sub));
                } else {
                    self.schedule_history(&sender);
                    self.refresh_mask_overlay();
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::AddSubMask(mask, ty) => {
                let (w, h) = self
                    .session
                    .base_image
                    .as_ref()
                    .map(|b| {
                        use image::GenericImageView;
                        let (w, h) = b.dimensions();
                        (w as f32, h as f32)
                    })
                    .unwrap_or((1000.0, 1000.0));
                if let Some(m) = self.session.masks.get_mut(mask) {
                    // Reuse new_mask's sub-mask seeding, then move it onto this
                    // container (keeps default geometry/params in one place).
                    let label = masks::MASK_TYPES
                        .iter()
                        .find(|(_, t)| *t == ty)
                        .map(|(l, _)| *l)
                        .unwrap_or(ty);
                    let sub = masks::new_mask(label, ty, w, h).sub_masks.remove(0);
                    m.sub_masks.push(sub);
                    let new_sub = m.sub_masks.len() - 1;
                    self.masks_panel
                        .rebuild(&self.session.masks, self.selected_mask, &sender);
                    // Arm "draw to place" for a radial/linear sub-mask too.
                    if matches!(ty, "radial" | "linear") {
                        self.canvas.set_mask_draw(Some((new_sub, ty == "radial")));
                    }
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::DeleteSubMask { mask, sub } => {
                if let Some(m) = self.session.masks.get_mut(mask) {
                    if sub < m.sub_masks.len() {
                        m.sub_masks.remove(sub);
                        self.masks_panel
                            .rebuild(&self.session.masks, self.selected_mask, &sender);
                        self.schedule_history(&sender);
                        sender.input(AppMsg::RequestRender);
                    }
                }
            }
            AppMsg::ToggleSubMaskVisible { mask, sub } => {
                if let Some(sm) = self
                    .session
                    .masks
                    .get_mut(mask)
                    .and_then(|m| m.sub_masks.get_mut(sub))
                {
                    sm.visible = !sm.visible;
                    self.masks_panel
                        .rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::ToggleSubMaskInvert { mask, sub } => {
                if let Some(sm) = self
                    .session
                    .masks
                    .get_mut(mask)
                    .and_then(|m| m.sub_masks.get_mut(sub))
                {
                    sm.invert = !sm.invert;
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::CopySettings => {
                self.settings_clip = Some(SettingsClip {
                    global: self.session.adjustments.global,
                    geom: self.geom,
                    lut: self.session.lut.clone(),
                    lut_path: self.lut_path.clone(),
                    vals: self.panel.snapshot(),
                });
                self.toasts.add_toast(adw::Toast::new("Settings copied"));
            }
            AppMsg::PasteSettings => {
                if let Some(c) = self.settings_clip.clone() {
                    self.session.adjustments.global = c.global;
                    self.geom = c.geom;
                    self.session.lut = c.lut;
                    self.lut_path = c.lut_path;
                    self.canvas.set_crop_rect(match c.geom.crop {
                        Some([x, y, w, h]) => (x as f64, y as f64, w as f64, h as f64),
                        None => (0.0, 0.0, 1.0, 1.0),
                    });
                    self.panel.restore(&c.vals);
                    self.panel.sync(&self.session.adjustments.global);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                    self.toasts.add_toast(adw::Toast::new("Settings pasted"));
                }
            }
            AppMsg::ShowAbout => {
                let about = adw::AboutWindow::builder()
                    .application_name("RapidRAW")
                    .application_icon("image-x-generic")
                    .developer_name("RapidRAW")
                    .version(env!("CARGO_PKG_VERSION"))
                    .comments("GPU-accelerated RAW editor — native GTK4/libadwaita frontend.")
                    .build();
                about.set_transient_for(Some(root));
                about.present();
            }
            AppMsg::ToggleFullscreen => {
                if root.is_fullscreen() {
                    root.unfullscreen();
                } else {
                    root.fullscreen();
                }
            }
            AppMsg::RateActive(r) => {
                // Only rate while an image is open in the editor.
                let in_editor = widgets
                    .nav
                    .visible_page()
                    .and_then(|p| p.tag())
                    .map_or(false, |t| t == "editor");
                if !in_editor {
                    return;
                }
                let Some(path) = self.session.active_path.clone() else {
                    return;
                };
                if r == 0 {
                    self.ratings.remove(&path);
                } else {
                    self.ratings.insert(path.clone(), r);
                }
                save_ratings(&self.ratings);
                // Reflect on the matching grid thumbnail.
                if let Some(i) = self.images.iter().position(|p| *p == path) {
                    self.thumbs.send(i, ThumbMsg::SetRating(r));
                }
                self.editor_stars.emit(StarsMsg::External(r));
            }
            AppMsg::RateThumb(path, n) => {
                let cur = self.ratings.get(&path).copied().unwrap_or(0);
                let r = if cur == n { 0 } else { n };
                if r == 0 {
                    self.ratings.remove(&path);
                } else {
                    self.ratings.insert(path.clone(), r);
                }
                save_ratings(&self.ratings);
                if let Some(i) = self.images.iter().position(|p| *p == path) {
                    self.thumbs.send(i, ThumbMsg::SetRating(r));
                }
                if self.session.active_path.as_deref() == Some(path.as_path()) {
                    self.editor_stars.emit(StarsMsg::External(r));
                }
            }
            AppMsg::OpenInEditor(path) => {
                log::info!("Open in editor: {}", path.display());
                // Persist the previously-open image's edits before switching.
                self.save_edits();
                // Pause thumbnail decoding while editing (frees the CPU).
                self.thumb_gen.fetch_add(1, Ordering::Relaxed);
                self.session.active_path = Some(path.clone());
                // Blank the canvas so the previous photo isn't shown while the
                // new selection decodes.
                self.canvas.clear();
                self.last_tex = None;
                self.original_tex = None;
                // Reset all controls to defaults *now* (in place, no rebuild) so
                // the previous photo's slider/curve/wheel state isn't shown while
                // the new image decodes. Saved edits (if any) are applied in
                // BaseReady, after decode. Cheap → opening stays fluid.
                self.geom = Geometry::default();
                self.crop_aspect = 0.0;
                self.crop_active = false;
                self.canvas.reset_crop();
                self.session.adjustments = Default::default();
                controls::init_defaults(&mut self.session.adjustments.global);
                self.session.lut = None;
                self.lut_path = None;
                self.session.masks.clear();
                self.selected_mask = None;
                self.masks_panel.rebuild(&self.session.masks, None, &sender);
                self.panel.reset();
                // Crop panel is small; rebuild so its toggles/straighten reset.
                let fresh = crop::CropPanel::new(&sender);
                self.content_stack.remove(self.crop.root());
                self.content_stack.add_named(fresh.root(), Some("crop"));
                self.crop = fresh;
                self.content_stack.set_visible_child_name("adjust");
                // Reset the tab switcher to Edit (opening from another tab, e.g.
                // Masks, must not leave that button highlighted).
                self.edit_btn.set_active(true);
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("RapidRAW");
                self.win_title.set_title(name);
                self.win_title.set_subtitle("");
                // Only push the editor page if we're not already on it (opening a
                // second image from the filmstrip stays in the editor) — pushing a
                // tag already in the stack is an Adwaita CRITICAL.
                let in_editor = widgets
                    .nav
                    .visible_page()
                    .and_then(|p| p.tag())
                    .map_or(false, |t| t == "editor");
                if !in_editor {
                    widgets.nav.push_by_tag("editor");
                }
                let r = self.ratings.get(&path).copied().unwrap_or(0);
                self.editor_stars.emit(StarsMsg::External(r));
                let p = path.clone();
                spawn_bg(&sender, move || match rapidraw_core::load_base_image(&p) {
                    Ok(img) => CmdMsg::BaseReady(p, img),
                    Err(e) => {
                        log::warn!("base decode failed for {}: {e}", p.display());
                        CmdMsg::BaseReady(p, DynamicImage::new_rgba8(1, 1))
                    }
                });
            }
            AppMsg::Adjust(Adjust { set, value }) => {
                set(&mut self.session.adjustments.global, value);
                self.schedule_history(&sender);
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::RequestRender => {
                // Debounce via generation token: bump it and let only the latest
                // timer fire a render. Rapid slider drags thus collapse into one.
                self.render_gen = self.render_gen.wrapping_add(1);
                let gen = self.render_gen;
                let sender = sender.clone();
                glib::timeout_add_local_once(Duration::from_millis(RENDER_DEBOUNCE_MS), move || {
                    sender.input(AppMsg::DoRender(gen))
                });
            }
            AppMsg::DoRender(gen) => {
                // Stale timer from a superseded RequestRender: ignore.
                if gen != self.render_gen {
                    return;
                }
                // Guard: nothing to render until a base image is loaded.
                let Some(base) = self.session.base_image.clone() else {
                    return;
                };
                // Hand off to the persistent render thread (reuses the cached
                // GpuProcessor, so slider drags stay smooth).
                // While editing the crop, show the full image so the overlay can
                // be adjusted against it.
                let mut geom = self.geom;
                if self.crop_active {
                    geom.crop = None;
                }
                let _ = self.render_tx.send(RenderJob::Preview {
                    base,
                    adj: Box::new(self.session.adjustments),
                    masks: self.session.masks.clone(),
                    lut: self.session.lut.clone(),
                    dim: self.settings.preview_dim,
                    geom,
                });
                // Mask edits all funnel through here (they RequestRender), so this
                // is the natural debounced point to refresh the coverage overlay.
                self.refresh_mask_preview(&sender);
            }
            AppMsg::ExportDialog => {
                if self.session.base_image.is_none() {
                    log::warn!("export: no image open");
                    return;
                }
                // Small modal: choose format + JPEG quality, then the save dialog.
                let win = gtk::Window::builder()
                    .title("Export options")
                    .modal(true)
                    .transient_for(root)
                    .default_width(280)
                    .build();
                let vb = gtk::Box::new(gtk::Orientation::Vertical, 8);
                vb.set_margin_all(12);

                let fmt = gtk::DropDown::from_strings(&[
                    "JPEG", "PNG", "TIFF", "WebP", "JPEG XL", "AVIF", "CUBE LUT",
                ]);
                let idx_to_format = |i: u32| match i {
                    1 => ExportFormat::Png,
                    2 => ExportFormat::Tiff,
                    3 => ExportFormat::Webp,
                    4 => ExportFormat::Jxl,
                    5 => ExportFormat::Avif,
                    6 => ExportFormat::CubeLut,
                    _ => ExportFormat::Jpeg,
                };

                let q = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
                q.set_value(90.0);
                let qrow = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let qlabel = gtk::Label::new(Some("Quality"));
                qrow.append(&qlabel);
                qrow.append(&q);

                // Resize: mode + value (0 = full) + don't-enlarge.
                let resize_mode = gtk::DropDown::from_strings(&["No resize", "Long edge", "Width", "Height"]);
                let resize = gtk::SpinButton::with_range(1.0, 20000.0, 100.0);
                resize.set_value(2048.0);
                let dont_enlarge = gtk::CheckButton::with_label("Don't enlarge");
                dont_enlarge.set_active(true);
                let rrow = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                rrow.append(&gtk::Label::new(Some("Resize")));
                rrow.append(&resize_mode);
                rrow.append(&resize);
                let rrow2 = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                rrow2.append(&dont_enlarge);
                {
                    // value + don't-enlarge only relevant when a resize mode is set.
                    let resize = resize.clone();
                    let dont_enlarge = dont_enlarge.clone();
                    let update = move |d: &gtk::DropDown| {
                        let on = d.selected() != 0;
                        resize.set_sensitive(on);
                        dont_enlarge.set_sensitive(on);
                    };
                    update(&resize_mode);
                    resize_mode.connect_selected_notify(update);
                }

                // Reactively show settings relevant to the selected format.
                {
                    let qrow = qrow.clone();
                    let rrow = rrow.clone();
                    let qlabel = qlabel.clone();
                    let q = q.clone();
                    let rrow2 = rrow2.clone();
                    let update = move |d: &gtk::DropDown| {
                        let f = idx_to_format(d.selected());
                        let raster = !matches!(f, ExportFormat::CubeLut);
                        qrow.set_visible(f.has_quality());
                        rrow.set_visible(raster);
                        rrow2.set_visible(raster);
                        // JXL at 100 = lossless.
                        qlabel.set_text(if matches!(f, ExportFormat::Jxl) && q.value() >= 100.0 {
                            "Quality (lossless)"
                        } else {
                            "Quality"
                        });
                    };
                    update(&fmt);
                    fmt.connect_selected_notify(update);
                }

                let go = gtk::Button::with_label("Export…");
                go.add_css_class("suggested-action");
                vb.append(&fmt);
                vb.append(&qrow);
                vb.append(&rrow);
                vb.append(&rrow2);
                vb.append(&go);
                win.set_child(Some(&vb));

                let sender = sender.clone();
                let win_c = win.clone();
                go.connect_clicked(move |_| {
                    let format = idx_to_format(fmt.selected());
                    win_c.close();
                    if matches!(format, ExportFormat::CubeLut) {
                        sender.input(AppMsg::ExportLutDialog);
                        return;
                    }
                    let resize = match resize_mode.selected() {
                        1 => Some(ResizeMode::LongEdge),
                        2 => Some(ResizeMode::Width),
                        3 => Some(ResizeMode::Height),
                        _ => None,
                    }
                    .map(|mode| Resize {
                        mode,
                        value: resize.value() as u32,
                        dont_enlarge: dont_enlarge.is_active(),
                    });
                    sender.input(AppMsg::ExportConfigured(ExportOpts {
                        format,
                        quality: q.value() as u8,
                        resize,
                    }));
                });
                win.present();
            }
            AppMsg::ExportConfigured(opts) => {
                let ext = opts.format.ext();
                let suggested = self
                    .session
                    .active_path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| format!("{}.{ext}", s.to_string_lossy()))
                    .unwrap_or_else(|| format!("export.{ext}"));
                let dialog = gtk::FileDialog::builder()
                    .title("Export")
                    .initial_name(suggested)
                    .build();
                let parent = root.clone();
                let sender = sender.clone();
                dialog.save(Some(&parent), gtk::gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res {
                        if let Some(path) = file.path() {
                            sender.input(AppMsg::ExportTo(path, opts));
                        }
                    }
                });
            }
            AppMsg::ExportTo(path, opts) => {
                let Some(base) = self.session.base_image.clone() else {
                    return;
                };
                log::info!("exporting to {}", path.display());
                let _ = self.render_tx.send(RenderJob::Export {
                    base,
                    adj: Box::new(self.session.adjustments),
                    masks: self.session.masks.clone(),
                    lut: self.session.lut.clone(),
                    path,
                    opts,
                    geom: self.geom,
                });
            }
            AppMsg::LoadLut => {
                let filter = gtk::FileFilter::new();
                filter.set_name(Some("3D LUT (.cube, .3dl)"));
                filter.add_pattern("*.cube");
                filter.add_pattern("*.3dl");
                let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                let dialog = gtk::FileDialog::builder()
                    .title("Load LUT")
                    .filters(&filters)
                    .build();
                let parent = root.clone();
                let sender = sender.clone();
                dialog.open(Some(&parent), gtk::gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res {
                        if let Some(path) = file.path() {
                            sender.input(AppMsg::LutChosen(path));
                        }
                    }
                });
            }
            AppMsg::LutChosen(path) => match parse_lut_file(&path.to_string_lossy()) {
                Ok(lut) => {
                    log::info!("LUT loaded: {}", path.display());
                    self.session.lut = Some(Arc::new(lut));
                    self.lut_path = Some(path.clone());
                    // Default to full strength so the effect is visible at once;
                    // the LUT intensity slider (0..100) overrides this.
                    if self.session.adjustments.global.lut_intensity <= 0.0 {
                        self.session.adjustments.global.lut_intensity = 1.0;
                    }
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
                Err(e) => log::warn!("LUT parse failed for {}: {e}", path.display()),
            },
            AppMsg::ClearLut => {
                self.session.lut = None;
                self.lut_path = None;
                self.schedule_history(&sender);
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::ExportLutDialog => {
                let dialog = gtk::FileDialog::builder()
                    .title("Export LUT")
                    .initial_name("look.cube")
                    .build();
                let parent = root.clone();
                let sender = sender.clone();
                dialog.save(Some(&parent), gtk::gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res {
                        if let Some(path) = file.path() {
                            sender.input(AppMsg::ExportLutTo(path));
                        }
                    }
                });
            }
            AppMsg::ExportLutTo(path) => {
                log::info!("exporting LUT to {}", path.display());
                let _ = self.render_tx.send(RenderJob::ExportLut {
                    adj: Box::new(self.session.adjustments),
                    lut: self.session.lut.clone(),
                    path,
                });
            }
            AppMsg::ShowLibrary => {
                // Fired when the editor page is popped (auto back button/gesture).
                self.save_edits();
                // Resume decoding any thumbnails that never finished.
                let missing: Vec<usize> = self
                    .thumb_loaded
                    .iter()
                    .enumerate()
                    .filter(|(_, done)| !**done)
                    .map(|(i, _)| i)
                    .collect();
                if !missing.is_empty() {
                    let gen = self.thumb_gen.fetch_add(1, Ordering::Relaxed) + 1;
                    dispatch_thumbs(
                        &sender,
                        &self.thumb_gen,
                        gen,
                        self.settings.thumb_dim,
                        &self.images,
                        missing,
                    );
                }
            }
            AppMsg::OpenSettings => {
                settings::present(root, self.settings, &sender);
            }
            AppMsg::SettingsChanged(s) => {
                self.settings = s;
                save_settings(&s); // persist so the next launch restores them
                self.canvas.set_background(s.background);
                // Re-render the preview at the (possibly new) preview size.
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::CurveChanged(ch, pts) => {
                let g = &mut self.session.adjustments.global;
                let (arr, count) = match ch {
                    Channel::Luma => (&mut g.luma_curve, &mut g.luma_curve_count),
                    Channel::Red => (&mut g.red_curve, &mut g.red_curve_count),
                    Channel::Green => (&mut g.green_curve, &mut g.green_curve_count),
                    Channel::Blue => (&mut g.blue_curve, &mut g.blue_curve_count),
                };
                for (i, (x, y)) in pts.iter().take(16).enumerate() {
                    arr[i] = Point::new(*x, *y);
                }
                // count < 2 means "identity" in the shader, so a 2-point line is a no-op.
                *count = pts.len().min(16) as u32;
                self.schedule_history(&sender);
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::CommitHistory(gen) => {
                // Stale timer from a superseded schedule_history: ignore.
                if gen != self.hist_gen {
                    return;
                }
                if self.suppress_history {
                    return;
                }
                let cur = self.session.adjustments;
                let lut = self.session.lut.clone();
                let masks = self.session.masks.clone();
                let masks_json = serde_json::to_value(&masks).unwrap_or_default();
                let same = self
                    .history
                    .get(self.hist_idx)
                    .map(|e| {
                        bytemuck::bytes_of(&e.adj.global) == bytemuck::bytes_of(&cur.global)
                            && lut_eq(&e.lut, &lut)
                            && serde_json::to_value(&e.masks).unwrap_or_default() == masks_json
                    })
                    .unwrap_or(false);
                if same {
                    return;
                }
                self.history.truncate(self.hist_idx + 1);
                self.history.push(HistEntry {
                    adj: cur,
                    lut,
                    vals: self.panel.snapshot(),
                    masks,
                });
                self.hist_idx = self.history.len() - 1;
                self.save_edits();
            }
            AppMsg::Undo => {
                if self.hist_idx > 0 {
                    self.hist_idx -= 1;
                    self.apply_history(&sender);
                }
            }
            AppMsg::Redo => {
                if self.hist_idx + 1 < self.history.len() {
                    self.hist_idx += 1;
                    self.apply_history(&sender);
                }
            }
            AppMsg::ToggleOriginal => {
                self.showing_original = !self.showing_original;
                // Swap to the "concealed" eye icon while showing the original so
                // the active state reads clearly (the toggle highlight aside).
                widgets.orig_btn.set_icon_name(if self.showing_original {
                    "view-conceal-symbolic"
                } else {
                    "view-reveal-symbolic"
                });
                self.show_active_tex();
            }
            AppMsg::ToggleClipping(on) => {
                self.clipping = on;
                self.clip_tex = if on {
                    self.last_rgba.as_ref().map(build_clip_tex)
                } else {
                    None
                };
                self.show_active_tex();
            }
            AppMsg::ShowAlbum(images) => {
                let paths: Vec<PathBuf> = images
                    .into_iter()
                    .map(PathBuf::from)
                    .filter(|p| p.exists())
                    .collect();
                self.all_images = paths;
                self.apply_library(&sender);
            }
            AppMsg::AlbumNew(name) => {
                let id = format!(
                    "album-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0)
                );
                self.albums.push(rapidraw_core::albums::AlbumItem::Album {
                    id,
                    name,
                    icon: None,
                    images: vec![],
                });
                self.persist_albums();
            }
            AppMsg::AlbumRename { id, name } => {
                rename_album(&mut self.albums, &id, &name);
                self.persist_albums();
            }
            AppMsg::AlbumDelete(id) => {
                delete_album(&mut self.albums, &id);
                self.persist_albums();
            }
        }
        // Keep the mask overlay in sync with selection/geometry/tab after any
        // message (cheap no-op when the Masks tab isn't showing).
        self.refresh_mask_overlay();
        self.update_view(widgets, sender);
    }

    fn update_cmd(
        &mut self,
        msg: Self::CommandOutput,
        sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match msg {
            CmdMsg::ThumbReady(gen, i, rgba) => {
                // Ignore stale (paused/cancelled) generations and skip markers.
                if gen != self.thumb_gen.load(Ordering::Relaxed)
                    || (rgba.width() <= 1 && rgba.height() <= 1)
                {
                    return;
                }
                if let Some(done) = self.thumb_loaded.get_mut(i) {
                    *done = true;
                }
                let tex = library::texture_from_rgba(&rgba);
                self.thumbs.send(i, ThumbMsg::SetTexture(tex));
            }
            CmdMsg::BaseReady(path, img) => {
                let (w, h) = img.dimensions();
                log::info!("base image ready: {} ({w}x{h})", path.display());
                // Controls were already reset to defaults in OpenInEditor; here we
                // just fill the EXIF subtitle and apply any saved edits.
                self.win_title
                    .set_subtitle(&meta::read_summary(&path).unwrap_or_default());
                if !self.settings.reset_on_open {
                    if let Some(e) = sidecar::load(&path) {
                        if e.global.len() == std::mem::size_of::<GlobalAdjustments>() {
                            // pod_read_unaligned: the JSON-decoded Vec<u8> has no
                            // alignment guarantee, unlike `from_bytes`.
                            self.session.adjustments.global =
                                bytemuck::pod_read_unaligned::<GlobalAdjustments>(&e.global);
                        }
                        self.geom.orientation_steps = e.orientation_steps;
                        self.geom.flip_h = e.flip_h;
                        self.geom.flip_v = e.flip_v;
                        self.geom.straighten = e.straighten;
                        self.geom.crop = e.crop;
                        self.canvas.set_crop_rect(match e.crop {
                            Some([x, y, w, h]) => (x as f64, y as f64, w as f64, h as f64),
                            None => (0.0, 0.0, 1.0, 1.0),
                        });
                        if let Some(lp) = &e.lut {
                            if let Ok(l) = parse_lut_file(lp) {
                                self.session.lut = Some(Arc::new(l));
                                self.lut_path = Some(PathBuf::from(lp));
                            }
                        }
                        self.panel.restore(&e.vals);
                        self.panel.sync(&self.session.adjustments.global);
                        self.session.masks = e.masks;
                        self.selected_mask = None;
                        self.masks_panel.rebuild(&self.session.masks, None, &sender);
                    }
                }
                // Show the un-adjusted base immediately. We're on the GTK main
                // thread here, so building the gdk texture is safe.
                let rgba = img.to_rgba8();
                let tex = library::texture_from_rgba(&rgba);
                self.canvas.set_texture(&tex);
                // The unedited image at preview size, for the before/after toggle.
                let preview = self.settings.preview_dim;
                let orig = if w.max(h) > preview {
                    img.resize(preview, preview, image::imageops::FilterType::Lanczos3)
                } else {
                    img.clone()
                };
                self.original_tex = Some(library::texture_from_rgba(&orig.to_rgba8()));
                self.last_tex = Some(tex);
                self.showing_original = false;
                self.session.base_image = Some(Arc::new(img));
                // Seed the undo history with this image's starting state.
                self.history = vec![HistEntry {
                    adj: self.session.adjustments,
                    lut: self.session.lut.clone(),
                    vals: self.panel.snapshot(),
                    masks: self.session.masks.clone(),
                }];
                self.hist_idx = 0;
                // Kick an initial engine render so the preview reflects the
                // current adjustment stack (Phase 9).
                sender.input(AppMsg::RequestRender);
            }
            CmdMsg::RenderReady(rgba) => {
                // A 1x1 image signals a failed/empty render: ignore it so the
                // previous preview stays on screen.
                if rgba.width() <= 1 && rgba.height() <= 1 {
                    return;
                }
                self.scopes.set_data(&rgba);
                self.last_tex = Some(library::texture_from_rgba(&rgba));
                self.clip_tex = if self.clipping {
                    Some(build_clip_tex(&rgba))
                } else {
                    None
                };
                self.last_rgba = Some(rgba);
                // Preserve the user's zoom/pan across preview updates. Don't
                // clobber the canvas while the user is viewing the original.
                if !self.showing_original {
                    self.show_active_tex();
                }
            }
            CmdMsg::ExportDone(Ok(path)) => {
                log::info!("export saved: {}", path.display());
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.toasts
                    .add_toast(adw::Toast::new(&format!("Saved {name}")));
            }
            CmdMsg::ExportDone(Err(e)) => {
                log::warn!("export failed: {e}");
                self.toasts
                    .add_toast(adw::Toast::new(&format!("Export failed: {e}")));
            }
            CmdMsg::AiMaskReady {
                mask,
                sub,
                result,
            } => {
                // Clear the "Generating…" status (back to the EXIF summary).
                let subtitle = self
                    .session
                    .active_path
                    .as_deref()
                    .and_then(meta::read_summary)
                    .unwrap_or_default();
                self.win_title.set_subtitle(&subtitle);
                match result {
                Ok(b64) => {
                    if let Some(sm) = self
                        .session
                        .masks
                        .get_mut(mask)
                        .and_then(|m| m.sub_masks.get_mut(sub))
                    {
                        if let Some(obj) = sm.parameters.as_object_mut() {
                            obj.insert("maskDataBase64".into(), serde_json::json!(b64));
                            // Inference ran in render space, so the transform is
                            // identity (resolver only scales).
                            obj.insert("rotation".into(), serde_json::json!(0.0));
                            obj.insert("flipHorizontal".into(), serde_json::json!(false));
                            obj.insert("flipVertical".into(), serde_json::json!(false));
                            obj.insert("orientationSteps".into(), serde_json::json!(0));
                        }
                        self.schedule_history(&sender);
                        self.refresh_mask_overlay();
                        sender.input(AppMsg::RequestRender);
                        self.toasts.add_toast(adw::Toast::new("AI mask generated"));
                    }
                }
                Err(e) => {
                    log::warn!("AI mask failed: {e}");
                    self.toasts
                        .add_toast(adw::Toast::new(&format!("AI mask failed: {e}")));
                }
                }
            }
            CmdMsg::MaskPreviewReady { gen, data } => {
                if gen == self.mpreview_gen {
                    self.canvas.set_mask_preview(data);
                }
            }
        }
    }
}

/// Get (creating if needed) the `colorGrading` object inside a mask's
/// `adjustments` JSON, so grading writes nest under it.
fn mask_cg_obj(adj: &mut serde_json::Value) -> &mut serde_json::Map<String, serde_json::Value> {
    mask_nested_1(adj, "colorGrading")
}

/// Get (creating if needed) `adj[outer]` as an object.
fn mask_nested_1<'a>(
    adj: &'a mut serde_json::Value,
    outer: &str,
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    if !adj.is_object() {
        *adj = serde_json::json!({});
    }
    let obj = adj.as_object_mut().unwrap();
    obj.entry(outer).or_insert_with(|| serde_json::json!({}));
    obj[outer].as_object_mut().unwrap()
}

/// Get (creating if needed) `adj[outer][inner]` as an object (e.g. hsl.reds).
fn mask_nested<'a>(
    adj: &'a mut serde_json::Value,
    outer: &str,
    inner: &str,
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    let om = mask_nested_1(adj, outer);
    om.entry(inner).or_insert_with(|| serde_json::json!({}));
    om[inner].as_object_mut().unwrap()
}

/// Build the clipping-indicator texture: blown pixels (any channel 255) tinted
/// red, crushed pixels (all channels 0) blue, everything else unchanged.
fn build_clip_tex(rgba: &RgbaImage) -> gdk::MemoryTexture {
    let mut out = rgba.clone();
    for px in out.pixels_mut() {
        let [r, g, b, _] = px.0;
        if r == 255 || g == 255 || b == 255 {
            px.0 = [255, 0, 0, 255];
        } else if r == 0 && g == 0 && b == 0 {
            px.0 = [0, 0, 255, 255];
        }
    }
    library::texture_from_rgba(&out)
}

/// Encode a rendered image to `path` per `opts` (format, JPEG quality, resize).
/// Install app-wide CSS once: a nicer Paned resize handle (a thin, rounded,
/// vertically-inset grip that highlights on hover instead of a hard full-height
/// line).
fn install_app_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(
            "paned > separator {
                 min-width: 5px;
                 margin: 12px 6px;
                 border-radius: 4px;
                 background-color: alpha(@borders, 0.7);
                 transition: background-color 150ms ease;
             }
             paned > separator:hover {
                 background-color: @accent_bg_color;
             }
             .welcome-scrim {
                 background: linear-gradient(to bottom,
                     alpha(black, 0.15), alpha(black, 0.55));
             }
             .welcome-title {
                 color: white;
                 font-size: 30px;
                 font-weight: 800;
                 text-shadow: 0 2px 8px alpha(black, 0.6);
             }
             .thumb-stars {
                 margin: 4px;
                 padding: 0 3px;
                 border-radius: 7px;
                 background: alpha(black, 0.3);
             }
             .thumb-stars button.star {
                 min-width: 0;
                 min-height: 0;
                 padding: 0 1px;
                 margin: 0;
                 background: none;
                 border: none;
                 box-shadow: none;
                 font-size: 11px;
                 color: alpha(white, 0.85);
                 text-shadow: 0 1px 2px alpha(black, 0.8);
             }
             .thumb-stars button.star:hover {
                 color: white;
                 background: none;
             }",
        );
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

/// Path of the file storing the last opened folder (for "Continue session").
fn state_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rapidraw-relm4").join("last_folder"))
}

fn save_last_folder(p: &std::path::Path) {
    if let Some(f) = state_file() {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(f, p.to_string_lossy().as_bytes());
    }
}

fn load_last_folder() -> Option<PathBuf> {
    let s = std::fs::read_to_string(state_file()?).ok()?;
    let p = PathBuf::from(s.trim());
    p.is_dir().then_some(p)
}

fn ratings_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rapidraw-relm4").join("ratings.json"))
}

fn albums_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rapidraw-relm4").join("albums.json"))
}

fn settings_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rapidraw-relm4").join("settings.json"))
}

/// Load persisted user settings, falling back to defaults.
fn load_settings() -> Settings {
    settings_file()
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_settings(s: &Settings) {
    let Some(f) = settings_file() else { return };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(s) {
        let _ = std::fs::write(f, json);
    }
}

fn rename_album(tree: &mut [rapidraw_core::albums::AlbumItem], target: &str, new_name: &str) {
    use rapidraw_core::albums::AlbumItem::*;
    for item in tree.iter_mut() {
        match item {
            Album { id, name, .. } if id == target => {
                *name = new_name.to_string();
                return;
            }
            Group { children, .. } => rename_album(children, target, new_name),
            _ => {}
        }
    }
}

fn delete_album(tree: &mut Vec<rapidraw_core::albums::AlbumItem>, target: &str) {
    use rapidraw_core::albums::AlbumItem::*;
    tree.retain(|i| !matches!(i, Album { id, .. } if id == target));
    for item in tree.iter_mut() {
        if let Group { children, .. } = item {
            delete_album(children, target);
        }
    }
}

fn load_ratings() -> HashMap<PathBuf, u8> {
    let Some(f) = ratings_file() else {
        return HashMap::new();
    };
    let Ok(bytes) = std::fs::read(f) else {
        return HashMap::new();
    };
    let map: HashMap<String, u8> = serde_json::from_slice(&bytes).unwrap_or_default();
    map.into_iter().map(|(k, v)| (PathBuf::from(k), v)).collect()
}

fn save_ratings(map: &HashMap<PathBuf, u8>) {
    let Some(f) = ratings_file() else { return };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let strmap: HashMap<String, u8> = map
        .iter()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), *v))
        .collect();
    if let Ok(json) = serde_json::to_vec(&strmap) {
        let _ = std::fs::write(f, json);
    }
}

/// The embedded splash image (welcome screen background), as a texture.
fn splash_texture() -> Option<gdk::MemoryTexture> {
    let bytes = include_bytes!("../../public/splash-grey.jpg");
    let img = image::load_from_memory(bytes).ok()?;
    Some(library::texture_from_rgba(&img.to_rgba8()))
}

/// Identity comparison for the active LUT (shared `Arc`, or both absent).
fn lut_eq(a: &Option<Arc<Lut>>, b: &Option<Arc<Lut>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

fn encode_image(
    img: &DynamicImage,
    path: &std::path::Path,
    opts: ExportOpts,
) -> Result<(), String> {
    let img = match opts.resize {
        Some(r) => resize_for_export(img, r),
        None => img.clone(),
    };
    let bytes = encode_image_to_bytes(&img, opts.format, opts.quality)?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// Resize `img` per the export resize options (aspect-preserving; honours
/// "don't enlarge"). Uses a large bound on the free axis for Width/Height so the
/// constrained axis lands exactly on `value`.
fn resize_for_export(img: &DynamicImage, r: Resize) -> DynamicImage {
    use image::GenericImageView;
    let (w, h) = img.dimensions();
    if r.dont_enlarge {
        let exceeds = match r.mode {
            ResizeMode::LongEdge => r.value < w.max(h),
            ResizeMode::Width => r.value < w,
            ResizeMode::Height => r.value < h,
        };
        if !exceeds {
            return img.clone();
        }
    }
    let f = image::imageops::FilterType::Lanczos3;
    match r.mode {
        ResizeMode::LongEdge => img.resize(r.value, r.value, f),
        ResizeMode::Width => img.resize(r.value, u32::MAX, f),
        ResizeMode::Height => img.resize(u32::MAX, r.value, f),
    }
}

/// Encode `img` to bytes for `format` (mirrors the original `export_processing`).
fn encode_image_to_bytes(
    img: &DynamicImage,
    format: ExportFormat,
    quality: u8,
) -> Result<Vec<u8>, String> {
    use image::GenericImageView;
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    match format {
        ExportFormat::Jpeg => {
            let rgb = img.to_rgb8();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
            rgb.write_with_encoder(enc).map_err(|e| e.to_string())?;
        }
        // 16-bit PNG/TIFF, like the original.
        ExportFormat::Png => DynamicImage::ImageRgb16(img.to_rgb16())
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?,
        ExportFormat::Tiff => DynamicImage::ImageRgb16(img.to_rgb16())
            .write_to(&mut cursor, image::ImageFormat::Tiff)
            .map_err(|e| e.to_string())?,
        ExportFormat::Avif => img
            .write_to(&mut cursor, image::ImageFormat::Avif)
            .map_err(|e| e.to_string())?,
        ExportFormat::Webp => {
            let enc = webp::Encoder::from_image(img).map_err(|e| e.to_string())?;
            return Ok(enc.encode(quality as f32).to_vec());
        }
        ExportFormat::Jxl => {
            use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};
            let (w, h) = img.dimensions();
            let alpha = img.color().has_alpha();
            let data = if quality >= 100 {
                if alpha {
                    LosslessConfig::new().encode(img.to_rgba8().as_raw(), w, h, PixelLayout::Rgba8)
                } else {
                    LosslessConfig::new().encode(img.to_rgb8().as_raw(), w, h, PixelLayout::Rgb8)
                }
            } else {
                let distance = ((100.0 - quality as f32) / 10.0).max(0.01);
                if alpha {
                    LossyConfig::new(distance).encode(img.to_rgba8().as_raw(), w, h, PixelLayout::Rgba8)
                } else {
                    LossyConfig::new(distance).encode(img.to_rgb8().as_raw(), w, h, PixelLayout::Rgb8)
                }
            };
            return data.map_err(|e| e.to_string());
        }
        ExportFormat::CubeLut => return Err("CUBE LUT uses the LUT export path".into()),
    }
    Ok(buf)
}

/// Bake the current look into a 33-point .cube LUT: process the identity HALD
/// through the engine, then convert it back to a cube file.
fn export_lut(
    ctx: &rapidraw_core::image_processing::GpuContext,
    adj: &rapidraw_core::image_processing::AllAdjustments,
    lut: Option<Arc<Lut>>,
    path: &std::path::Path,
) -> Result<(), String> {
    const SIZE: u32 = 33;
    let identity = rapidraw_core::lut_processing::generate_identity_lut_image(SIZE);
    let processed = rapidraw_core::render(ctx, &identity, adj, &[], lut, None, None)?;
    let cube = rapidraw_core::lut_processing::convert_image_to_cube_lut(&processed, SIZE)?;
    std::fs::write(path, cube).map_err(|e| e.to_string())
}

fn main() {
    env_logger::init();

    let ctx = rapidraw_core::headless_context().expect("gpu init");
    let engine = Engine { ctx: Arc::new(ctx) };
    log::info!("GPU context initialized");

    let app = RelmApp::new("com.rapidraw.relm4");
    app.run::<AppModel>(engine);
}
