use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
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
mod settings;
mod state;
mod thumb;
use controls::AdjustPanel;
use curves::Channel;
use editor::EditorCanvas;
use rapidraw_core::image_processing::{GlobalAdjustments, Point};
use rapidraw_core::lut_processing::{parse_lut_file, Lut};
use settings::Settings;
use state::{Engine, Session};
use thumb::{Thumb, ThumbMsg};

/// Debounce window (ms) for coalescing rapid slider drags into one render.
const RENDER_DEBOUNCE_MS: u64 = 80;

/// Output options for export.
#[derive(Debug, Clone, Copy)]
pub struct ExportOpts {
    /// PNG when true, otherwise JPEG.
    pub png: bool,
    /// JPEG quality 1..=100 (ignored for PNG).
    pub quality: u8,
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
    /// Return from the editor to the thumbnail grid.
    ShowLibrary,
    /// Open the settings window.
    OpenSettings,
    /// Settings changed in the settings window.
    SettingsChanged(Settings),
    /// A tone curve changed: channel + points (x,y in 0..255).
    CurveChanged(Channel, Vec<(f32, f32)>),
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
}

#[derive(Debug)]
enum CmdMsg {
    /// A worker finished decoding+downscaling a thumbnail. Carries the factory
    /// index and the raw RGBA pixels (the gdk texture is built on the main thread).
    ThumbReady(usize, RgbaImage),
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
    /// Pending debounce timer for the next render; replaced (restarting the
    /// timer) on each `RequestRender` so rapid drags coalesce into one render.
    render_timer: Option<glib::SourceId>,
    /// User settings (preview/thumbnail size, editor background).
    settings: Settings,
    /// Channel to the persistent render thread.
    render_tx: std::sync::mpsc::Sender<RenderJob>,
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
        while let Ok(job) = rx.recv() {
            match job {
                RenderJob::Preview {
                    base,
                    adj,
                    lut,
                    dim,
                } => match rapidraw_core::render(&ctx, &base, &adj, lut, Some(dim)) {
                    Ok(out) => {
                        let _ = cmd.send(CmdMsg::RenderReady(out.to_rgba8()));
                    }
                    Err(e) => log::warn!("preview render failed: {e}"),
                },
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
                #[name = "stack"]
                set_content = &gtk::Stack {
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
                            set_margin_all: 4,
                            gtk::Button {
                                set_icon_name: "go-previous-symbolic",
                                set_label: "Library",
                                connect_clicked => AppMsg::ShowLibrary,
                            },
                        },

                        #[name = "editor_page"]
                        gtk::Box {
                            set_vexpand: true,
                            // Canvas on the left, adjustment panel on the right.
                            set_orientation: gtk::Orientation::Horizontal,
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
            render_timer: None,
            settings: Settings::default(),
            render_tx,
        };

        let flow_box = model.thumbs.widget();
        let images = model.images_shared.clone();
        let widgets = view_output!();
        // Attach the editor canvas (left) and adjustment panel (right) into the
        // (otherwise empty) editor page.
        widgets.editor_page.append(model.canvas.root());
        widgets.editor_page.append(model.panel.root());
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

                // Kick off a background decode per image; results stream back as
                // CmdMsg::ThumbReady and the texture is built on the main thread.
                let thumb_dim = self.settings.thumb_dim;
                for (i, p) in self.images.iter().cloned().enumerate() {
                    sender.oneshot_command(async move {
                        match rapidraw_core::load_base_image(&p) {
                            Ok(img) => {
                                let (w, h) = img.dimensions();
                                let scaled = if w.max(h) > thumb_dim {
                                    img.resize(
                                        thumb_dim,
                                        thumb_dim,
                                        image::imageops::FilterType::Triangle,
                                    )
                                } else {
                                    img
                                };
                                CmdMsg::ThumbReady(i, scaled.to_rgba8())
                            }
                            Err(e) => {
                                log::warn!("thumb decode failed for {}: {e}", p.display());
                                CmdMsg::ThumbReady(i, RgbaImage::new(1, 1))
                            }
                        }
                    });
                }
            }
            AppMsg::OpenInEditor(path) => {
                log::info!("Open in editor: {}", path.display());
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

                let fmt = gtk::DropDown::from_strings(&["JPEG", "PNG"]);
                let q = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
                q.set_value(90.0);
                let qrow = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                qrow.append(&gtk::Label::new(Some("JPEG quality")));
                qrow.append(&q);
                // Quality only applies to JPEG.
                {
                    let q = q.clone();
                    fmt.connect_selected_notify(move |d| q.set_sensitive(d.selected() == 0));
                }

                let go = gtk::Button::with_label("Export…");
                go.add_css_class("suggested-action");
                vb.append(&fmt);
                vb.append(&qrow);
                vb.append(&go);
                win.set_child(Some(&vb));

                let sender = sender.clone();
                let win_c = win.clone();
                go.connect_clicked(move |_| {
                    let opts = ExportOpts {
                        png: fmt.selected() == 1,
                        quality: q.value() as u8,
                    };
                    win_c.close();
                    sender.input(AppMsg::ExportConfigured(opts));
                });
                win.present();
            }
            AppMsg::ExportConfigured(opts) => {
                let ext = if opts.png { "png" } else { "jpg" };
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
                    sender.input(AppMsg::RequestRender);
                }
                Err(e) => log::warn!("LUT parse failed for {}: {e}", path.display()),
            },
            AppMsg::ClearLut => {
                self.session.lut = None;
                sender.input(AppMsg::RequestRender);
            }
            AppMsg::ShowLibrary => {
                widgets.stack.set_visible_child_name("library");
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
                sender.input(AppMsg::RequestRender);
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
            CmdMsg::ThumbReady(i, rgba) => {
                // Build the gdk texture here, on the main thread.
                let tex = library::texture_from_rgba(&rgba);
                self.thumbs.send(i, ThumbMsg::SetTexture(tex));
            }
            CmdMsg::BaseReady(path, img) => {
                let (w, h) = img.dimensions();
                log::info!("base image ready: {} ({w}x{h})", path.display());
                // Show the un-adjusted base immediately. We're on the GTK main
                // thread here, so building the gdk texture is safe.
                let rgba = img.to_rgba8();
                let tex = library::texture_from_rgba(&rgba);
                self.canvas.set_texture(&tex);
                self.session.base_image = Some(Arc::new(img));
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
                let tex = library::texture_from_rgba(&rgba);
                self.canvas.set_texture(&tex);
            }
            CmdMsg::ExportDone(Ok(path)) => {
                log::info!("export saved: {}", path.display());
            }
            CmdMsg::ExportDone(Err(e)) => {
                log::warn!("export failed: {e}");
            }
        }
    }
}

/// Encode a rendered image to `path` as JPEG (with quality) or PNG.
fn encode_image(img: &DynamicImage, path: &std::path::Path, opts: ExportOpts) -> Result<(), String> {
    use image::ImageEncoder;
    let rgb = img.to_rgb8();
    if opts.png {
        rgb.save_with_format(path, image::ImageFormat::Png)
            .map_err(|e| e.to_string())
    } else {
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let enc =
            image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), opts.quality);
        enc.write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())
    }
}

fn main() {
    env_logger::init();

    let ctx = rapidraw_core::headless_context().expect("gpu init");
    let engine = Engine { ctx: Arc::new(ctx) };
    log::info!("GPU context initialized");

    let app = RelmApp::new("com.rapidraw.relm4");
    app.run::<AppModel>(engine);
}
