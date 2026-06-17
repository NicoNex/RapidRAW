//! Right-side adjustment slider panel.
//!
//! Like the editor canvas, this is plain GTK (not a nested relm4 component).
//! `AdjustPanel` owns a `gtk::Box` of `gtk::Scale` rows, one per exposed
//! `GlobalAdjustments` f32 field. Each scale's `value-changed` signal feeds an
//! `AppMsg::Adjust(Adjust::<Field>(v))` into the model, which writes the value
//! into `session.adjustments.global.<field>` and requests a render.

use gtk::prelude::*;
use relm4::{ComponentSender, RelmWidgetExt};

use crate::{AdjustField, AppModel, AppMsg};

/// Static table of the GlobalAdjustments fields we expose, with the slider
/// ranges copied from the Tauri frontend (`src/components/adjustments/*`).
/// `(field, label, min, max)`. Only fields that actually exist on
/// `GlobalAdjustments` are listed.
const FIELDS: &[(AdjustField, &str, f64, f64)] = &[
    (AdjustField::Exposure, "Exposure", -5.0, 5.0),
    (AdjustField::Contrast, "Contrast", -100.0, 100.0),
    (AdjustField::Highlights, "Highlights", -100.0, 100.0),
    (AdjustField::Shadows, "Shadows", -100.0, 100.0),
    (AdjustField::Whites, "Whites", -100.0, 100.0),
    (AdjustField::Blacks, "Blacks", -100.0, 100.0),
    (AdjustField::Temperature, "Temperature", -100.0, 100.0),
    (AdjustField::Tint, "Tint", -100.0, 100.0),
    (AdjustField::Vibrance, "Vibrance", -100.0, 100.0),
    (AdjustField::Saturation, "Saturation", -100.0, 100.0),
    (AdjustField::Clarity, "Clarity", -100.0, 100.0),
    (AdjustField::Dehaze, "Dehaze", -100.0, 100.0),
    (AdjustField::Structure, "Structure", -100.0, 100.0),
    (AdjustField::Sharpness, "Sharpness", -100.0, 100.0),
    (AdjustField::LumaNoiseReduction, "Luminance NR", 0.0, 100.0),
    (AdjustField::ColorNoiseReduction, "Color NR", 0.0, 100.0),
    (AdjustField::Vignette, "Vignette", -100.0, 100.0),
    (AdjustField::Grain, "Grain", 0.0, 100.0),
];

/// Owns the right-side panel widget tree.
pub struct AdjustPanel {
    root: gtk::ScrolledWindow,
}

impl AdjustPanel {
    pub fn new(sender: &ComponentSender<AppModel>) -> Self {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        list.set_margin_all(8);
        list.set_width_request(280);

        for &(field, label, min, max) in FIELDS {
            let lbl = gtk::Label::new(Some(label));
            lbl.set_halign(gtk::Align::Start);
            lbl.set_margin_top(6);

            let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 1.0);
            scale.set_hexpand(true);
            scale.set_draw_value(true);
            scale.set_value(0.0);

            // value-changed closures must be 'static: clone the sender in.
            let sender = sender.clone();
            scale.connect_value_changed(move |s| {
                sender.input(AppMsg::Adjust(field.with(s.value() as f32)));
            });

            list.append(&lbl);
            list.append(&scale);
        }

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&list));

        Self { root }
    }

    /// Widget to insert into the editor page layout (right side).
    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }
}
