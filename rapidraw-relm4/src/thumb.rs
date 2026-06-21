use std::path::PathBuf;

use gtk::gdk;
use gtk::prelude::*;
use relm4::prelude::*;

/// A single thumbnail cell in the library grid.
pub struct Thumb {
    pub path: PathBuf,
    pub texture: Option<gdk::MemoryTexture>,
    pub rating: u8,
    /// Star buttons cached so `update` can relabel them without widget access.
    star_btns: Vec<gtk::Button>,
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

            gtk::Overlay {
                #[name = "picture"]
                gtk::Picture {
                    set_size_request: (150, 150),
                    set_content_fit: gtk::ContentFit::Contain,
                    #[watch]
                    set_paintable: self.texture.as_ref().map(|t| t.upcast_ref::<gdk::Paintable>()),
                },
                add_overlay = &gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::End,
                    set_spacing: 0,
                    add_css_class: "thumb-stars",
                    #[name = "star_box"]
                    gtk::Box {},
                },
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

    fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        let (path, rating) = init;
        Self {
            path,
            texture: None,
            rating,
            star_btns: Vec::new(),
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
        let path = self.path.clone();
        for i in 1..=5u8 {
            let b = gtk::Button::builder().css_classes(["flat", "star"]).build();
            b.set_label("☆");
            let s = sender.clone();
            let p = path.clone();
            b.connect_clicked(move |_| {
                s.output(ThumbOut::Rate(p.clone(), i)).ok();
            });
            widgets.star_box.append(&b);
            self.star_btns.push(b);
        }
        render_stars(&self.star_btns, self.rating);
        widgets
    }

    fn update(&mut self, msg: Self::Input, _sender: FactorySender<Self>) {
        match msg {
            ThumbMsg::SetTexture(tex) => {
                self.texture = Some(tex);
            }
            ThumbMsg::SetRating(r) => {
                self.rating = r;
                render_stars(&self.star_btns, r);
            }
        }
    }
}

fn render_stars(btns: &[gtk::Button], rating: u8) {
    for (i, btn) in btns.iter().enumerate() {
        btn.set_label(if (i as u8) < rating { "★" } else { "☆" });
    }
}
