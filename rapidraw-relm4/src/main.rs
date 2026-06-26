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
mod info;
mod inpaint;
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
use info::InfoPanel;
use inpaint::InpaintPanel;
use masks::MasksPanel;
use curves::Channel;
use editor::EditorCanvas;
use rapidraw_core::image_processing::{GlobalAdjustments, Point};
use rapidraw_core::mask_generation::{AiPatchDefinition, MaskDefinition, SubMask};
use rapidraw_core::lut_processing::{parse_lut_file, Lut};
use scopes::Scopes;
use settings::Settings;
use state::{Engine, Session};
use thumb::{Thumb, ThumbMsg, ThumbOut};

/// Debounce window (ms) for coalescing rapid slider drags into one render.
/// Small: the render thread also coalesces, and the cached GpuProcessor makes
/// each render cheap, so a short debounce keeps the preview responsive.
const RENDER_DEBOUNCE_MS: u64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResizeMode {
    LongEdge,
    Width,
    Height,
}

/// Resize on export: target `value` px for `mode`, optionally never upscaling.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Resize {
    pub mode: ResizeMode,
    pub value: u32,
    pub dont_enlarge: bool,
}

/// Output options for export (the last-used set is persisted in `Settings`).
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct ExportOpts {
    pub format: ExportFormat,
    /// JPEG quality 1..=100 (ignored for PNG/TIFF).
    pub quality: u8,
    pub resize: Option<Resize>,
}

