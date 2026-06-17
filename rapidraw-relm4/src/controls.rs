//! Right-side adjustment panel: all default (global) editor sections.
//!
//! Plain GTK (not a nested relm4 component). `AdjustPanel` owns a
//! `ScrolledWindow` of collapsible sections, each holding labeled `gtk::Scale`
//! rows. Every scale writes one `GlobalAdjustments` field via a fn-pointer
//! setter (`AppMsg::Adjust`), which the model applies then requests a render.
//!
//! Masks, curves, HSL and color-grading (non-scalar / complex) are out of scope.

use gtk::prelude::*;
use relm4::{ComponentSender, RelmWidgetExt};

use crate::{AppModel, AppMsg};
use rapidraw_core::image_processing::GlobalAdjustments;

/// Writes one f32 field of `GlobalAdjustments`.
type Setter = fn(&mut GlobalAdjustments, f32);

/// One slider: `(label, min, max, setter)`.
type Row = (&'static str, f64, f64, Setter);

/// The default editor sections, each a list of scalar sliders. Ranges mirror
/// the Tauri frontend; neutral value for every field is 0 (engine default).
const SECTIONS: &[(&str, &[Row])] = &[
    (
        "Basic",
        &[
            ("Exposure", -5.0, 5.0, |g, v| g.exposure = v),
            ("Brightness", -100.0, 100.0, |g, v| g.brightness = v),
            ("Contrast", -100.0, 100.0, |g, v| g.contrast = v),
            ("Highlights", -100.0, 100.0, |g, v| g.highlights = v),
            ("Shadows", -100.0, 100.0, |g, v| g.shadows = v),
            ("Whites", -100.0, 100.0, |g, v| g.whites = v),
            ("Blacks", -100.0, 100.0, |g, v| g.blacks = v),
        ],
    ),
    (
        "Color",
        &[
            ("Temperature", -100.0, 100.0, |g, v| g.temperature = v),
            ("Tint", -100.0, 100.0, |g, v| g.tint = v),
            ("Vibrance", -100.0, 100.0, |g, v| g.vibrance = v),
            ("Saturation", -100.0, 100.0, |g, v| g.saturation = v),
            ("Hue", -180.0, 180.0, |g, v| g.hue = v),
        ],
    ),
    (
        "Details",
        &[
            ("Sharpness", 0.0, 100.0, |g, v| g.sharpness = v),
            ("Sharpness Threshold", 0.0, 100.0, |g, v| {
                g.sharpness_threshold = v
            }),
            ("Luminance NR", 0.0, 100.0, |g, v| g.luma_noise_reduction = v),
            ("Color NR", 0.0, 100.0, |g, v| g.color_noise_reduction = v),
        ],
    ),
    (
        "Effects",
        &[
            ("Clarity", -100.0, 100.0, |g, v| g.clarity = v),
            ("Dehaze", -100.0, 100.0, |g, v| g.dehaze = v),
            ("Structure", -100.0, 100.0, |g, v| g.structure = v),
            ("Vignette", -100.0, 100.0, |g, v| g.vignette_amount = v),
            ("Vignette Midpoint", 0.0, 100.0, |g, v| {
                g.vignette_midpoint = v
            }),
            ("Vignette Roundness", -100.0, 100.0, |g, v| {
                g.vignette_roundness = v
            }),
            ("Vignette Feather", 0.0, 100.0, |g, v| g.vignette_feather = v),
            ("Grain", 0.0, 100.0, |g, v| g.grain_amount = v),
            ("Grain Size", 0.0, 100.0, |g, v| g.grain_size = v),
            ("Grain Roughness", 0.0, 100.0, |g, v| g.grain_roughness = v),
            ("Chromatic Aberration R/C", -100.0, 100.0, |g, v| {
                g.chromatic_aberration_red_cyan = v
            }),
            ("Chromatic Aberration B/Y", -100.0, 100.0, |g, v| {
                g.chromatic_aberration_blue_yellow = v
            }),
            ("Glow", 0.0, 100.0, |g, v| g.glow_amount = v),
            ("Halation", 0.0, 100.0, |g, v| g.halation_amount = v),
            ("Flare", 0.0, 100.0, |g, v| g.flare_amount = v),
        ],
    ),
];

/// Owns the right-side panel widget tree.
pub struct AdjustPanel {
    root: gtk::ScrolledWindow,
}

impl AdjustPanel {
    pub fn new(sender: &ComponentSender<AppModel>) -> Self {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        list.set_margin_all(8);

        for (title, rows) in SECTIONS {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 4);
            section.set_margin_all(6);
            for &(label, min, max, set) in *rows {
                section.append(&build_row(label, min, max, set, sender));
            }

            let expander = gtk::Expander::new(Some(title));
            expander.set_expanded(true);
            expander.set_child(Some(&section));
            list.append(&expander);
        }

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&list));
        // Fixed sensible width; the editor canvas takes the remaining space.
        root.set_hexpand(false);
        root.set_vexpand(true);
        root.set_width_request(320);

        Self { root }
    }

    /// Widget to insert into the editor page layout (right side).
    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }
}

/// Build a label + scale row. The scale ignores the mouse wheel (so scrolling
/// the panel never nudges a value) and feeds changes to the model.
fn build_row(
    label: &str,
    min: f64,
    max: f64,
    set: Setter,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let lbl = gtk::Label::new(Some(label));
    lbl.set_halign(gtk::Align::Start);
    lbl.set_margin_top(4);

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 1.0);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    scale.set_value(0.0);

    // Eat scroll events in the capture phase so the wheel never changes the
    // value; scrolling over a slider simply does nothing.
    let stop_scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    stop_scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    stop_scroll.connect_scroll(|_, _, _| gtk::glib::Propagation::Stop);
    scale.add_controller(stop_scroll);

    let sender = sender.clone();
    scale.connect_value_changed(move |s| {
        sender.input(AppMsg::Adjust(crate::Adjust {
            set,
            value: s.value() as f32,
        }));
    });

    row.append(&lbl);
    row.append(&scale);
    row
}
