//! Crop / geometry panel (right-rail "Crop" section): aspect-ratio crop,
//! 90° rotation, horizontal/vertical flip, and free straighten.
//!
//! These map to [`crate::Geometry`], applied to the base image (CPU) before the
//! GPU render. Aspect crop is currently centred (no interactive rectangle yet).

use gtk::prelude::*;
use relm4::{ComponentSender, RelmWidgetExt};

use crate::{AppModel, AppMsg};

/// Aspect presets: (label, ratio w/h; 0 = free, -1 = original/native).
const ASPECTS: &[(&str, f32)] = &[
    ("Free", 0.0),
    ("Original", -1.0),
    ("1:1", 1.0),
    ("5:4", 5.0 / 4.0),
    ("4:3", 4.0 / 3.0),
    ("3:2", 3.0 / 2.0),
    ("16:9", 16.0 / 9.0),
    ("21:9", 21.0 / 9.0),
    ("65:24", 65.0 / 24.0),
];

pub struct CropPanel {
    root: gtk::ScrolledWindow,
}

impl CropPanel {
    pub fn new(sender: &ComponentSender<AppModel>) -> Self {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        list.set_margin_all(10);

        // Aspect ratio.
        let aspect_lbl = gtk::Label::new(Some("Aspect Ratio"));
        aspect_lbl.set_halign(gtk::Align::Start);
        aspect_lbl.add_css_class("heading");
        let labels: Vec<&str> = ASPECTS.iter().map(|(l, _)| *l).collect();
        let aspect_dd = gtk::DropDown::from_strings(&labels);
        {
            let sender = sender.clone();
            aspect_dd.connect_selected_notify(move |dd| {
                let a = ASPECTS.get(dd.selected() as usize).map(|(_, r)| *r).unwrap_or(0.0);
                sender.input(AppMsg::CropAspect(a));
            });
        }
        let aspect_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        aspect_dd.set_hexpand(true);
        let swap = gtk::Button::from_icon_name("object-rotate-right-symbolic");
        swap.set_tooltip_text(Some("Swap orientation (e.g. 3:2 ↔ 2:3)"));
        {
            let sender = sender.clone();
            swap.connect_clicked(move |_| sender.input(AppMsg::CropSwapOrient));
        }
        aspect_row.append(&aspect_dd);
        aspect_row.append(&swap);
        list.append(&aspect_lbl);
        list.append(&aspect_row);

        // Rotate + flip.
        let rot_lbl = gtk::Label::new(Some("Rotate & Flip"));
        rot_lbl.set_halign(gtk::Align::Start);
        rot_lbl.add_css_class("heading");
        list.append(&rot_lbl);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let rot_ccw = gtk::Button::from_icon_name("object-rotate-left-symbolic");
        rot_ccw.set_tooltip_text(Some("Rotate left"));
        let rot_cw = gtk::Button::from_icon_name("object-rotate-right-symbolic");
        rot_cw.set_tooltip_text(Some("Rotate right"));
        let flip_h = gtk::ToggleButton::new();
        flip_h.set_icon_name("object-flip-horizontal-symbolic");
        flip_h.set_tooltip_text(Some("Flip horizontal"));
        let flip_v = gtk::ToggleButton::new();
        flip_v.set_icon_name("object-flip-vertical-symbolic");
        flip_v.set_tooltip_text(Some("Flip vertical"));
        {
            let sender = sender.clone();
            rot_ccw.connect_clicked(move |_| sender.input(AppMsg::RotateCcw));
        }
        {
            let sender = sender.clone();
            rot_cw.connect_clicked(move |_| sender.input(AppMsg::RotateCw));
        }
        {
            let sender = sender.clone();
            flip_h.connect_toggled(move |b| sender.input(AppMsg::FlipH(b.is_active())));
        }
        {
            let sender = sender.clone();
            flip_v.connect_toggled(move |b| sender.input(AppMsg::FlipV(b.is_active())));
        }
        row.append(&rot_ccw);
        row.append(&rot_cw);
        row.append(&flip_h);
        row.append(&flip_v);
        list.append(&row);

        // Straighten (free rotation, degrees).
        let straighten_lbl = gtk::Label::new(Some("Straighten"));
        straighten_lbl.set_halign(gtk::Align::Start);
        straighten_lbl.add_css_class("heading");
        list.append(&straighten_lbl);
        let vadj = gtk::Adjustment::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        {
            let sender = sender.clone();
            let s = crate::slider::slider(
                "Angle", -45.0, 45.0, 0.1, 0.0, crate::slider::Track::Plain, &vadj,
                move |v| sender.input(AppMsg::Straighten(v as f32)),
            );
            list.append(&s);
        }

        // Reset.
        let reset = gtk::Button::with_label("Reset crop");
        reset.add_css_class("flat");
        reset.set_margin_top(6);
        {
            let sender = sender.clone();
            reset.connect_clicked(move |_| sender.input(AppMsg::CropReset));
        }
        list.append(&reset);

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&list));
        root.set_hexpand(false);
        root.set_vexpand(true);
        root.set_width_request(320);

        Self { root }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }
}
