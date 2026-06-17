//! Right-side adjustment panel: all default (global) editor sections.
//!
//! Plain GTK (not a nested relm4 component). `AdjustPanel` owns a
//! `ScrolledWindow` of collapsible sections, each holding labeled `gtk::Scale`
//! rows. Every scale writes one `GlobalAdjustments` field via a fn-pointer
//! setter (`AppMsg::Adjust`), which the model applies then requests a render.
//!
//! Ranges + step increments mirror the default RapidRAW UI; the per-field
//! `scale` divisor mirrors `image_processing::SCALES` so the value the shader
//! receives (and thus the slider sensitivity) matches the original exactly.
//! Masks/HSL out of scope; curves handled separately.

use gtk::prelude::*;
use relm4::{ComponentSender, RelmWidgetExt};

use crate::colorwheel::ColorWheel;
use crate::{AppModel, AppMsg};
use rapidraw_core::image_processing::GlobalAdjustments;

/// Writes one f32 field of `GlobalAdjustments` (value already scaled).
type Setter = fn(&mut GlobalAdjustments, f32);

/// One slider: `(label, min, max, step, scale, setter)`. The UI value is
/// divided by `scale` before being written (matching the engine parser).
type Row = (&'static str, f64, f64, f64, f64, Setter);

/// Default editor sections; ranges/steps/scales copied from the React UI +
/// `SCALES`.
const SECTIONS: &[(&str, &[Row])] = &[
    (
        "Basic",
        &[
            ("Exposure", -5.0, 5.0, 0.01, 0.8, |g, v| g.exposure = v),
            ("Contrast", -100.0, 100.0, 1.0, 100.0, |g, v| g.contrast = v),
            ("Highlights", -100.0, 100.0, 1.0, 120.0, |g, v| {
                g.highlights = v
            }),
            ("Shadows", -100.0, 100.0, 1.0, 120.0, |g, v| g.shadows = v),
            ("Whites", -100.0, 100.0, 1.0, 30.0, |g, v| g.whites = v),
            ("Blacks", -100.0, 100.0, 1.0, 70.0, |g, v| g.blacks = v),
        ],
    ),
    (
        "Color",
        &[
            ("Temperature", -100.0, 100.0, 1.0, 25.0, |g, v| {
                g.temperature = v
            }),
            ("Tint", -100.0, 100.0, 1.0, 100.0, |g, v| g.tint = v),
            ("Vibrance", -100.0, 100.0, 1.0, 100.0, |g, v| g.vibrance = v),
            ("Saturation", -100.0, 100.0, 1.0, 100.0, |g, v| {
                g.saturation = v
            }),
            ("Hue", -180.0, 180.0, 1.0, 1.0, |g, v| g.hue = v),
        ],
    ),
    (
        "Details",
        &[
            ("Sharpness", -100.0, 100.0, 1.0, 50.0, |g, v| g.sharpness = v),
            ("Sharpness Threshold", 0.0, 80.0, 1.0, 100.0, |g, v| {
                g.sharpness_threshold = v
            }),
            ("Clarity", -100.0, 100.0, 1.0, 200.0, |g, v| g.clarity = v),
            ("Dehaze", -100.0, 100.0, 1.0, 750.0, |g, v| g.dehaze = v),
            ("Structure", -100.0, 100.0, 1.0, 200.0, |g, v| {
                g.structure = v
            }),
            ("Luminance NR", 0.0, 100.0, 1.0, 100.0, |g, v| {
                g.luma_noise_reduction = v
            }),
            ("Color NR", 0.0, 100.0, 1.0, 100.0, |g, v| {
                g.color_noise_reduction = v
            }),
            ("Chromatic Aberration R/C", -100.0, 100.0, 1.0, 10000.0, |g, v| {
                g.chromatic_aberration_red_cyan = v
            }),
            ("Chromatic Aberration B/Y", -100.0, 100.0, 1.0, 10000.0, |g, v| {
                g.chromatic_aberration_blue_yellow = v
            }),
        ],
    ),
    (
        "Effects",
        &[
            ("Glow", 0.0, 100.0, 1.0, 100.0, |g, v| g.glow_amount = v),
            ("Halation", 0.0, 100.0, 1.0, 100.0, |g, v| {
                g.halation_amount = v
            }),
            ("Light Flares", 0.0, 100.0, 1.0, 100.0, |g, v| {
                g.flare_amount = v
            }),
            ("Vignette Amount", -100.0, 100.0, 1.0, 100.0, |g, v| {
                g.vignette_amount = v
            }),
            ("Vignette Midpoint", 0.0, 100.0, 1.0, 100.0, |g, v| {
                g.vignette_midpoint = v
            }),
            ("Vignette Roundness", -100.0, 100.0, 1.0, 100.0, |g, v| {
                g.vignette_roundness = v
            }),
            ("Vignette Feather", 0.0, 100.0, 1.0, 100.0, |g, v| {
                g.vignette_feather = v
            }),
            ("Grain Amount", 0.0, 100.0, 1.0, 200.0, |g, v| {
                g.grain_amount = v
            }),
            ("Grain Size", 0.0, 100.0, 1.0, 50.0, |g, v| g.grain_size = v),
            ("Grain Roughness", 0.0, 100.0, 1.0, 100.0, |g, v| {
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
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        list.set_margin_all(6);

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&list));
        root.set_hexpand(false);
        root.set_vexpand(true);
        root.set_width_request(320);

        let vadj = root.vadjustment();

        for (title, rows) in SECTIONS {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 2);
            section.set_margin_all(4);
            for &(label, min, max, step, scale, set) in *rows {
                section.append(&build_row(label, min, max, step, scale, set, sender, &vadj));
            }
            // Color grading wheels live under the Color section.
            if *title == "Color" {
                section.append(&build_grading_wheels(sender, &vadj));
            }

            let expander = gtk::Expander::new(Some(title));
            expander.set_expanded(true);
            expander.set_child(Some(&section));
            list.append(&expander);
        }

        list.append(&build_lut_section(sender, &vadj));

        Self { root }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }
}

#[allow(clippy::too_many_arguments)]
fn build_row(
    label: &str,
    min: f64,
    max: f64,
    step: f64,
    scale: f64,
    set: Setter,
    sender: &ComponentSender<AppModel>,
    vadj: &gtk::Adjustment,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let lbl = gtk::Label::new(Some(label));
    lbl.set_halign(gtk::Align::Start);
    lbl.add_css_class("caption");

    let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    s.set_hexpand(true);
    s.set_draw_value(true);
    s.set_digits(if step < 1.0 { 2 } else { 0 });
    s.set_value(0.0);

    forward_wheel(&s, vadj);

    {
        let sender = sender.clone();
        s.connect_value_changed(move |s| {
            // Divide by the field scale so the engine receives the same value
            // the original UI would have parsed.
            sender.input(AppMsg::Adjust(crate::Adjust {
                set,
                value: (s.value() / scale) as f32,
            }));
        });
    }

    // Double-click resets to default (0).
    let reset = gtk::GestureClick::new();
    {
        let s = s.clone();
        reset.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                s.set_value(0.0);
            }
        });
    }
    s.add_controller(reset);

    row.append(&lbl);
    row.append(&s);
    row
}

