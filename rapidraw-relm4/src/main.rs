use std::path::PathBuf;
use std::sync::Arc;

use gtk::prelude::*;
use image::{GenericImageView, RgbaImage};
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;

mod library;
mod state;
mod thumb;
use state::{Engine, Session};
use thumb::{Thumb, ThumbMsg};

/// Longest-edge size (px) used for library thumbnails.
const THUMB_DIM: u32 = 300;

#[derive(Debug)]
enum AppMsg {
    OpenFolderDialog,
    FolderChosen(PathBuf),
}

#[derive(Debug)]
enum CmdMsg {
    /// A worker finished decoding+downscaling a thumbnail. Carries the factory
    /// index and the raw RGBA pixels (the gdk texture is built on the main thread).
    ThumbReady(usize, RgbaImage),
}

struct AppModel {
    engine: Engine,
    session: Session,
    images: Vec<PathBuf>,
    thumbs: FactoryVecDeque<Thumb>,
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
                },

                gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_hexpand: true,
                    set_hscrollbar_policy: gtk::PolicyType::Never,

                    #[local_ref]
                    flow_box -> gtk::FlowBox {
                        set_valign: gtk::Align::Start,
                        set_selection_mode: gtk::SelectionMode::Single,
                        set_homogeneous: true,
                        set_column_spacing: 8,
                        set_row_spacing: 8,
                        set_margin_all: 8,
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
            thumbs,
        };

        let flow_box = model.thumbs.widget();
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
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
        }
    }

    fn update_cmd(
        &mut self,
        msg: Self::CommandOutput,
        _sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match msg {
            CmdMsg::ThumbReady(i, rgba) => {
                // Build the gdk texture here, on the main thread.
                let tex = library::texture_from_rgba(&rgba);
                self.thumbs.send(i, ThumbMsg::SetTexture(tex));
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
