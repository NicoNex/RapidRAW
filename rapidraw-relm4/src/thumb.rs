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

#[derive(Debug)]
pub enum ThumbOut {
    /// User clicked star `n` (1..=5) on this thumbnail.
    Rate(PathBuf, u8),
}

/// "★★★☆☆" for a 0..5 rating.
fn stars(r: u8) -> String {
    (1..=5).map(|i| if i <= r { '★' } else { '☆' }).collect()
}

#[relm4::factory(pub)]
impl FactoryComponent for Thumb {
    type Init = (PathBuf, u8);
    type Input = ThumbMsg;
    type Output = ThumbOut;
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

            // Single lightweight label (one widget per cell) — click position picks the
            // star. Far cheaper than 5 buttons per thumbnail, which made the grid lag.
            #[name = "star_label"]
            gtk::Label {
                add_css_class: "thumb-stars",
                set_halign: gtk::Align::Center,
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

    fn init_widgets(
        &mut self,
        _index: &DynamicIndex,
        root: Self::Root,
        _returned_widget: &gtk::FlowBoxChild,
        sender: FactorySender<Self>,
    ) -> Self::Widgets {
        let widgets = view_output!();
        let click = gtk::GestureClick::new();
        let s = sender.clone();
        let p = self.path.clone();
        let label = widgets.star_label.clone();
        click.connect_released(move |_, _, x, _| {
            let w = label.width().max(1) as f64;
            let n = (((x / w) * 5.0).ceil() as i64).clamp(1, 5) as u8;
            s.output(ThumbOut::Rate(p.clone(), n)).ok();
        });
        widgets.star_label.add_controller(click);
        widgets
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
