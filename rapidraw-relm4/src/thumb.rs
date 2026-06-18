use std::path::PathBuf;

use gtk::gdk;
use gtk::prelude::*;
use relm4::prelude::*;

/// A single thumbnail cell in the library grid.
pub struct Thumb {
    pub path: PathBuf,
    pub texture: Option<gdk::MemoryTexture>,
    pub rating: u8,
}

#[derive(Debug)]
pub enum ThumbMsg {
    /// Decoded texture arrived from a background worker (built on the main thread).
    SetTexture(gdk::MemoryTexture),
    /// Star rating changed (0..5).
    SetRating(u8),
}

/// "★★★☆☆" for a 0..5 rating.
fn stars(r: u8) -> String {
    (1..=5).map(|i| if i <= r { '★' } else { '☆' }).collect()
}

#[relm4::factory(pub)]
impl FactoryComponent for Thumb {
    type Init = (PathBuf, u8);
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

            gtk::Label {
                add_css_class: "caption",
                add_css_class: "dim-label",
                #[watch]
                set_label: &stars(self.rating),
            },
        }
    }

    fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        let (path, rating) = init;
        Self {
            path,
            texture: None,
            rating,
        }
    }

    fn update(&mut self, msg: Self::Input, _sender: FactorySender<Self>) {
        match msg {
            ThumbMsg::SetTexture(tex) => {
                self.texture = Some(tex);
            }
            ThumbMsg::SetRating(r) => {
                self.rating = r;
            }
        }
    }
}
