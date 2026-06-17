use std::sync::Arc;

use gtk::prelude::*;
use relm4::prelude::*;

mod state;
use state::{Engine, Session};

struct AppModel {
    engine: Engine,
    session: Session,
}

#[relm4::component]
impl SimpleComponent for AppModel {
    type Init = Engine;
    type Input = ();
    type Output = ();

    view! {
        gtk::Window {
            set_title: Some("RapidRAW"),
            set_default_size: (1440, 900),
            gtk::Label {
                set_label: "RapidRAW (relm4) — scaffold",
            },
        }
    }

    fn init(
        engine: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = AppModel {
            engine,
            session: Session::default(),
        };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }
}

fn main() {
    env_logger::init();

    let ctx = rapidraw_core::headless_context().expect("gpu init");
    let engine = Engine {
        ctx: Arc::new(ctx),
    };
    log::info!("GPU context initialized");

    let app = RelmApp::new("com.rapidraw.relm4");
    app.run::<AppModel>(engine);
}
