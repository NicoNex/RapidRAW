use std::cell::RefCell;
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

mod colorwheel;
mod controls;
mod curves;
mod editor;
mod library;
mod scopes;
mod settings;
mod slider;
mod state;
mod thumb;
use controls::AdjustPanel;
use scopes::Scopes;
use curves::Channel;
use editor::EditorCanvas;
use rapidraw_core::image_processing::{GlobalAdjustments, Point};
use rapidraw_core::lut_processing::{parse_lut_file, Lut};
use settings::Settings;
use state::{Engine, Session};
use thumb::{Thumb, ThumbMsg};

/// Debounce window (ms) for coalescing rapid slider drags into one render.
/// Small: the render thread also coalesces, and the cached GpuProcessor makes
/// each render cheap, so a short debounce keeps the preview responsive.
const RENDER_DEBOUNCE_MS: u64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Jpeg,
    Png,
    Tiff,
}

impl ExportFormat {
    fn ext(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Png => "png",
            ExportFormat::Tiff => "tiff",
        }
    }
}

/// Output options for export.
#[derive(Debug, Clone, Copy)]
pub struct ExportOpts {
    pub format: ExportFormat,
    /// JPEG quality 1..=100 (ignored for PNG/TIFF).
    pub quality: u8,
    /// Optional resize: clamp the longest edge to this many px.
    pub resize: Option<u32>,
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
    /// Debounce timer fired: actually launch the background render.
    DoRender,
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
    /// Debounced: commit the current adjustment state to the undo history.
    CommitHistory,
    /// Undo / redo the adjustment history (Ctrl+Z / Ctrl+Shift+Z).
    Undo,
    Redo,
    /// Toggle the before/after view (show the unedited original).
    ToggleOriginal,
}

/// One undo/redo step: the full engine state plus the slider UI values needed to
/// restore the panel.
#[derive(Clone)]
struct HistEntry {
    adj: rapidraw_core::image_processing::AllAdjustments,
    lut: Option<Arc<Lut>>,
    vals: Vec<f64>,
}

/// Work sent to the persistent render thread. Keeping a single long-lived
/// thread lets the GpuProcessor (and its compiled shader) be reused across
/// renders instead of rebuilt per frame.
enum RenderJob {
    Preview {
        base: Arc<DynamicImage>,
        adj: Box<rapidraw_core::image_processing::AllAdjustments>,
        lut: Option<Arc<Lut>>,
        dim: u32,
    },
    Export {
        base: Arc<DynamicImage>,
        adj: Box<rapidraw_core::image_processing::AllAdjustments>,
        lut: Option<Arc<Lut>>,
        path: PathBuf,
        opts: ExportOpts,
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
    /// Preview scopes (histogram/waveform/vectorscope) above the panel.
    scopes: Scopes,
    /// Overlay for transient status toasts (export done, LUT loaded, …).
    toasts: adw::ToastOverlay,
    /// Pending debounce timer for the next render; replaced (restarting the
    /// timer) on each `RequestRender` so rapid drags coalesce into one render.
    render_timer: Option<glib::SourceId>,
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
    /// Debounce timer so a burst of slider changes records one history step.
    hist_timer: Option<glib::SourceId>,
    /// While true (during undo/redo restore), changes don't record history.
    suppress_history: bool,
    /// Last processed preview texture (for toggling back from "show original").
    last_tex: Option<gdk::MemoryTexture>,
    /// The unedited image at preview size (for "show original").
    original_tex: Option<gdk::MemoryTexture>,
    /// Whether the before/after view is currently showing the original.
    showing_original: bool,
}

impl AppModel {
    /// Restart the history-commit debounce timer.
    fn schedule_history(&mut self, sender: &ComponentSender<AppModel>) {
        if self.suppress_history {
            return;
        }
        if let Some(id) = self.hist_timer.take() {
            id.remove();
        }
        let sender = sender.clone();
        self.hist_timer = Some(glib::timeout_add_local_once(
            Duration::from_millis(500),
            move || sender.input(AppMsg::CommitHistory),
        ));
    }

