use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use image::{DynamicImage, GenericImageView, RgbaImage};
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;

mod controls;
mod editor;
mod library;
mod state;
mod thumb;
use controls::AdjustPanel;
use editor::EditorCanvas;
use state::{Engine, Session};
use thumb::{Thumb, ThumbMsg};

/// Longest-edge size (px) used for library thumbnails.
const THUMB_DIM: u32 = 300;
/// Longest-edge size (px) for the live editor preview render.
const PREVIEW_DIM: u32 = 2048;
/// Debounce window (ms) for coalescing rapid slider drags into one render.
const RENDER_DEBOUNCE_MS: u64 = 80;

/// Identifies a single exposed `GlobalAdjustments` f32 field. The slider panel
/// in `controls.rs` builds one row per variant; `with` packages a new value
/// into the `Adjust` message the model applies.
#[derive(Debug, Clone, Copy)]
pub enum AdjustField {
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Temperature,
    Tint,
    Vibrance,
    Saturation,
    Clarity,
    Dehaze,
    Structure,
    Sharpness,
    LumaNoiseReduction,
    ColorNoiseReduction,
    Vignette,
    Grain,
}

impl AdjustField {
    /// Pair this field with a new value for delivery via `AppMsg::Adjust`.
    pub fn with(self, value: f32) -> Adjust {
        Adjust { field: self, value }
    }
}

/// A field/value pair written into `session.adjustments.global` on apply.
#[derive(Debug, Clone, Copy)]
pub struct Adjust {
    field: AdjustField,
    value: f32,
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
    /// Export button: open the save dialog.
    ExportDialog,
    /// A save path was chosen: full-res render + JPEG encode to it.
    ExportTo(PathBuf),
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

struct AppModel {
    engine: Engine,
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
}

#[relm4::component]
impl Component for AppModel {
    type Init = Engine;
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = CmdMsg;

    view! {
        gtk::Window {
            set_title: Some("RapidRAW"),
            set_default_size: (1440, 900),

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                gtk::HeaderBar {
                    pack_start = &gtk::Button {
                        set_label: "Open Folder",
                        connect_clicked => AppMsg::OpenFolderDialog,
                    },
                    pack_end = &gtk::Button {
                        set_label: "Export",
                        connect_clicked => AppMsg::ExportDialog,
                    },
                },

                #[name = "stack"]
                gtk::Stack {
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

                    #[name = "editor_page"]
                    add_named[Some("editor")] = &gtk::Box {
                        // Canvas on the left, adjustment panel on the right.
                        set_orientation: gtk::Orientation::Horizontal,
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

        let model = AppModel {
            engine,
            session: Session::default(),
            images: Vec::new(),
            images_shared: Rc::new(RefCell::new(Vec::new())),
            thumbs,
            canvas: EditorCanvas::new(),
            panel: AdjustPanel::new(&sender),
            render_timer: None,
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
                for (i, p) in self.images.iter().cloned().enumerate() {
                    sender.oneshot_command(async move {
                        match rapidraw_core::load_base_image(&p) {
                            Ok(img) => {
                                let (w, h) = img.dimensions();
                                let scaled = if w.max(h) > THUMB_DIM {
                                    img.resize(
                                        THUMB_DIM,
                                        THUMB_DIM,
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
                sender.oneshot_command(async move {
                    match rapidraw_core::load_base_image(&p) {
                        Ok(img) => CmdMsg::BaseReady(p, img),
                        Err(e) => {
                            log::warn!("base decode failed for {}: {e}", p.display());
                            CmdMsg::BaseReady(p, DynamicImage::new_rgba8(1, 1))
                        }
                    }
                });
            }
            AppMsg::Adjust(Adjust { field, value }) => {
                let g = &mut self.session.adjustments.global;
                match field {
                    AdjustField::Exposure => g.exposure = value,
                    AdjustField::Contrast => g.contrast = value,
                    AdjustField::Highlights => g.highlights = value,
                    AdjustField::Shadows => g.shadows = value,
                    AdjustField::Whites => g.whites = value,
                    AdjustField::Blacks => g.blacks = value,
                    AdjustField::Temperature => g.temperature = value,
                    AdjustField::Tint => g.tint = value,
                    AdjustField::Vibrance => g.vibrance = value,
                    AdjustField::Saturation => g.saturation = value,
                    AdjustField::Clarity => g.clarity = value,
                    AdjustField::Dehaze => g.dehaze = value,
                    AdjustField::Structure => g.structure = value,
                    AdjustField::Sharpness => g.sharpness = value,
                    AdjustField::LumaNoiseReduction => g.luma_noise_reduction = value,
                    AdjustField::ColorNoiseReduction => g.color_noise_reduction = value,
                    AdjustField::Vignette => g.vignette_amount = value,
                    AdjustField::Grain => g.grain_amount = value,
                }
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
                let ctx = self.engine.ctx.clone();
                let adj = self.session.adjustments.clone();
                // ponytail: build a new GpuProcessor per render call; cache one
                // keyed by max dimensions if slider latency is too high.
                sender.oneshot_command(async move {
                    match rapidraw_core::render(&ctx, &base, &adj, Some(PREVIEW_DIM)) {
                        Ok(out) => CmdMsg::RenderReady(out.to_rgba8()),
                        Err(e) => {
                            log::warn!("preview render failed: {e}");
                            // Reuse RenderReady with an empty image as a no-op
                            // signal; update_cmd ignores 1x1 results.
                            CmdMsg::RenderReady(RgbaImage::new(1, 1))
                        }
                    }
                });
            }
            AppMsg::ExportDialog => {
                if self.session.base_image.is_none() {
                    log::warn!("export: no image open");
                    return;
                }
                // Default filename: <source stem>.jpg.
                let suggested = self
                    .session
                    .active_path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| format!("{}.jpg", s.to_string_lossy()))
                    .unwrap_or_else(|| "export.jpg".to_string());
                let dialog = gtk::FileDialog::builder()
                    .title("Export JPEG")
                    .initial_name(suggested)
                    .build();
                let parent = root.clone();
                let sender = sender.clone();
                dialog.save(Some(&parent), gtk::gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res {
                        if let Some(path) = file.path() {
                            sender.input(AppMsg::ExportTo(path));
                        }
                    }
                });
            }
            AppMsg::ExportTo(path) => {
                let Some(base) = self.session.base_image.clone() else {
                    return;
                };
                let ctx = self.engine.ctx.clone();
                let adj = self.session.adjustments.clone();
                log::info!("exporting to {}", path.display());
                sender.oneshot_command(async move {
                    // Full-res render (no downscale), then JPEG encode to the path.
                    let result = rapidraw_core::render(&ctx, &base, &adj, None)
                        .and_then(|out| {
                            out.to_rgb8()
                                .save_with_format(&path, image::ImageFormat::Jpeg)
                                .map_err(|e| e.to_string())
                        })
                        .map(|()| path);
                    CmdMsg::ExportDone(result)
                });
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

fn main() {
    env_logger::init();

    let ctx = rapidraw_core::headless_context().expect("gpu init");
    let engine = Engine { ctx: Arc::new(ctx) };
    log::info!("GPU context initialized");

    let app = RelmApp::new("com.rapidraw.relm4");
    app.run::<AppModel>(engine);
}