impl Default for ExportOpts {
    fn default() -> Self {
        Self { format: ExportFormat::Jpeg, quality: 90, resize: None }
    }
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
    /// Remove a root folder from the sidebar (un-list only; files untouched).
    RemoveRoot(PathBuf),
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
    /// Compute an auto tone curve matching the camera's embedded preview and
    /// apply it to the Luma curve (the "Auto" button in the Curves section).
    AutoToneCurve,
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
    /// Reset every edit (adjustments, curves, masks, LUT, crop) to defaults.
    ResetAll,
    /// Right-rail switcher: show the adjustments panel / the crop panel.
    ShowAdjustPanel,
    ShowCropPanel,
    ShowMasksPanel,
    /// Show the AI inpaint (generative replace) panel.
    ShowInpaintPanel,
    /// Inpaint panel actions. Patches reuse the sub-mask messages below
    /// (AddSubMask/DeleteSubMask/…); routing to a patch vs a mask is decided by
    /// the `edit_patch` flag set when a patch is selected.
    /// Create a new patch seeded with the given region tool (one of the
    /// "Create New Generative Edit" grid cards).
    AddPatch(&'static str),
    SelectPatch(Option<usize>),
    DeletePatch(usize),
    TogglePatchVisible(usize),
    SetPatchPrompt(usize, String),
    /// Panel-level toggle: fast local erase vs prompt-driven connector.
    SetInpaintFast(bool),
    /// Run the inpaint engine for patch `patch` and store the result.
    GenerateInpaint { patch: usize },
    /// Show the Info (metadata) panel.
    ShowInfoPanel,
    /// Set one editable author field ("title"/"artist"/"copyright"/"comment").
    SetMetaField(&'static str, String),
    /// Add/remove a user tag on the open image.
    AddMetaTag(String),
    RemoveMetaTag(String),
    /// Set (or clear, with None) the colour label.
    SetColorLabel(Option<String>),
    /// Masks panel actions.
    AddMask(&'static str),
    ResetAllMasks,
    CopyMask(usize),
    PasteMask,
    DuplicateMask(usize),
    DuplicateMaskInvert(usize),
    RenameMask(usize, String),
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
/// Monotonic counter for unique patch ids within a session.
fn next_patch_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

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

/// Bake AI inpaint patches onto the (geometry-applied) `base` before the engine
/// runs, mirroring the Tauri path. Patches are generated in this same
/// render-space (geometry applied), so they align 1:1. Falls back to the
/// un-composited base on error.
// ponytail: re-cropping after generating a patch can misalign it (mask was
// rasterized against the prior crop); regenerate the patch if that matters.
fn composite_patches(base: DynamicImage, patches: &[AiPatchDefinition]) -> DynamicImage {
    if patches.is_empty() {
        return base;
    }
    let adj = serde_json::json!({ "aiPatches": patches });
    match rapidraw_core::image_loader::composite_patches_on_image(
        &base,
        &adj,
        Some(&rapidraw_core::ai::ai_sub_mask_resolver),
    ) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("patch compositing failed: {e}");
            base
        }
    }
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
    ai_patches: Vec<AiPatchDefinition>,
}

/// Work sent to the persistent render thread. Keeping a single long-lived
/// thread lets the GpuProcessor (and its compiled shader) be reused across
/// renders instead of rebuilt per frame.
enum RenderJob {
    Preview {
        base: Arc<DynamicImage>,
        adj: Box<rapidraw_core::image_processing::AllAdjustments>,
        masks: Vec<MaskDefinition>,
        patches: Vec<AiPatchDefinition>,
        lut: Option<Arc<Lut>>,
        dim: u32,
        geom: Geometry,
    },
    Export {
        base: Arc<DynamicImage>,
        adj: Box<rapidraw_core::image_processing::AllAdjustments>,
        masks: Vec<MaskDefinition>,
        patches: Vec<AiPatchDefinition>,
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
    /// Compute the auto tone curve: render `base` neutrally (the engine's
    /// display output, sRGB) as the match source, then histogram-match it to the
    /// RAW's embedded camera preview. Runs on the render thread because it needs
    /// the GPU pipeline (the source must be the real pre-curve output, not a raw
    /// linear develop, or the match is in the wrong domain).
    AutoCurve {
        base: Arc<DynamicImage>,
        path: PathBuf,
        geom: Geometry,
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
    /// A worker computed an auto tone curve (luma control points, 0..255).
    /// Empty if the RAW has no embedded preview.
    AutoCurveReady(Vec<(f32, f32)>),
    /// A worker finished a preview render. Carries the RGBA pixels (the gdk
    /// texture is built on the main thread).
    RenderReady(RgbaImage),
    /// A worker finished a full-res export: Ok(path) or Err(message).
    ExportDone(Result<PathBuf, String>),
    /// An AI inference job finished for container `mask`'s sub `sub`: Ok(base64
    /// PNG) or Err. `patch` routes the result to a patch (vs a mask) — captured
    /// at request time so a container switch mid-inference can't misroute it.
    AiMaskReady {
        mask: usize,
        sub: usize,
        patch: bool,
        result: Result<String, String>,
    },
    /// An inpaint job finished for patch `patch`: Ok(PatchData) or Err.
    InpaintReady {
        patch: usize,
        result: Result<rapidraw_core::mask_generation::PatchData, String>,
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
    /// Root folders shown in the sidebar (persisted across sessions).
    roots: Vec<PathBuf>,
    /// Crop/geometry transforms applied before the GPU render.
    geom: Geometry,
    /// Crop panel (right-rail "Crop" section).
    crop: crop::CropPanel,
    /// Masks panel (right-rail "Masks" section).
    masks_panel: MasksPanel,
    /// Index of the mask whose adjustments are shown in the masks panel.
    selected_mask: Option<usize>,
    /// AI inpaint panel (right-rail "AI" section).
    inpaint_panel: InpaintPanel,
    /// Index of the selected patch in the inpaint panel.
    selected_patch: Option<usize>,
    /// When `Some(i)`, the shared sub-mask tools (brush/pick/AI/add/delete) edit
    /// patch `i` instead of the selected mask. Set on patch select, cleared when
    /// a mask panel/selection takes over.
    edit_patch: Option<usize>,
    /// Clipboard for copy/paste of a whole mask container (not persisted).
    copied_mask: Option<MaskDefinition>,
    /// Inpaint panel: fast local erase (true) vs prompt-driven connector.
    inpaint_fast: bool,
    /// Info (metadata) panel.
    info_panel: InfoPanel,
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

    /// The sub_masks the shared sub-mask tools currently edit: a patch's when
    /// the inpaint panel has armed one (`edit_patch`), else the given mask's.
    fn container_subs_mut(&mut self, idx: usize) -> Option<&mut Vec<SubMask>> {
        if self.edit_patch.is_some() {
            self.session.ai_patches.get_mut(idx).map(|p| &mut p.sub_masks)
        } else {
            self.session.masks.get_mut(idx).map(|m| &mut m.sub_masks)
        }
    }

    /// Rebuild whichever right-rail list owns the active container.
    fn rebuild_active(&self, sender: &ComponentSender<AppModel>) {
        if self.edit_patch.is_some() {
            self.inpaint_panel.rebuild(
                &self.session.ai_patches,
                self.selected_patch,
                self.inpaint_fast,
                sender,
            );
        } else {
            self.masks_panel
                .rebuild(&self.session.masks, self.selected_mask, sender);
        }
    }

    /// Container index the canvas-armed tools (brush/pick/AI) act on: the armed
    /// patch, else the selected mask.
    fn active_container(&self) -> Option<usize> {
        self.edit_patch.or(self.selected_mask)
    }

    /// Rebuild the Info panel from the open image's EXIF + sidecar metadata.
    fn refresh_info_panel(&self, sender: &ComponentSender<AppModel>) {
        let Some(path) = self.session.active_path.as_ref() else {
            self.info_panel.rebuild(None, sender);
            return;
        };
        let exif = meta::read_full_exif(path);
        let (w, h) = self
            .session
            .base_image
            .as_ref()
            .map(|b| {
                use image::GenericImageView;
                b.dimensions()
            })
            .unwrap_or((0, 0));
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_uppercase())
            .unwrap_or_else(|| "FILE".into());
        let rating = self.ratings.get(path).copied().unwrap_or(0);
        let data = info::InfoData {
            file_name,
            extension,
            width: w,
            height: h,
            exif: &exif,
            meta: &self.session.meta,
            rating,
        };
        self.info_panel.rebuild(Some(&data), sender);
    }

    /// True when the Info tab is the visible right-rail panel.
    fn info_visible(&self) -> bool {
        self.content_stack.visible_child_name().as_deref() == Some("info")
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
            ai_patches: self.session.ai_patches.clone(),
            meta: self.session.meta.clone(),
        };
        // Build the snapshot synchronously (captures current state), but write to
        // disk off the main thread: this runs on every photo-open / library-return
        // / history step, and a synchronous write hitched those transitions.
        // ponytail: detached thread, last-writer-wins. Human-paced switching won't
        // race meaningfully; add a per-path serial queue only if it ever does.
        std::thread::spawn(move || sidecar::save(&path, &e));
    }

    /// Compute the auto tone curve (camera-look match) for the current RAW and
    /// deliver it via `CmdMsg::AutoCurveReady`. Dispatched to the render thread
    /// (it needs the GPU pipeline to produce the neutral source). No-op without
    /// an open image.
    fn spawn_auto_curve(&self) {
        let (Some(path), Some(base)) =
            (self.session.active_path.clone(), self.session.base_image.clone())
        else {
            return;
        };
        let _ = self.render_tx.send(RenderJob::AutoCurve {
            base,
            path,
            geom: self.geom,
        });
    }

    /// Reset every edit (adjustments, curves, masks, LUT, crop/geometry) to
    /// defaults, in place (no panel rebuild). Shared by opening a new image and
    /// the Reset button. Does not render or record history — the caller does.
    fn reset_edits(&mut self, sender: &ComponentSender<AppModel>) {
        self.geom = Geometry::default();
        self.crop_aspect = 0.0;
        self.crop_active = false;
        self.canvas.reset_crop();
        self.session.adjustments = Default::default();
        controls::init_defaults(&mut self.session.adjustments.global);
        self.session.lut = None;
        self.lut_path = None;
        self.session.masks.clear();
        self.session.ai_patches.clear();
        self.selected_mask = None;
        self.selected_patch = None;
        self.edit_patch = None;
        self.masks_panel.rebuild(&self.session.masks, None, sender);
        self.inpaint_panel
            .rebuild(&self.session.ai_patches, None, self.inpaint_fast, sender);
        self.panel.reset();
        // Crop panel is small; rebuild so its toggles/straighten reset. geom was
        // just set to default above, so this seeds it to defaults.
        let fresh = crop::CropPanel::new(sender, self.geom);
        self.content_stack.remove(self.crop.root());
        self.content_stack.add_named(fresh.root(), Some("crop"));
        self.crop = fresh;
        self.content_stack.set_visible_child_name("adjust");
        // Reset the tab switcher to Edit.
        self.edit_btn.set_active(true);
    }

    /// Push the selected mask's drawable shapes to the canvas overlay (only while
    /// the Masks tab is active); hides it otherwise.
    fn refresh_mask_overlay(&self) {
        let tab = self.content_stack.visible_child_name();
        let on = matches!(tab.as_deref(), Some("masks") | Some("inpaint"));
        let shapes = match (on, self.active_container(), self.session.base_image.as_ref()) {
            (true, Some(i), Some(base)) => {
                use image::GenericImageView;
                let (w, h) = base.dimensions();
                let (w, h) = (w as f64, h as f64);
                // active_container indexes patches when editing one, else masks.
                if self.edit_patch.is_some() {
                    self.session
                        .ai_patches
                        .get(i)
                        .map(|p| masks::overlay_shapes(&p.sub_masks, w, h))
                        .unwrap_or_default()
                } else {
                    self.session
                        .masks
                        .get(i)
                        .map(|m| masks::overlay_shapes(&m.sub_masks, w, h))
                        .unwrap_or_default()
                }
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
        self.session.ai_patches = entry.ai_patches;
        self.selected_patch = self
            .selected_patch
            .filter(|&i| i < self.session.ai_patches.len());
        self.edit_patch = self.edit_patch.filter(|&i| i < self.session.ai_patches.len());
        self.inpaint_panel.rebuild(
            &self.session.ai_patches,
            self.selected_patch,
            self.inpaint_fast,
            sender,
        );
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
                        patches,
                        lut,
                        path,
                        opts,
                        geom,
                    } => {
                        let base = apply_geometry(&base, geom);
                        let base = composite_patches(base, &patches);
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
                    RenderJob::AutoCurve { base, path, geom } => {
                        // Source = the engine's neutral display output (sRGB), so
                        // the match is in the same space the luma curve applies.
                        let base = apply_geometry(&base, geom);
                        // Match the app's neutral (init_defaults seeds a few
                        // non-zero defaults) so the source equals the real
                        // pre-curve preview, not a bare zeroed struct.
                        let mut neutral =
                            rapidraw_core::image_processing::AllAdjustments::default();
                        controls::init_defaults(&mut neutral.global);
                        let pts = rapidraw_core::render(
                            &ctx,
                            &base,
                            &neutral,
                            &[],
                            None,
                            Some(1024),
                            Some(&rapidraw_core::ai::ai_sub_mask_resolver),
                        )
                        .ok()
                        .zip(rapidraw_core::embedded_preview(&path))
                        .map(|(src, target)| rapidraw_core::auto_tone_curve(&src, &target))
                        .unwrap_or_default();
                        let _ = cmd.send(CmdMsg::AutoCurveReady(pts));
                    }
                }
            }
            if let Some(RenderJob::Preview {
                base,
                adj,
                masks,
                patches,
                lut,
                dim,
                geom,
            }) = latest_preview
            {
                let base = apply_geometry(&base, geom);
                let base = composite_patches(base, &patches);
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
                    set_shrink_end_child: false,
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
                            #[name = "header_lib"]
                            add_top_bar = &adw::HeaderBar {
                                // macOS controls live on the left sidebar header;
                                // here only the right-side controls (Linux close).
                                set_show_start_title_buttons: false,
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
                            #[name = "header_ed"]
                            add_top_bar = &adw::HeaderBar {
                                // macOS controls live on the left sidebar header;
                                // here only the right-side controls (Linux close).
                                set_show_start_title_buttons: false,
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
                                        set_icon_name: "edit-clear-all-symbolic",
                                        set_tooltip_text: Some("Reset all adjustments"),
                                        connect_clicked => AppMsg::ResetAll,
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
        // Register the bundled relm4-icons gresource and add it to the icon theme
        // (needs a live display, so it runs here, not in main()). Without this the
        // tab/card icon names fall back to the broken-image placeholder.
        relm4_icons::initialize_icons();

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
                SidebarOut::RemoveRootFolder(p) => AppMsg::RemoveRoot(p),
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

        let loaded = load_settings();
        // Copy out the fields needed elsewhere in the literal before `loaded` is
        // moved into `settings` (Settings is no longer Copy).
        let (loaded_raw_filter, loaded_sort_by) = (loaded.raw_filter, loaded.sort_by);
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
            settings: loaded,
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
            raw_filter: loaded_raw_filter,
            sort_by: loaded_sort_by,
            search: String::new(),
            last_folder: load_last_folder(),
            roots: load_roots(),
            geom: Geometry::default(),
            crop: crop::CropPanel::new(&sender, Geometry::default()),
            masks_panel: MasksPanel::new(&sender),
            selected_mask: None,
            inpaint_panel: InpaintPanel::new(&sender),
            selected_patch: None,
            edit_patch: None,
            copied_mask: None,
            inpaint_fast: true,
            info_panel: InfoPanel::new(),
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

        // Reflect the persisted filter/sort in the menu radios, and apply the
        // persisted editor background to the canvas (load only set the field).
        let filter_key = match model.raw_filter {
            library::RawFilter::RawOnly => "raw",
            library::RawFilter::NonRawOnly => "nonraw",
            library::RawFilter::PreferRaw => "prefer",
            library::RawFilter::All => "all",
        };
        let sort_key = match model.sort_by {
            library::SortBy::DateNewest => "new",
            library::SortBy::DateOldest => "old",
            library::SortBy::RatingDesc => "rating",
            library::SortBy::Name => "name",
        };
        act_filter.set_state(&gtk::glib::Variant::from(filter_key));
        act_sort.set_state(&gtk::glib::Variant::from(sort_key));
        model.canvas.set_background(model.settings.background);

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
        model
            .content_stack
            .add_named(model.inpaint_panel.root(), Some("inpaint"));
        model
            .content_stack
            .add_named(model.info_panel.root(), Some("info"));
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
        // Icon tabs (relm4-icons line glyphs), matching the Tauri rail. Tooltips
        // carry the names since the buttons are icon-only.
        let adj_btn = gtk::ToggleButton::new();
        adj_btn.set_icon_name("options-regular");
        adj_btn.set_tooltip_text(Some("Adjustments"));
        adj_btn.set_active(true);
        let crop_btn = gtk::ToggleButton::new();
        crop_btn.set_icon_name("crop-regular");
        crop_btn.set_tooltip_text(Some("Crop & geometry"));
        crop_btn.set_group(Some(&adj_btn));
        let masks_btn = gtk::ToggleButton::new();
        masks_btn.set_icon_name("layer-diagonal-regular");
        masks_btn.set_tooltip_text(Some("Masks"));
        masks_btn.set_group(Some(&adj_btn));
        let ai_btn = gtk::ToggleButton::new();
        ai_btn.set_icon_name("paint-brush-regular");
        ai_btn.set_tooltip_text(Some("Inpaint (generative replace)"));
        ai_btn.set_group(Some(&adj_btn));
        let info_btn = gtk::ToggleButton::new();
        info_btn.set_icon_name("info-regular");
        info_btn.set_tooltip_text(Some("Photo info & metadata"));
        info_btn.set_group(Some(&adj_btn));
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
        {
            let sender = sender.clone();
            ai_btn.connect_toggled(move |b| {
                if b.is_active() {
                    sender.input(AppMsg::ShowInpaintPanel);
                }
            });
        }
        {
            let sender = sender.clone();
            info_btn.connect_toggled(move |b| {
                if b.is_active() {
                    sender.input(AppMsg::ShowInfoPanel);
                }
            });
        }
        tabs.append(&adj_btn);
        tabs.append(&crop_btn);
        tabs.append(&masks_btn);
        tabs.append(&ai_btn);
        tabs.append(&info_btn);
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
        // macOS: native traffic lights sit at the window's top-left. They normally
        // live over the sidebar header; when the sidebar collapses, the content
        // header slides under them and they cover the toggle/Open Folder buttons.
        // Reserve the controls inset on the content headers while collapsed so the
        // buttons shift clear. No-op on Linux (controls live on the right).
        let header_lib = widgets.header_lib.clone();
        let header_ed = widgets.header_ed.clone();
        let reserve_inset = move |sidebar_visible: bool| {
            if cfg!(target_os = "macos") {
                header_lib.set_show_start_title_buttons(!sidebar_visible);
                header_ed.set_show_start_title_buttons(!sidebar_visible);
            }
        };
        {
            let sb = model.sidebar.widget().clone();
            let other = widgets.sidebar_toggle_ed.clone();
            let reserve_inset = reserve_inset.clone();
            widgets.sidebar_toggle_lib.connect_toggled(move |b| {
                sb.set_visible(b.is_active());
                reserve_inset(b.is_active());
                if other.is_active() != b.is_active() {
                    other.set_active(b.is_active());
                }
            });
        }
        {
            let sb = model.sidebar.widget().clone();
            let other = widgets.sidebar_toggle_lib.clone();
            let reserve_inset = reserve_inset.clone();
            widgets.sidebar_toggle_ed.connect_toggled(move |b| {
                sb.set_visible(b.is_active());
                reserve_inset(b.is_active());
                if other.is_active() != b.is_active() {
                    other.set_active(b.is_active());
                }
            });
        }
        widgets.split.set_start_child(Some(model.sidebar.widget()));
        // Hidden until a folder is opened (no sidebar on the welcome/splash screen).
        model.sidebar.widget().set_visible(false);
        reserve_inset(false); // sidebar starts hidden; reserve the macOS controls inset
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
                self.session.current_folder = Some(path.clone());
                if !self.roots.contains(&path) {
                    self.roots.push(path);
                    save_roots(&self.roots);
                }
                self.sidebar.emit(SidebarIn::SetRoots(self.roots.clone()));
                widgets.lib_stack.set_visible_child_name("grid");
                // Reveal the sidebar now that there's a folder to navigate.
                self.sidebar.widget().set_visible(true);
                widgets.sidebar_toggle_lib.set_active(true);
                widgets.sidebar_toggle_ed.set_active(true);
                self.apply_library(&sender);
            }
            AppMsg::RemoveRoot(path) => {
                self.roots.retain(|r| r != &path);
                save_roots(&self.roots);
                self.sidebar.emit(SidebarIn::SetRoots(self.roots.clone()));
            }
            AppMsg::ContinueSession => {
                // Re-show every previously added root, not just the last one. Opening the
                // folder for the grid (FolderChosen) re-emits the full root list.
                let target = self.last_folder.clone().or_else(|| self.roots.first().cloned());
                if let Some(p) = target {
                    sender.input(AppMsg::FolderChosen(p));
                }
            }
            AppMsg::ShowFolder(dir) => {
                self.all_images = library::scan_dir(&dir);
                self.apply_library(&sender);
                widgets.lib_stack.set_visible_child_name("grid");
                // If we're on the editor page (folder clicked mid-edit), return
                // to the library so the new folder's thumbnails are visible.
                widgets.nav.pop_to_tag("library");
            }
            AppMsg::FilterChanged(f) => {
                self.raw_filter = f;
                self.settings.raw_filter = f;
                save_settings(&self.settings);
                self.apply_library(&sender);
            }
            AppMsg::SortChanged(s) => {
                self.sort_by = s;
                self.settings.sort_by = s;
                save_settings(&self.settings);
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
                let fresh = crop::CropPanel::new(&sender, self.geom);
                self.content_stack.remove(self.crop.root());
                self.content_stack.add_named(fresh.root(), Some("crop"));
                self.crop = fresh;
                if self.crop_active {
                    self.content_stack.set_visible_child_name("crop");
                }
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::ResetAll => {
                self.reset_edits(&sender);
                self.schedule_history(&sender);
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::AutoToneCurve => {
                self.spawn_auto_curve();
            }
            AppMsg::ShowAdjustPanel => {
                self.content_stack.set_visible_child_name("adjust");
                self.edit_patch = None;
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
                self.edit_patch = None;
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
                // Masks mode takes over the shared sub-mask tools from patches.
                self.edit_patch = None;
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
            AppMsg::ShowInpaintPanel => {
                self.content_stack.set_visible_child_name("inpaint");
                // Scopes show only on Adjust/Masks (matching the Tauri UI).
                self.scopes.root().set_visible(false);
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
                // Re-arm patch editing for the selected patch (if any).
                self.edit_patch = self.selected_patch;
                self.inpaint_panel.rebuild(
                    &self.session.ai_patches,
                    self.selected_patch,
                    self.inpaint_fast,
                    &sender,
                );
                self.refresh_mask_overlay();
            }
            AppMsg::AddPatch(region_ty) => {
                let n = self.session.ai_patches.len() + 1;
                let label = inpaint::tool_label(region_ty);
                self.session.ai_patches.push(AiPatchDefinition {
                    id: format!("patch-{}", next_patch_id()),
                    name: format!("{label} {n}"),
                    visible: true,
                    invert: false,
                    prompt: String::new(),
                    patch_data: None,
                    opacity: 100.0,
                    sub_masks: Vec::new(),
                });
                let i = self.session.ai_patches.len() - 1;
                self.selected_patch = Some(i);
                self.edit_patch = Some(i);
                // Quick Erase defaults to local fast erase; the rest to the
                // prompt-driven connector (toggle on the patch overrides).
                self.inpaint_fast = region_ty == "quick-eraser";
                self.inpaint_panel.rebuild(
                    &self.session.ai_patches,
                    self.selected_patch,
                    self.inpaint_fast,
                    &sender,
                );
                self.schedule_history(&sender);
                // Seed the chosen region (auto-arms its tool via AddSubMask).
                sender.input(AppMsg::AddSubMask(i, region_ty));
            }
            AppMsg::SelectPatch(idx) => {
                self.selected_patch = idx;
                self.edit_patch = idx;
                // Disarm canvas tools (they targeted the prior container).
                if self.paint_sub.take().is_some() {
                    self.canvas.set_paint(None);
                }
                self.canvas.set_pick(None);
                self.canvas.set_mask_draw(None);
                self.inpaint_panel.rebuild(
                    &self.session.ai_patches,
                    self.selected_patch,
                    self.inpaint_fast,
                    &sender,
                );
                self.refresh_mask_overlay();
            }
            AppMsg::DeletePatch(i) => {
                if i < self.session.ai_patches.len() {
                    self.session.ai_patches.remove(i);
                    self.selected_patch = match self.selected_patch {
                        Some(s) if s == i => None,
                        Some(s) if s > i => Some(s - 1),
                        other => other,
                    };
                    self.edit_patch = self.selected_patch;
                    self.inpaint_panel.rebuild(
                        &self.session.ai_patches,
                        self.selected_patch,
                        self.inpaint_fast,
                        &sender,
                    );
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::TogglePatchVisible(i) => {
                if let Some(p) = self.session.ai_patches.get_mut(i) {
                    p.visible = !p.visible;
                    self.inpaint_panel.rebuild(
                        &self.session.ai_patches,
                        self.selected_patch,
                        self.inpaint_fast,
                        &sender,
                    );
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::SetPatchPrompt(i, text) => {
                if let Some(p) = self.session.ai_patches.get_mut(i) {
                    p.prompt = text;
                    self.schedule_history(&sender);
                }
            }
            AppMsg::SetInpaintFast(on) => {
                self.inpaint_fast = on;
                // Rebuild so the prompt row enables/disables.
                self.inpaint_panel.rebuild(
                    &self.session.ai_patches,
                    self.selected_patch,
                    self.inpaint_fast,
                    &sender,
                );
            }
            AppMsg::GenerateInpaint { patch } => {
                let Some(base) = self.session.base_image.clone() else { return };
                let Some(p) = self.session.ai_patches.get(patch).cloned() else { return };
                if p.sub_masks.is_empty() {
                    self.toasts
                        .add_toast(adw::Toast::new("Add a region to the patch first"));
                    return;
                }
                // Source = the image with the OTHER patches baked in (so this
                // patch inpaints over the current look), geometry applied.
                let others: Vec<AiPatchDefinition> = self
                    .session
                    .ai_patches
                    .iter()
                    .filter(|q| q.id != p.id)
                    .cloned()
                    .collect();
                let mask_def = MaskDefinition {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    visible: true,
                    invert: p.invert,
                    opacity: 100.0,
                    adjustments: serde_json::Value::Null,
                    sub_masks: p.sub_masks.clone(),
                };
                let geom = self.geom;
                let fast = self.inpaint_fast;
                let prompt = p.prompt.clone();
                let connector = self.settings.ai_connector_address.clone();
                let source_path = self
                    .session
                    .active_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let backend = if fast {
                    Some(ai_masks::InpaintBackend::FastErase)
                } else {
                    connector
                        .filter(|a| !a.is_empty())
                        .map(|address| ai_masks::InpaintBackend::Connector {
                            base_url: format!("http://{address}"),
                            source_path,
                            prompt,
                        })
                };
                let Some(backend) = backend else {
                    self.toasts.add_toast(adw::Toast::new(
                        "Set the AI connector address in Settings, or enable Fast erase",
                    ));
                    return;
                };
                self.win_title.set_subtitle("Generating inpaint…");
                spawn_bg(&sender, move || {
                    let img = apply_geometry(&base, geom);
                    let src = composite_patches(img.clone(), &others);
                    let (w, h) = {
                        use image::GenericImageView;
                        img.dimensions()
                    };
                    let result = rapidraw_core::mask_generation::generate_mask_bitmap(
                        &mask_def,
                        w,
                        h,
                        1.0,
                        (0.0, 0.0),
                        None,
                        Some(&rapidraw_core::ai::ai_sub_mask_resolver),
                    )
                    .ok_or_else(|| "patch mask is empty (draw or auto-mask a region)".to_string())
                    .and_then(|mask| ai_masks::run_inpaint(&src, &mask, backend));
                    CmdMsg::InpaintReady { patch, result }
                });
            }
            AppMsg::ShowInfoPanel => {
                self.content_stack.set_visible_child_name("info");
                self.edit_patch = None;
                // Scopes show only on Adjust/Masks (matching the Tauri UI).
                self.scopes.root().set_visible(false);
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
                self.refresh_info_panel(&sender);
                self.refresh_mask_preview(&sender); // clears (Masks tab off)
            }
            AppMsg::SetMetaField(field, value) => {
                let v = {
                    let t = value.trim();
                    (!t.is_empty()).then(|| t.to_string())
                };
                match field {
                    "title" => self.session.meta.title = v,
                    "artist" => self.session.meta.artist = v,
                    "copyright" => self.session.meta.copyright = v,
                    "comment" => self.session.meta.comment = v,
                    _ => {}
                }
                self.save_edits();
            }
            AppMsg::AddMetaTag(tag) => {
                let t = tag.trim().to_lowercase();
                if !t.is_empty() && !self.session.meta.tags.contains(&t) {
                    self.session.meta.tags.push(t);
                    self.session.meta.tags.sort();
                    self.save_edits();
                    self.refresh_info_panel(&sender);
                }
            }
            AppMsg::RemoveMetaTag(tag) => {
                self.session.meta.tags.retain(|x| x != &tag);
                self.save_edits();
                self.refresh_info_panel(&sender);
            }
            AppMsg::SetColorLabel(c) => {
                self.session.meta.color = c;
                self.save_edits();
                self.refresh_info_panel(&sender);
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
                // Brush/flow: auto-arm painting so the first drag paints.
                if matches!(ty, "radial" | "linear") {
                    self.canvas.set_mask_draw(Some((0, ty == "radial")));
                } else if matches!(ty, "brush" | "flow") {
                    sender.input(AppMsg::ArmPaint(Some(0)));
                }
                self.schedule_history(&sender);
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::ResetAllMasks => {
                self.session.masks.clear();
                self.selected_mask = None;
                self.edit_patch = None;
                self.canvas.set_mask_draw(None);
                self.masks_panel.rebuild(&self.session.masks, None, &sender);
                self.refresh_mask_preview(&sender);
                self.schedule_history(&sender);
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::CopyMask(i) => {
                self.copied_mask = self.session.masks.get(i).cloned();
                self.masks_panel
                    .rebuild(&self.session.masks, self.selected_mask, &sender);
            }
            AppMsg::PasteMask => {
                if let Some(src) = self.copied_mask.clone() {
                    self.session.masks.push(masks::clone_mask(&src, false));
                    self.selected_mask = Some(self.session.masks.len() - 1);
                    self.masks_panel
                        .rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.refresh_mask_preview(&sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::DuplicateMask(i) => {
                if let Some(src) = self.session.masks.get(i).cloned() {
                    self.session.masks.insert(i + 1, masks::clone_mask(&src, false));
                    self.selected_mask = Some(i + 1);
                    self.masks_panel
                        .rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.refresh_mask_preview(&sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::DuplicateMaskInvert(i) => {
                if let Some(src) = self.session.masks.get(i).cloned() {
                    self.session.masks.insert(i + 1, masks::clone_mask(&src, true));
                    self.selected_mask = Some(i + 1);
                    self.masks_panel
                        .rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.refresh_mask_preview(&sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::RenameMask(i, name) => {
                if let Some(m) = self.session.masks.get_mut(i) {
                    m.name = name;
                    self.masks_panel
                        .rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.schedule_history(&sender);
                }
            }
            AppMsg::SelectMask(idx) => {
                self.selected_mask = idx;
                self.edit_patch = None;
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
                if let Some(sm) = self.container_subs_mut(mask).and_then(|s| s.get_mut(sub)) {
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
                if let Some(sm) = self.container_subs_mut(mask).and_then(|s| s.get_mut(sub)) {
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
                let c = self.active_container().unwrap_or(usize::MAX);
                if let Some(sm) = self.container_subs_mut(c).and_then(|s| s.get_mut(sub)) {
                    if let Some(obj) = sm.parameters.as_object_mut() {
                        for (k, v) in keys {
                            obj.insert(k.to_string(), serde_json::json!(v));
                        }
                    }
                    // ponytail: spin rows stay stale until the panel rebuilds
                    // (reselect); the overlay updates live. Rebuild on every drag
                    // tick would thrash the panel.
                    self.schedule_history(&sender);
                    self.refresh_mask_overlay();
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
                let (bsize, bfeather) = (self.brush_size, self.brush_feather);
                let Some(c) = self.active_container() else { return };
                if let Some(sm) = self.container_subs_mut(c).and_then(|s| s.get_mut(sub)) {
                    let is_flow = sm.mask_type == "flow";
                    let pts: Vec<serde_json::Value> = points
                        .iter()
                        .map(|(x, y)| serde_json::json!({ "x": x * w, "y": y * h }))
                        .collect();
                    let mut line = serde_json::json!({
                        "tool": if erase { "eraser" } else { "brush" },
                        "brushSize": bsize,
                        "feather": bfeather / 100.0,
                        "points": pts,
                    });
                    if is_flow {
                        // Flow lines carry a per-line flow (default matches core).
                        line["flow"] = serde_json::json!(10.0);
                    }
                    if let Some(obj) = sm.parameters.as_object_mut() {
                        obj.entry("lines").or_insert_with(|| serde_json::json!([]));
                        if let Some(arr) = obj["lines"].as_array_mut() {
                            arr.push(line);
                        }
                    }
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::ClearStrokes(sub) => {
                let Some(c) = self.active_container() else { return };
                if let Some(sm) = self.container_subs_mut(c).and_then(|s| s.get_mut(sub)) {
                    if let Some(obj) = sm.parameters.as_object_mut() {
                        obj.insert("lines".into(), serde_json::json!([]));
                    }
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::GenerateAiMask(sub) => {
                let Some(mi) = self.active_container() else { return };
                let patch = self.edit_patch.is_some();
                let Some(base) = self.session.base_image.clone() else { return };
                let Some(sm) = self.container_subs_mut(mi).and_then(|s| s.get(sub).cloned())
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
                        patch,
                        result,
                    }
                });
            }
            AppMsg::SetMaskOverlayShown(shown) => {
                self.canvas.set_mask_preview_visible(shown);
            }
            AppMsg::ArmPick(sub) => {
                let c = self.active_container().unwrap_or(usize::MAX);
                let arm = sub.and_then(|s| {
                    let ty = self
                        .container_subs_mut(c)
                        .and_then(|subs| subs.get(s))
                        .map(|sm| sm.mask_type.clone());
                    ty.map(|t| (s, matches!(t.as_str(), "ai-subject" | "quick-eraser")))
                });
                self.canvas.set_pick(arm);
            }
            AppMsg::PickResult { sub, x1, y1, x2, y2 } => {
                let Some((w, h)) = self.image_dims() else { return };
                let Some(mi) = self.active_container() else { return };
                let Some(sm) = self.container_subs_mut(mi).and_then(|s| s.get_mut(sub))
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
                let label = masks::MASK_TYPES
                    .iter()
                    .find(|(_, t)| *t == ty)
                    .map(|(l, _)| *l)
                    .unwrap_or(ty);
                // Reuse new_mask's sub-mask seeding, then move it onto this
                // container (keeps default geometry/params in one place).
                let sub = masks::new_mask(label, ty, w, h).sub_masks.remove(0);
                let is_patch = self.edit_patch.is_some();
                if let Some(subs) = self.container_subs_mut(mask) {
                    subs.push(sub);
                    let new_sub = subs.len() - 1;
                    self.rebuild_active(&sender);
                    // Arm "draw to place" for a radial/linear sub-mask (masks and
                    // patches alike — geometry routes via active_container).
                    if matches!(ty, "radial" | "linear") {
                        self.canvas.set_mask_draw(Some((new_sub, ty == "radial")));
                    } else {
                        match ty {
                            // Brush/flow auto-arm painting for masks AND patches, so
                            // a fresh brush mask paints on the first drag (mirrors the
                            // original auto-selecting the brush tool). Without this
                            // the drag just panned and nothing appeared.
                            "brush" | "flow" => sender.input(AppMsg::ArmPaint(Some(new_sub))),
                            // AI auto-mask tools stay one-click only inside a patch.
                            "ai-subject" | "quick-eraser" if is_patch => {
                                sender.input(AppMsg::ArmPick(Some(new_sub)))
                            }
                            "ai-foreground" if is_patch => {
                                sender.input(AppMsg::GenerateAiMask(new_sub))
                            }
                            _ => {}
                        }
                    }
                    self.refresh_mask_overlay();
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::DeleteSubMask { mask, sub } => {
                let removed = self
                    .container_subs_mut(mask)
                    .filter(|s| sub < s.len())
                    .map(|s| {
                        s.remove(sub);
                    })
                    .is_some();
                if removed {
                    self.rebuild_active(&sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::ToggleSubMaskVisible { mask, sub } => {
                let ok = self
                    .container_subs_mut(mask)
                    .and_then(|s| s.get_mut(sub))
                    .map(|sm| sm.visible = !sm.visible)
                    .is_some();
                if ok {
                    self.rebuild_active(&sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::ToggleSubMaskInvert { mask, sub } => {
                let ok = self
                    .container_subs_mut(mask)
                    .and_then(|s| s.get_mut(sub))
                    .map(|sm| sm.invert = !sm.invert)
                    .is_some();
                if ok {
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
                if self.info_visible() {
                    self.refresh_info_panel(&sender);
                }
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
                // Reset all controls to defaults *now* (in place) so the previous
                // photo's state isn't shown while the new image decodes. Saved
                // edits (if any) are applied in BaseReady, after decode.
                self.reset_edits(&sender);
                // Clear the previous image's metadata too (Reset button keeps it,
                // so this lives here, not in reset_edits). Restored from sidecar
                // on load if present.
                self.session.meta = Default::default();
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
                    patches: self.session.ai_patches.clone(),
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

                // Restore the last-used export options (set before the update
                // closures fire so visibility matches the restored format).
                let last = self.settings.last_export;
                fmt.set_selected(match last.format {
                    ExportFormat::Png => 1,
                    ExportFormat::Tiff => 2,
                    ExportFormat::Webp => 3,
                    ExportFormat::Jxl => 4,
                    ExportFormat::Avif => 5,
                    ExportFormat::CubeLut => 6,
                    ExportFormat::Jpeg => 0,
                });
                q.set_value(last.quality as f64);
                if let Some(r) = last.resize {
                    resize_mode.set_selected(match r.mode {
                        ResizeMode::LongEdge => 1,
                        ResizeMode::Width => 2,
                        ResizeMode::Height => 3,
                    });
                    resize.set_value(r.value as f64);
                    dont_enlarge.set_active(r.dont_enlarge);
                }
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
                // Persist as the last-used export options.
                self.settings.last_export = opts;
                save_settings(&self.settings);
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
                    patches: self.session.ai_patches.clone(),
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
                settings::present(root, self.settings.clone(), &sender);
            }
            AppMsg::SettingsChanged(s) => {
                save_settings(&s); // persist so the next launch restores them
                self.canvas.set_background(s.background);
                self.settings = s;
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
                let patches = self.session.ai_patches.clone();
                let patches_json = serde_json::to_value(&patches).unwrap_or_default();
                let same = self
                    .history
                    .get(self.hist_idx)
                    .map(|e| {
                        bytemuck::bytes_of(&e.adj.global) == bytemuck::bytes_of(&cur.global)
                            && lut_eq(&e.lut, &lut)
                            && serde_json::to_value(&e.masks).unwrap_or_default() == masks_json
                            && serde_json::to_value(&e.ai_patches).unwrap_or_default()
                                == patches_json
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
                    ai_patches: patches,
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
        root: &Self::Root,
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
            CmdMsg::AutoCurveReady(pts) => {
                // <2 points = no embedded preview / no tonal range: leave as-is.
                if pts.len() >= 2 {
                    let pts: Vec<(f64, f64)> =
                        pts.iter().map(|&(x, y)| (x as f64, y as f64)).collect();
                    // set_luma_curve emits CurveChanged → engine + history + render.
                    self.panel.set_luma_curve(pts);
                }
            }
            CmdMsg::BaseReady(path, img) => {
                let (w, h) = img.dimensions();
                log::info!("base image ready: {} ({w}x{h})", path.display());
                // Controls were already reset to defaults in OpenInEditor; here we
                // just fill the EXIF subtitle and apply any saved edits.
                self.win_title
                    .set_subtitle(&meta::read_summary(&path).unwrap_or_default());
                let mut applied_sidecar = false;
                if !self.settings.reset_on_open {
                    if let Some(e) = sidecar::load(&path) {
                        applied_sidecar = true;
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
                        self.session.ai_patches = e.ai_patches;
                        self.session.meta = e.meta;
                        self.selected_mask = None;
                        self.masks_panel.rebuild(&self.session.masks, None, &sender);
                        // Reseed the crop panel (built with defaults in
                        // OpenInEditor, before geom was known) so its flip
                        // toggles / straighten reflect the restored geometry.
                        let fresh = crop::CropPanel::new(&sender, self.geom);
                        self.content_stack.remove(self.crop.root());
                        self.content_stack.add_named(fresh.root(), Some("crop"));
                        self.crop = fresh;
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
                    ai_patches: self.session.ai_patches.clone(),
                }];
                self.hist_idx = 0;
                // Refresh the Info panel now the image (dims) + sidecar metadata
                // are loaded, if it's the visible tab.
                if self.info_visible() {
                    self.refresh_info_panel(&sender);
                }
                // RawTherapee-style: on a fresh RAW (no saved edits) set the
                // auto tone curve to match the camera's embedded preview.
                if !applied_sidecar && rapidraw_core::formats::is_raw_file(&path) {
                    self.spawn_auto_curve();
                }
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
                // Recover the gl renderer's stale framebuffer after the export's
                // GPU work (macOS: window goes transparent without this).
                root.queue_draw();
                log::info!("export saved: {}", path.display());
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.toasts
                    .add_toast(adw::Toast::new(&format!("Saved {name}")));
            }
            CmdMsg::ExportDone(Err(e)) => {
                root.queue_draw();
                log::warn!("export failed: {e}");
                self.toasts
                    .add_toast(adw::Toast::new(&format!("Export failed: {e}")));
            }
            CmdMsg::AiMaskReady {
                mask,
                sub,
                patch,
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
                    // Route by the captured `patch` flag (not edit_patch) so a
                    // container switch during inference can't misroute the result.
                    let subs = if patch {
                        self.session.ai_patches.get_mut(mask).map(|p| &mut p.sub_masks)
                    } else {
                        self.session.masks.get_mut(mask).map(|m| &mut m.sub_masks)
                    };
                    if let Some(sm) = subs.and_then(|s| s.get_mut(sub)) {
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
                        if !patch {
                            self.refresh_mask_overlay();
                        }
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
            CmdMsg::InpaintReady { patch, result } => {
                let subtitle = self
                    .session
                    .active_path
                    .as_deref()
                    .and_then(meta::read_summary)
                    .unwrap_or_default();
                self.win_title.set_subtitle(&subtitle);
                match result {
                    Ok(data) => {
                        if let Some(p) = self.session.ai_patches.get_mut(patch) {
                            p.patch_data = Some(data);
                            self.inpaint_panel.rebuild(
                                &self.session.ai_patches,
                                self.selected_patch,
                                self.inpaint_fast,
                                &sender,
                            );
                            self.schedule_history(&sender);
                            sender.input(AppMsg::RequestRender);
                            self.toasts.add_toast(adw::Toast::new("Inpaint generated"));
                        }
                    }
                    Err(e) => {
                        log::warn!("inpaint failed: {e}");
                        self.toasts
                            .add_toast(adw::Toast::new(&format!("Inpaint failed: {e}")));
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
                 font-size: 12px;
                 letter-spacing: 2px;
                 opacity: 0.8;
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

/// File holding the list of root folders shown in the sidebar (across sessions).
fn roots_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rapidraw-relm4").join("roots.json"))
}

fn save_roots(roots: &[PathBuf]) {
    let Some(f) = roots_file() else { return };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let list: Vec<String> = roots.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    if let Ok(json) = serde_json::to_vec(&list) {
        let _ = std::fs::write(f, json);
    }
}

fn load_roots() -> Vec<PathBuf> {
    let Some(f) = roots_file() else { return Vec::new() };
    let Ok(bytes) = std::fs::read(f) else { return Vec::new() };
    let list: Vec<String> = serde_json::from_slice(&bytes).unwrap_or_default();
    list.into_iter().map(PathBuf::from).filter(|p| p.is_dir()).collect()
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

    // Pick the GTK GSK renderer before GTK initialises the display. Per-platform
    // default (macOS GL, Linux Vulkan) unless the user overrode it in Settings;
    // an explicit GSK_RENDERER env var still wins over both. On the gl renderer
    // the macOS framebuffer can go stale after a full-res GPU export (window
    // transparent, regions repaint only on hover); a full-tree queue_draw() on
    // ExportDone recovers it (see update_cmd).
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", load_settings().renderer.gsk_value());
    }

    let ctx = rapidraw_core::headless_context().expect("gpu init");
    let engine = Engine { ctx: Arc::new(ctx) };
    log::info!("GPU context initialized");

    let app = RelmApp::new("com.rapidraw.relm4");
    app.run::<AppModel>(engine);
}
