//! Right-side adjustment panel: all default (global) editor sections.
//!
//! Plain GTK (not a nested relm4 component). `AdjustPanel` owns a
//! `ScrolledWindow` of collapsible sections, each holding labeled `gtk::Scale`
//! rows. Every scale writes one `GlobalAdjustments` field via a fn-pointer
//! setter (`AppMsg::Adjust`), which the model applies then requests a render.
//!
//! Ranges and step increments mirror the default RapidRAW UI
//! (`src/components/adjustments/*.tsx`) so slider sensitivity matches exactly.
//! Masks, curves, HSL and color-grading (non-scalar / complex) are out of scope.

use gtk::prelude::*;
use relm4::{ComponentSender, RelmWidgetExt};

use crate::{AppModel, AppMsg};
use rapidraw_core::image_processing::GlobalAdjustments;

/// Writes one f32 field of `GlobalAdjustments`.
type Setter = fn(&mut GlobalAdjustments, f32);

/// One slider: `(label, min, max, step, setter)`.
type Row = (&'static str, f64, f64, f64, Setter);

/// Default editor sections, ranges/steps copied verbatim from the React UI.
const SECTIONS: &[(&str, &[Row])] = &[
    (
        "Basic",
        &[
            ("Exposure", -5.0, 5.0, 0.01, |g, v| g.exposure = v),
            ("Contrast", -100.0, 100.0, 1.0, |g, v| g.contrast = v),
            ("Highlights", -100.0, 100.0, 1.0, |g, v| g.highlights = v),
            ("Shadows", -100.0, 100.0, 1.0, |g, v| g.shadows = v),
            ("Whites", -100.0, 100.0, 1.0, |g, v| g.whites = v),
            ("Blacks", -100.0, 100.0, 1.0, |g, v| g.blacks = v),
        ],
    ),
    (
        "Color",
        &[
            ("Temperature", -100.0, 100.0, 1.0, |g, v| g.temperature = v),
            ("Tint", -100.0, 100.0, 1.0, |g, v| g.tint = v),
            ("Vibrance", -100.0, 100.0, 1.0, |g, v| g.vibrance = v),
            ("Saturation", -100.0, 100.0, 1.0, |g, v| g.saturation = v),
            ("Hue", -180.0, 180.0, 1.0, |g, v| g.hue = v),
        ],
    ),
    (
        "Details",
        &[
            ("Sharpness", -100.0, 100.0, 1.0, |g, v| g.sharpness = v),
            ("Sharpness Threshold", 0.0, 80.0, 1.0, |g, v| {
                g.sharpness_threshold = v
            }),
            ("Clarity", -100.0, 100.0, 1.0, |g, v| g.clarity = v),
            ("Dehaze", -100.0, 100.0, 1.0, |g, v| g.dehaze = v),
            ("Structure", -100.0, 100.0, 1.0, |g, v| g.structure = v),
            ("Luminance NR", 0.0, 100.0, 1.0, |g, v| {
                g.luma_noise_reduction = v
            }),
            ("Color NR", 0.0, 100.0, 1.0, |g, v| g.color_noise_reduction = v),
            ("Chromatic Aberration R/C", -100.0, 100.0, 1.0, |g, v| {
                g.chromatic_aberration_red_cyan = v
            }),
            ("Chromatic Aberration B/Y", -100.0, 100.0, 1.0, |g, v| {
                g.chromatic_aberration_blue_yellow = v
            }),
        ],
    ),
    (
        "Effects",
        &[
            ("Glow", 0.0, 100.0, 1.0, |g, v| g.glow_amount = v),
            ("Halation", 0.0, 100.0, 1.0, |g, v| g.halation_amount = v),
            ("Light Flares", 0.0, 100.0, 1.0, |g, v| g.flare_amount = v),
            ("Vignette Amount", -100.0, 100.0, 1.0, |g, v| {
                g.vignette_amount = v
            }),
            ("Vignette Midpoint", 0.0, 100.0, 1.0, |g, v| {
                g.vignette_midpoint = v
            }),
            ("Vignette Roundness", -100.0, 100.0, 1.0, |g, v| {
                g.vignette_roundness = v
            }),
            ("Vignette Feather", 0.0, 100.0, 1.0, |g, v| {
                g.vignette_feather = v
            }),
            ("Grain Amount", 0.0, 100.0, 1.0, |g, v| g.grain_amount = v),
            ("Grain Size", 0.0, 100.0, 1.0, |g, v| g.grain_size = v),
            ("Grain Roughness", 0.0, 100.0, 1.0, |g, v| {
                g.grain_roughness = v
            }),
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

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&list));
        // Fixed sensible width; the editor canvas takes the remaining space.
        root.set_hexpand(false);
        root.set_vexpand(true);
        root.set_width_request(320);

        // Sliders forward the wheel to this adjustment instead of changing their
        // value, so scrolling over a slider still scrolls the panel.
        let vadj = root.vadjustment();

        for (title, rows) in SECTIONS {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 4);
            section.set_margin_all(6);
            for &(label, min, max, step, set) in *rows {
                section.append(&build_row(label, min, max, step, set, sender, &vadj));
            }

            let expander = gtk::Expander::new(Some(title));
            expander.set_expanded(true);
            expander.set_child(Some(&section));
            list.append(&expander);
        }

        list.append(&build_lut_section(sender, &vadj));

        Self { root }
    }

    /// Widget to insert into the editor page layout (right side).
    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }
}

