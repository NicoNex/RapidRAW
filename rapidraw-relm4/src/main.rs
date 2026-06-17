use gtk::prelude::*;
use relm4::prelude::*;

struct AppModel;

#[relm4::component]
impl SimpleComponent for AppModel {
    type Init = ();
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
        _init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = AppModel;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }
}

fn main() {
    env_logger::init();
    let app = RelmApp::new("com.rapidraw.relm4");
    app.run::<AppModel>(());
}
