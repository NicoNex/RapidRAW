use std::path::PathBuf;
use std::sync::Arc;

use gtk::prelude::*;
use relm4::prelude::*;

mod state;
use state::{Engine, Session};

#[derive(Debug)]
enum AppMsg {
    OpenFolderDialog,
    FolderChosen(PathBuf),
}

struct AppModel {
    engine: Engine,
    session: Session,
}

#[relm4::component]
impl Component for AppModel {
    type Init = Engine;
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = ();

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

                gtk::Label {
                    set_vexpand: true,
                    set_label: "RapidRAW (relm4) — scaffold",
                },
            },
        }
    }

    fn init(
        engine: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = AppModel {
            engine,
            session: Session::default(),
        };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            AppMsg::OpenFolderDialog => {
                let dialog = gtk::FileDialog::builder().title("Select folder").build();
                let parent = root.clone();
                let sender = sender.clone();
                dialog.select_folder(
                    Some(&parent),
                    gtk::gio::Cancellable::NONE,
                    move |res| {
                        if let Ok(file) = res {
                            if let Some(path) = file.path() {
                                sender.input(AppMsg::FolderChosen(path));
                            }
                        }
                    },
                );
            }
            AppMsg::FolderChosen(path) => {
                log::info!("Folder chosen: {}", path.display());
                self.session.current_folder = Some(path);
                // Phase 7: kick off scan
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