    /// Apply the history entry at `hist_idx`: set engine state, restore the
    /// panel UI, and re-render. Does not record new history.
    fn apply_history(&mut self, sender: &ComponentSender<AppModel>) {
        let entry = self.history[self.hist_idx].clone();
        self.session.adjustments = entry.adj;
        self.session.lut = entry.lut;
        self.suppress_history = true;
        self.panel.restore(&entry.vals);
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
    for i in indices {
        let p = images[i].clone();
        let tok = gen_tok.clone();
        sender.oneshot_command(async move {
            if tok.load(Ordering::Relaxed) != gen {
                return CmdMsg::ThumbReady(gen, i, RgbaImage::new(1, 1)); // skipped
            }
            match rapidraw_core::load_base_image(&p) {
                Ok(img) => {
                    let (w, h) = img.dimensions();
                    let scaled = if w.max(h) > thumb_dim {
                        img.resize(thumb_dim, thumb_dim, image::imageops::FilterType::Triangle)
                    } else {
                        img
                    };
                    CmdMsg::ThumbReady(gen, i, scaled.to_rgba8())
                }
                Err(e) => {
                    log::warn!("thumb decode failed for {}: {e}", p.display());
                    CmdMsg::ThumbReady(gen, i, RgbaImage::new(1, 1))
                }
            }
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
                        lut,
                        path,
                        opts,
                    } => {
                        let res = rapidraw_core::render(&ctx, &base, &adj, lut, None)
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
                lut,
                dim,
            }) = latest_preview
            {
                match rapidraw_core::render(&ctx, &base, &adj, lut, Some(dim)) {
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

            adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {
                    pack_start = &gtk::Button {
                        set_label: "Open Folder",
                        connect_clicked => AppMsg::OpenFolderDialog,
                    },
                    pack_end = &gtk::Button {
                        set_icon_name: "emblem-system-symbolic",
                        set_tooltip_text: Some("Settings"),
                        connect_clicked => AppMsg::OpenSettings,
                    },
                    pack_end = &gtk::Button {
                        set_label: "Export",
                        connect_clicked => AppMsg::ExportDialog,
                    },
                },

                #[wrap(Some)]
                #[name = "toast_overlay"]
                set_content = &adw::ToastOverlay {
                    #[wrap(Some)]
                    #[name = "stack"]
                    set_child = &gtk::Stack {
                    set_vexpand: true,
                    set_hexpand: true,

                    add_named[Some("library")] = &gtk::ScrolledWindow {
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

                    add_named[Some("editor")] = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,

                        gtk::Box {
                            set_orientation: gtk::Orientation::Horizontal,
                            set_spacing: 4,
                            set_margin_all: 4,
                            gtk::Button {
                                set_icon_name: "go-previous-symbolic",
                                set_label: "Library",
                                connect_clicked => AppMsg::ShowLibrary,
                            },
                            gtk::Separator { set_orientation: gtk::Orientation::Vertical },
                            gtk::Button {
                                set_icon_name: "edit-undo-symbolic",
                                set_tooltip_text: Some("Undo (Ctrl+Z)"),
                                add_css_class: "flat",
                                connect_clicked => AppMsg::Undo,
                            },
                            gtk::Button {
                                set_icon_name: "edit-redo-symbolic",
                                set_tooltip_text: Some("Redo (Ctrl+Shift+Z)"),
                                add_css_class: "flat",
                                connect_clicked => AppMsg::Redo,
                            },
                            gtk::ToggleButton {
                                set_icon_name: "view-reveal-symbolic",
                                set_tooltip_text: Some("Show original (before/after)"),
                                add_css_class: "flat",
                                connect_toggled => AppMsg::ToggleOriginal,
                            },
                        },

                        #[name = "editor_page"]
                        gtk::Paned {
                            set_vexpand: true,
                            // Canvas on the left, adjustment panel on the right,
                            // with a draggable, mouse-resizable divider.
                            set_orientation: gtk::Orientation::Horizontal,
                            set_wide_handle: true,
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
            .detach();

        let render_tx = spawn_render_worker(engine.ctx.clone(), sender.clone());

        let model = AppModel {
            session: Session::default(),
            images: Vec::new(),
            images_shared: Rc::new(RefCell::new(Vec::new())),
            thumbs,
            canvas: EditorCanvas::new(),
            panel: AdjustPanel::new(&sender),
            right_col: gtk::Box::new(gtk::Orientation::Vertical, 4),
            scopes: Scopes::new(),
            toasts: adw::ToastOverlay::new(), // replaced by the view's overlay below
            render_timer: None,
            settings: Settings::default(),
            render_tx,
            thumb_gen: Arc::new(AtomicUsize::new(0)),
            thumb_loaded: Vec::new(),
            history: Vec::new(),
            hist_idx: 0,
            hist_timer: None,
            suppress_history: false,
            last_tex: None,
            original_tex: None,
            showing_original: false,
        };
        // Seed the engine struct with the UI defaults (e.g. vignette midpoint/
        // feather = 50) so effects behave like the original at zero amount.
        let mut model = model;
        controls::init_defaults(&mut model.session.adjustments.global);

        let flow_box = model.thumbs.widget();
        let images = model.images_shared.clone();
        let widgets = view_output!();
        model.toasts = widgets.toast_overlay.clone();
        // Undo/redo keyboard shortcuts (Ctrl+Z / Ctrl+Shift+Z, plus Ctrl+Y).
        let key = gtk::EventControllerKey::new();
        {
            let sender = sender.clone();
            key.connect_key_pressed(move |_, keyval, _, state| {
                if !state.contains(gdk::ModifierType::CONTROL_MASK) {
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
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        root.add_controller(key);
        // Editor page: canvas on the left; right column = scopes on top of the
        // adjustment panel. A Paned divider keeps the panel at a fixed,
        // mouse-resizable width that the photo zoom never disturbs.
        model.right_col.append(model.scopes.root());
        model.right_col.append(model.panel.root());
        let paned = &widgets.editor_page;
        paned.set_start_child(Some(model.canvas.root()));
        paned.set_end_child(Some(&model.right_col));
        // Start (canvas) absorbs window resizes and may shrink below its child's
        // size (clipped); the panel keeps its width unless the user drags.
        paned.set_resize_start_child(true);
        paned.set_shrink_start_child(true);
        paned.set_resize_end_child(false);
        paned.set_shrink_end_child(false);
        // Comfortable default panel width (~380px); window opens at 1440.
        paned.set_position(1060);
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
                self.images = library::scan_dir(&path);
                *self.images_shared.borrow_mut() = self.images.clone();
                log::info!("{} images", self.images.len());
                self.session.current_folder = Some(path);

                // Rebuild the thumbnail grid: one placeholder cell per image.
                let mut guard = self.thumbs.guard();
                guard.clear();
                for p in &self.images {
                    guard.push_back(p.clone());
                }
                drop(guard);

                // New generation; decode every thumbnail in the background.
                self.thumb_loaded = vec![false; self.images.len()];
                let gen = self.thumb_gen.fetch_add(1, Ordering::Relaxed) + 1;
                dispatch_thumbs(
                    &sender,
                    &self.thumb_gen,
                    gen,
                    self.settings.thumb_dim,
                    &self.images,
                    0..self.images.len(),
                );
            }
            AppMsg::OpenInEditor(path) => {
                log::info!("Open in editor: {}", path.display());
                // Pause thumbnail decoding while editing (frees the CPU).
                self.thumb_gen.fetch_add(1, Ordering::Relaxed);
                self.session.active_path = Some(path.clone());
                widgets.stack.set_visible_child_name("editor");
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
                // Debounce: drop any pending timer and start a fresh one. Rapid
                // slider drags thus collapse into a single DoRender.
                if let Some(id) = self.render_timer.take() {
                    id.remove();
                }
                let sender = sender.clone();
                self.render_timer = Some(glib::timeout_add_local_once(
                    Duration::from_millis(RENDER_DEBOUNCE_MS),
                    move || sender.input(AppMsg::DoRender),
                ));
            }
            AppMsg::DoRender => {
                self.render_timer = None;
                // Guard: nothing to render until a base image is loaded.
                let Some(base) = self.session.base_image.clone() else {
                    return;
                };
                // Hand off to the persistent render thread (reuses the cached
                // GpuProcessor, so slider drags stay smooth).
                let _ = self.render_tx.send(RenderJob::Preview {
                    base,
                    adj: Box::new(self.session.adjustments),
                    lut: self.session.lut.clone(),
                    dim: self.settings.preview_dim,
                });
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

                let fmt = gtk::DropDown::from_strings(&["JPEG", "PNG", "TIFF"]);
                let q = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
                q.set_value(90.0);
                let qrow = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                qrow.append(&gtk::Label::new(Some("JPEG quality")));
                qrow.append(&q);
                {
                    let q = q.clone();
                    fmt.connect_selected_notify(move |d| q.set_sensitive(d.selected() == 0));
                }

                // Resize: 0 = full resolution.
                let resize = gtk::SpinButton::with_range(0.0, 20000.0, 100.0);
                resize.set_value(0.0);
                let rrow = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                rrow.append(&gtk::Label::new(Some("Resize long edge (0=full)")));
                rrow.append(&resize);

                let go = gtk::Button::with_label("Export…");
                go.add_css_class("suggested-action");
                vb.append(&fmt);
                vb.append(&qrow);
                vb.append(&rrow);
                vb.append(&go);
                win.set_child(Some(&vb));

                let sender = sender.clone();
                let win_c = win.clone();
                go.connect_clicked(move |_| {
                    let format = match fmt.selected() {
                        1 => ExportFormat::Png,
                        2 => ExportFormat::Tiff,
                        _ => ExportFormat::Jpeg,
                    };
                    let r = resize.value() as u32;
                    let opts = ExportOpts {
                        format,
                        quality: q.value() as u8,
                        resize: (r > 0).then_some(r),
                    };
                    win_c.close();
                    sender.input(AppMsg::ExportConfigured(opts));
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
                    lut: self.session.lut.clone(),
                    path,
                    opts,
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
                widgets.stack.set_visible_child_name("library");
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
            AppMsg::CommitHistory => {
                self.hist_timer = None;
                if self.suppress_history {
                    return;
                }
                let cur = self.session.adjustments;
                let lut = self.session.lut.clone();
                let same = self
                    .history
                    .get(self.hist_idx)
                    .map(|e| {
                        bytemuck::bytes_of(&e.adj.global) == bytemuck::bytes_of(&cur.global)
                            && lut_eq(&e.lut, &lut)
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
                });
                self.hist_idx = self.history.len() - 1;
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
                let tex = if self.showing_original {
                    self.original_tex.as_ref()
                } else {
                    self.last_tex.as_ref()
                };
                if let Some(tex) = tex {
                    self.canvas.update_texture(tex);
                }
            }
        }
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
                // Start each image from defaults (unless disabled in settings),
                // rebuilding the panel so the controls reflect the reset.
                if self.settings.reset_on_open {
                    self.session.adjustments = Default::default();
                    controls::init_defaults(&mut self.session.adjustments.global);
                    self.session.lut = None;
                    let fresh = AdjustPanel::new(&sender);
                    self.right_col.remove(self.panel.root());
                    self.right_col.append(fresh.root());
                    self.panel = fresh;
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
                let tex = library::texture_from_rgba(&rgba);
                self.last_tex = Some(tex.clone());
                // Preserve the user's zoom/pan across preview updates. Don't
                // clobber the canvas while the user is viewing the original.
                if !self.showing_original {
                    self.canvas.update_texture(&tex);
                }
            }
            CmdMsg::ExportDone(Ok(path)) => {
                log::info!("export saved: {}", path.display());
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.toasts.add_toast(adw::Toast::new(&format!("Saved {name}")));
            }
            CmdMsg::ExportDone(Err(e)) => {
                log::warn!("export failed: {e}");
                self.toasts
                    .add_toast(adw::Toast::new(&format!("Export failed: {e}")));
            }
        }
    }
}

/// Encode a rendered image to `path` per `opts` (format, JPEG quality, resize).
/// Identity comparison for the active LUT (shared `Arc`, or both absent).
fn lut_eq(a: &Option<Arc<Lut>>, b: &Option<Arc<Lut>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

fn encode_image(img: &DynamicImage, path: &std::path::Path, opts: ExportOpts) -> Result<(), String> {
    use image::{GenericImageView, ImageEncoder};
    let img = match opts.resize {
        Some(m) if img.dimensions().0.max(img.dimensions().1) > m => {
            img.resize(m, m, image::imageops::FilterType::Lanczos3)
        }
        _ => img.clone(),
    };
    match opts.format {
        ExportFormat::Png => img
            .to_rgb8()
            .save_with_format(path, image::ImageFormat::Png)
            .map_err(|e| e.to_string()),
        ExportFormat::Tiff => img
            .to_rgb8()
            .save_with_format(path, image::ImageFormat::Tiff)
            .map_err(|e| e.to_string()),
        ExportFormat::Jpeg => {
            let rgb = img.to_rgb8();
            let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
                std::io::BufWriter::new(file),
                opts.quality,
            );
            enc.write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| e.to_string())
        }
    }
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
    let processed = rapidraw_core::render(ctx, &identity, adj, lut, None)?;
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