/// Make a slider's mouse wheel scroll the panel (`vadj`) instead of changing
/// its value. Captured before the widget's own handler runs.
pub fn forward_wheel(widget: &impl IsA<gtk::Widget>, vadj: &gtk::Adjustment) {
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
    widget.as_ref().add_controller(wheel);
}

/// The three color-grading wheels (shadows / midtones / highlights).
fn build_grading_wheels(sender: &ComponentSender<AppModel>, vadj: &gtk::Adjustment) -> gtk::FlowBox {
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_column_spacing(4);
    flow.set_row_spacing(4);
    flow.set_homogeneous(true);

    let shadows = ColorWheel::new(
        "Shadows",
        sender,
        vadj,
        |g, v| g.color_grading_shadows.hue = v,
        |g, v| g.color_grading_shadows.saturation = v,
        |g, v| g.color_grading_shadows.luminance = v,
    );
    let midtones = ColorWheel::new(
        "Midtones",
        sender,
        vadj,
        |g, v| g.color_grading_midtones.hue = v,
        |g, v| g.color_grading_midtones.saturation = v,
        |g, v| g.color_grading_midtones.luminance = v,
    );
    let highlights = ColorWheel::new(
        "Highlights",
        sender,
        vadj,
        |g, v| g.color_grading_highlights.hue = v,
        |g, v| g.color_grading_highlights.saturation = v,
        |g, v| g.color_grading_highlights.luminance = v,
    );
    flow.append(shadows.root());
    flow.append(midtones.root());
    flow.append(highlights.root());
    flow
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
    lbl.add_css_class("caption");

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
