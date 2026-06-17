use std::path::PathBuf;

use gtk::gdk;
use gtk::prelude::*;
use relm4::prelude::*;

/// A single thumbnail cell in the library grid.
pub struct Thumb {
    pub path: PathBuf,
    pub texture: Option<gdk::MemoryTexture>,
}

#[derive(Debug)]
pub enum ThumbMsg {
    /// Decoded texture arrived from a background worker (built on the main thread).
    SetTexture(gdk::MemoryTexture),
}

#[relm4::factory(pub)]
impl FactoryComponent for Thumb {
    type Init = PathBuf;
    type Input = ThumbMsg;
    type Output = ();
    type CommandOutput = ();
    type ParentWidget = gtk::FlowBox;

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Vertical,
            set_spacing: 4,
            set_width_request: 160,

            #[name = "picture"]
            gtk::Picture {
                set_size_request: (150, 150),
                set_content_fit: gtk::ContentFit::Contain,
                #[watch]
                set_paintable: self.texture.as_ref().map(|t| t.upcast_ref::<gdk::Paintable>()),
            },

            gtk::Label {
                set_ellipsize: gtk::pango::EllipsizeMode::Middle,
                set_max_width_chars: 18,
                #[watch]
                set_label: &self
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string(),
            },
        }
    }

    fn init_model(path: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        Self {
            path,
            texture: None,
        }
    }

    fn update(&mut self, msg: Self::Input, _sender: FactorySender<Self>) {
        match msg {
            ThumbMsg::SetTexture(tex) => {
                self.texture = Some(tex);
            }
        }
    }
}