/// Build a label + scale row. The wheel scrolls the panel (via `vadj`) instead
/// of changing the slider value; the slider only moves by click/drag.
fn build_row(
    label: &str,
    min: f64,
    max: f64,
    step: f64,
    set: Setter,
    sender: &ComponentSender<AppModel>,
    vadj: &gtk::Adjustment,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let lbl = gtk::Label::new(Some(label));
    lbl.set_halign(gtk::Align::Start);
    lbl.set_margin_top(4);

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    // Show decimals only for sub-unit steps (e.g. exposure 0.01).
    scale.set_digits(if step < 1.0 { 2 } else { 0 });
    scale.set_value(0.0);

    forward_wheel(&scale, vadj);

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

/// Make a slider's mouse wheel scroll the panel (`vadj`) instead of changing
/// its value. Captured before the Scale's own handler runs.
fn forward_wheel(scale: &gtk::Scale, vadj: &gtk::Adjustment) {
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    let vadj = vadj.clone();
    wheel.connect_scroll(move |_, _dx, dy| {
        let step = vadj.step_increment().max(40.0);
        let next = (vadj.value() + dy * step)
            .clamp(vadj.lower(), (vadj.upper() - vadj.page_size()).max(vadj.lower()));
        vadj.set_value(next);
        gtk::glib::Propagation::Stop
    });
    scale.add_controller(wheel);
}

/// The LUT section: load/clear a .cube/.3dl file plus an intensity slider
/// (0..100 mapped to the engine's 0.0..1.0 `lut_intensity`).
fn build_lut_section(sender: &ComponentSender<AppModel>, vadj: &gtk::Adjustment) -> gtk::Expander {
    let lut_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    lut_box.set_margin_all(6);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let load = gtk::Button::with_label("Load .cube");
    let clear = gtk::Button::with_label("Clear");
    {
        let sender = sender.clone();
        load.connect_clicked(move |_| sender.input(AppMsg::LoadLut));
    }
    {
        let sender = sender.clone();
        clear.connect_clicked(move |_| sender.input(AppMsg::ClearLut));
    }
    buttons.append(&load);
    buttons.append(&clear);
    lut_box.append(&buttons);

    let lbl = gtk::Label::new(Some("Intensity"));
    lbl.set_halign(gtk::Align::Start);
    lbl.set_margin_top(4);

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    scale.set_value(100.0); // matches the full-strength default set on load
    forward_wheel(&scale, vadj);
    {
        let sender = sender.clone();
        scale.connect_value_changed(move |s| {
            sender.input(AppMsg::Adjust(crate::Adjust {
                set: |g, v| g.lut_intensity = v / 100.0,
                value: s.value() as f32,
            }));
        });
    }
    lut_box.append(&lbl);
    lut_box.append(&scale);

    let expander = gtk::Expander::new(Some("LUT"));
    expander.set_expanded(true);
    expander.set_child(Some(&lut_box));
    expander
}
