//! Right-side adjustment panel: all default (global) editor sections, visually
//! grouped as cards (Curves, Basic, Color, Details, Effects, LUT).
//!
//! Every scale writes one `GlobalAdjustments` field via a fn-pointer setter
//! (`AppMsg::Adjust`); the UI value is divided by the per-field `scale`
//! (mirroring `image_processing::SCALES`). Each row also carries a UI `default`
//! (mostly 0, but e.g. vignette midpoint/feather are 50) so sliders start where
//! the original UI does and `init_defaults` seeds the engine struct to match.

use gtk::prelude::*;
use relm4::{ComponentSender, RelmWidgetExt};

use crate::colorwheel::ColorWheel;
use crate::curves::CurveEditor;
use crate::slider::{slider, Track};
use crate::{AppModel, AppMsg};
use rapidraw_core::image_processing::GlobalAdjustments;

type Setter = fn(&mut GlobalAdjustments, f32);
/// `(label, min, max, step, scale, default, setter)`.
type Row = (&'static str, f64, f64, f64, f64, f64, Setter);

// Exposure lives in the Tone Mapper card (see `build_basic`), so it is not in
// this table; the rest of the basic tones follow.
const BASIC: &[Row] = &[
    ("Contrast", -100.0, 100.0, 1.0, 100.0, 0.0, |g, v| g.contrast = v),
    ("Highlights", -100.0, 100.0, 1.0, 120.0, 0.0, |g, v| g.highlights = v),
    ("Shadows", -100.0, 100.0, 1.0, 120.0, 0.0, |g, v| g.shadows = v),
    ("Whites", -100.0, 100.0, 1.0, 30.0, 0.0, |g, v| g.whites = v),
    ("Blacks", -100.0, 100.0, 1.0, 70.0, 0.0, |g, v| g.blacks = v),
];

const DETAILS: &[Row] = &[
    ("Sharpness", -100.0, 100.0, 1.0, 50.0, 0.0, |g, v| g.sharpness = v),
    ("Sharpness Threshold", 0.0, 80.0, 1.0, 100.0, 15.0, |g, v| {
        g.sharpness_threshold = v
    }),
    ("Clarity", -100.0, 100.0, 1.0, 200.0, 0.0, |g, v| g.clarity = v),
    ("Dehaze", -100.0, 100.0, 1.0, 750.0, 0.0, |g, v| g.dehaze = v),
    ("Structure", -100.0, 100.0, 1.0, 200.0, 0.0, |g, v| g.structure = v),
    ("Centre", -100.0, 100.0, 1.0, 250.0, 0.0, |g, v| g.centré = v),
    ("Luminance NR", 0.0, 100.0, 1.0, 100.0, 0.0, |g, v| {
        g.luma_noise_reduction = v
    }),
    ("Color NR", 0.0, 100.0, 1.0, 100.0, 0.0, |g, v| {
        g.color_noise_reduction = v
    }),
    ("Chromatic Aberration R/C", -100.0, 100.0, 1.0, 10000.0, 0.0, |g, v| {
        g.chromatic_aberration_red_cyan = v
    }),
    ("Chromatic Aberration B/Y", -100.0, 100.0, 1.0, 10000.0, 0.0, |g, v| {
        g.chromatic_aberration_blue_yellow = v
    }),
];

const EFFECTS: &[Row] = &[
    ("Glow", 0.0, 100.0, 1.0, 100.0, 0.0, |g, v| g.glow_amount = v),
    ("Halation", 0.0, 100.0, 1.0, 100.0, 0.0, |g, v| g.halation_amount = v),
    ("Light Flares", 0.0, 100.0, 1.0, 100.0, 0.0, |g, v| g.flare_amount = v),
    ("Vignette Amount", -100.0, 100.0, 1.0, 100.0, 0.0, |g, v| {
        g.vignette_amount = v
    }),
    ("Vignette Midpoint", 0.0, 100.0, 1.0, 100.0, 50.0, |g, v| {
        g.vignette_midpoint = v
    }),
    ("Vignette Roundness", -100.0, 100.0, 1.0, 100.0, 0.0, |g, v| {
        g.vignette_roundness = v
    }),
    ("Vignette Feather", 0.0, 100.0, 1.0, 100.0, 50.0, |g, v| {
        g.vignette_feather = v
    }),
    ("Grain Amount", 0.0, 100.0, 1.0, 200.0, 0.0, |g, v| g.grain_amount = v),
    ("Grain Size", 0.0, 100.0, 1.0, 50.0, 25.0, |g, v| g.grain_size = v),
    ("Grain Roughness", 0.0, 100.0, 1.0, 100.0, 50.0, |g, v| {
        g.grain_roughness = v
    }),
];

const COLOR_WB: &[Row] = &[
    ("Temperature", -100.0, 100.0, 1.0, 25.0, 0.0, |g, v| g.temperature = v),
    ("Tint", -100.0, 100.0, 1.0, 100.0, 0.0, |g, v| g.tint = v),
];
const COLOR_PRESENCE: &[Row] = &[
    ("Vibrance", -100.0, 100.0, 1.0, 100.0, 0.0, |g, v| g.vibrance = v),
    ("Saturation", -100.0, 100.0, 1.0, 100.0, 0.0, |g, v| g.saturation = v),
    ("Hue", -180.0, 180.0, 1.0, 1.0, 0.0, |g, v| g.hue = v),
];

const HSL_HUE_SCALE: f64 = 1.0 / 0.3;
const HSL_BANDS: &[(&str, Setter, Setter, Setter)] = &[
    ("Reds", |g, v| g.hsl[0].hue = v, |g, v| g.hsl[0].saturation = v, |g, v| g.hsl[0].luminance = v),
    ("Oranges", |g, v| g.hsl[1].hue = v, |g, v| g.hsl[1].saturation = v, |g, v| g.hsl[1].luminance = v),
    ("Yellows", |g, v| g.hsl[2].hue = v, |g, v| g.hsl[2].saturation = v, |g, v| g.hsl[2].luminance = v),
    ("Greens", |g, v| g.hsl[3].hue = v, |g, v| g.hsl[3].saturation = v, |g, v| g.hsl[3].luminance = v),
    ("Aquas", |g, v| g.hsl[4].hue = v, |g, v| g.hsl[4].saturation = v, |g, v| g.hsl[4].luminance = v),
    ("Blues", |g, v| g.hsl[5].hue = v, |g, v| g.hsl[5].saturation = v, |g, v| g.hsl[5].luminance = v),
    ("Purples", |g, v| g.hsl[6].hue = v, |g, v| g.hsl[6].saturation = v, |g, v| g.hsl[6].luminance = v),
    ("Magentas", |g, v| g.hsl[7].hue = v, |g, v| g.hsl[7].saturation = v, |g, v| g.hsl[7].luminance = v),
];

const CALIBRATION: &[Row] = &[
    ("Shadows Tint", -100.0, 100.0, 1.0, 400.0, 0.0, |g, v| {
        g.color_calibration.shadows_tint = v
    }),
    ("Red Hue", -100.0, 100.0, 1.0, 400.0, 0.0, |g, v| {
        g.color_calibration.red_hue = v
    }),
    ("Red Saturation", -100.0, 100.0, 1.0, 120.0, 0.0, |g, v| {
        g.color_calibration.red_saturation = v
    }),
    ("Green Hue", -100.0, 100.0, 1.0, 400.0, 0.0, |g, v| {
        g.color_calibration.green_hue = v
    }),
    ("Green Saturation", -100.0, 100.0, 1.0, 120.0, 0.0, |g, v| {
        g.color_calibration.green_saturation = v
    }),
    ("Blue Hue", -100.0, 100.0, 1.0, 400.0, 0.0, |g, v| {
        g.color_calibration.blue_hue = v
    }),
    ("Blue Saturation", -100.0, 100.0, 1.0, 120.0, 0.0, |g, v| {
        g.color_calibration.blue_saturation = v
    }),
];

const GRADING_EXTRA: &[Row] = &[
    ("Blending", 0.0, 100.0, 1.0, 100.0, 50.0, |g, v| {
        g.color_grading_blending = v
    }),
    ("Balance", -100.0, 100.0, 1.0, 200.0, 0.0, |g, v| {
        g.color_grading_balance = v
    }),
];

/// Seed `g` with the UI defaults (most 0, but e.g. vignette feather/midpoint
/// are 50). Keeps the engine struct in sync with the slider start positions.
pub fn init_defaults(g: &mut GlobalAdjustments) {
    for table in [
        BASIC,
        DETAILS,
        EFFECTS,
        COLOR_WB,
        COLOR_PRESENCE,
        CALIBRATION,
        GRADING_EXTRA,
    ] {
        for &(_, _, _, _, scale, default, set) in table {
            set(g, (default / scale) as f32);
        }
    }
    // Tone mapper: default to "basic" (0); seed the AGX matrices so switching to
    // AgX works (the relm4 path builds GlobalAdjustments directly, not via JSON).
    g.tonemapper_mode = 0;
    let (pipe_to_rendering, rendering_to_pipe) =
        rapidraw_core::image_processing::agx_matrices();
    g.agx_pipe_to_rendering_matrix = pipe_to_rendering;
    g.agx_rendering_to_pipe_matrix = rendering_to_pipe;
}

pub struct AdjustPanel {
    root: gtk::ScrolledWindow,
    /// All slider handles, in build order, for undo/redo snapshot/restore.
    handles: Vec<crate::slider::SliderHandle>,
    /// Slider UI values at build time (defaults), for in-place reset on open.
    defaults: Vec<f64>,
    /// Reset closures for non-slider widgets (curve editor, colour wheels).
    reset_hooks: Vec<std::rc::Rc<dyn Fn()>>,
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

        crate::slider::reg_begin();
        let curves = CurveEditor::new(sender, &vadj);
        list.append(&card(&expander("Curves", curves.root(), true)));
        list.append(&card(&build_basic(sender, &vadj)));
        list.append(&card(&build_color(sender, &vadj)));
        list.append(&card(&section("Details", DETAILS, sender, &vadj)));
        list.append(&card(&section("Effects", EFFECTS, sender, &vadj)));
        list.append(&card(&build_lut_section(sender, &vadj)));
        let handles = crate::slider::reg_take();
        let reset_hooks = crate::slider::reset_take();
        let defaults = handles.iter().map(|h| h.get()).collect();

        Self {
            root,
            handles,
            defaults,
            reset_hooks,
        }
    }

    /// Snapshot every slider's current UI value (build order).
    pub fn snapshot(&self) -> Vec<f64> {
        self.handles.iter().map(|h| h.get()).collect()
    }

    /// Reset all controls (sliders, curve editor, colour wheels) to defaults,
    /// in place — no widget rebuild, so opening a new image stays fluid.
    pub fn reset(&self) {
        for h in &self.reset_hooks {
            h();
        }
        self.restore(&self.defaults);
    }

    /// Restore slider UI values from a [`snapshot`](Self::snapshot) (no change
    /// callbacks fire; the caller sets the engine state + renders).
    pub fn restore(&self, vals: &[f64]) {
        for (h, &v) in self.handles.iter().zip(vals) {
            h.set_ui(v);
        }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }
}

/// Wrap a section widget in a libadwaita `.card` so groups read as distinct
/// panels (like the default UI).
fn card(child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("card");
    b.set_margin_top(3);
    b.set_margin_bottom(3);
    child.as_ref().set_margin_start(8);
    child.as_ref().set_margin_end(8);
    child.as_ref().set_margin_top(6);
    child.as_ref().set_margin_bottom(6);
    b.append(child);
    b
}

fn expander(title: &str, child: &impl IsA<gtk::Widget>, expanded: bool) -> gtk::Expander {
    let e = gtk::Expander::new(Some(title));
    e.set_expanded(expanded);
    e.set_child(Some(child));
    e
}

fn subheader(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_halign(gtk::Align::Start);
    l.set_margin_top(8);
    l.add_css_class("heading");
    l
}

fn section(
    title: &str,
    rows: &[Row],
    sender: &ComponentSender<AppModel>,
    vadj: &gtk::Adjustment,
) -> gtk::Expander {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 2);
    body.set_margin_all(4);
    append_rows(&body, rows, sender, vadj);
    expander(title, &body, true)
}

/// Basic section: the Tone Mapper card (mapper choice + EV shift) on top, then
/// the remaining basic tone sliders.
fn build_basic(sender: &ComponentSender<AppModel>, vadj: &gtk::Adjustment) -> gtk::Expander {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 2);
    body.set_margin_all(4);
    body.append(&tone_mapper_card(sender, vadj));
    append_rows(&body, BASIC, sender, vadj);
    expander("Basic", &body, true)
}

/// Tone Mapper sub-card: a Basic/AgX segmented selector plus the EV-shift slider
/// (which is the exposure control), mirroring the original's inner card.
fn tone_mapper_card(sender: &ComponentSender<AppModel>, vadj: &gtk::Adjustment) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    outer.add_css_class("card");
    outer.set_margin_top(2);
    outer.set_margin_bottom(4);

    let title = gtk::Label::new(Some("Tone Mapper"));
    title.set_halign(gtk::Align::Start);
    title.add_css_class("caption");
    title.set_margin_start(6);
    title.set_margin_top(4);
    outer.append(&title);

    // Segmented Basic / AgX selector (linked toggle buttons in one group).
    let seg = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    seg.add_css_class("linked");
    seg.set_margin_start(6);
    seg.set_margin_end(6);
    let basic_btn = gtk::ToggleButton::with_label("Basic");
    let agx_btn = gtk::ToggleButton::with_label("AgX");
    agx_btn.set_group(Some(&basic_btn));
    basic_btn.set_active(true); // default mapper
    basic_btn.set_hexpand(true);
    agx_btn.set_hexpand(true);
    {
        let sender = sender.clone();
        // Send on the basic button's toggle (the group fires it for both).
        basic_btn.connect_toggled(move |b| {
            let mode = if b.is_active() { 0.0 } else { 1.0 };
            sender.input(AppMsg::Adjust(crate::Adjust {
                set: |g, v| g.tonemapper_mode = v as u32,
                value: mode,
            }));
        });
    }
    seg.append(&basic_btn);
    seg.append(&agx_btn);
    outer.append(&seg);

    // New-image reset returns the selector to Basic (matches init_defaults).
    {
        let basic_btn = basic_btn.clone();
        crate::slider::register_reset(std::rc::Rc::new(move || basic_btn.set_active(true)));
    }

    // EV shift = exposure (-5..5), same as the original's card slider.
    outer.append(&build_row(
        "EV Shift",
        -5.0,
        5.0,
        0.01,
        0.8,
        0.0,
        |g, v| g.exposure = v,
        Track::Plain,
        sender,
        vadj,
    ));
    outer
}

fn append_rows(
    body: &gtk::Box,
    rows: &[Row],
    sender: &ComponentSender<AppModel>,
    vadj: &gtk::Adjustment,
) {
    for &(label, min, max, step, scale, default, set) in rows {
        body.append(&build_row(
            label, min, max, step, scale, default, set, Track::Plain, sender, vadj,
        ));
    }
}

fn build_color(sender: &ComponentSender<AppModel>, vadj: &gtk::Adjustment) -> gtk::Expander {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 2);
    body.set_margin_all(4);

    body.append(&subheader("White Balance"));
    for &(label, min, max, step, scale, default, set) in COLOR_WB {
        let track = match label {
            "Temperature" => Track::Temperature,
            "Tint" => Track::Tint,
            _ => Track::Plain,
        };
        body.append(&build_row(
            label, min, max, step, scale, default, set, track, sender, vadj,
        ));
    }

    body.append(&subheader("Presence"));
    for &(label, min, max, step, scale, default, set) in COLOR_PRESENCE {
        let track = if label == "Hue" { Track::Hue } else { Track::Plain };
        body.append(&build_row(
            label, min, max, step, scale, default, set, track, sender, vadj,
        ));
    }

    body.append(&subheader("Color Grading"));
    body.append(&build_grading(sender, vadj));

    body.append(&subheader("HSL"));
    body.append(&build_hsl(sender, vadj));

    body.append(&subheader("Calibration"));
    append_rows(&body, CALIBRATION, sender, vadj);

    expander("Color", &body, true)
}

fn build_grading(sender: &ComponentSender<AppModel>, vadj: &gtk::Adjustment) -> gtk::Box {
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 4);

    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_column_spacing(4);
    flow.set_row_spacing(4);
    flow.set_homogeneous(true);

    let wheels = [
        (
            "Shadows",
            (|g: &mut GlobalAdjustments, v| g.color_grading_shadows.hue = v) as Setter,
            (|g: &mut GlobalAdjustments, v| g.color_grading_shadows.saturation = v) as Setter,
            (|g: &mut GlobalAdjustments, v| g.color_grading_shadows.luminance = v) as Setter,
        ),
        (
            "Midtones",
            (|g, v| g.color_grading_midtones.hue = v) as Setter,
            (|g, v| g.color_grading_midtones.saturation = v) as Setter,
            (|g, v| g.color_grading_midtones.luminance = v) as Setter,
        ),
        (
            "Highlights",
            (|g, v| g.color_grading_highlights.hue = v) as Setter,
            (|g, v| g.color_grading_highlights.saturation = v) as Setter,
            (|g, v| g.color_grading_highlights.luminance = v) as Setter,
        ),
        (
            "Global",
            (|g, v| g.color_grading_global.hue = v) as Setter,
            (|g, v| g.color_grading_global.saturation = v) as Setter,
            (|g, v| g.color_grading_global.luminance = v) as Setter,
        ),
    ];
    let built: Vec<ColorWheel> = wheels
        .into_iter()
        .map(|(name, h, s, l)| {
            let w = ColorWheel::new(name, sender, vadj, h, s, l);
            flow.append(w.root());
            w
        })
        .collect();

    // "Toggle sliders": reveal the Hue/Saturation sliders on every wheel at once
    // (mirrors the original's Sliders button; luminance is always shown).
    let toggle = gtk::ToggleButton::new();
    toggle.set_icon_name("view-list-symbolic");
    toggle.add_css_class("flat");
    toggle.set_halign(gtk::Align::End);
    toggle.set_tooltip_text(Some("Toggle hue/saturation sliders"));
    {
        let built = built.clone();
        toggle.connect_toggled(move |b| {
            for w in &built {
                w.set_sliders_visible(b.is_active());
            }
        });
    }

    wrap.append(&toggle);
    wrap.append(&flow);
    append_rows(&wrap, GRADING_EXTRA, sender, vadj);
    wrap
}

/// Band centre hue (deg), matching `src/styles.css` HSL mixer gradients, in the
/// same order as `HSL_BANDS`.
const HSL_CENTERS: [f64; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 300.0, 340.0];

/// HSL mixer: a row of clickable colour swatches (one per band) selecting which
/// band the Hue/Sat/Lum sliders below edit — mirroring the original UI. The
/// slider set is rebuilt on band switch (so its gradients/setters track the
/// active band); per-band values are kept in a small store.
fn build_hsl(sender: &ComponentSender<AppModel>, vadj: &gtk::Adjustment) -> gtk::Box {
    use crate::slider::slider_ex;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    install_hsl_css();
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 6);
    wrap.set_margin_all(4);

    // Per-band [hue, sat, lum] UI values (all start at 0).
    let store = Rc::new(RefCell::new([[0.0_f64; 3]; 8]));
    let active = Rc::new(Cell::new(0usize));

    let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    swatches.set_halign(gtk::Align::Center);
    let container = gtk::Box::new(gtk::Orientation::Vertical, 2);

    // Rebuild the 3 sliders for `band`, seeded from the store.
    let rebuild: Rc<dyn Fn(usize)> = {
        let container = container.clone();
        let store = store.clone();
        let sender = sender.clone();
        let vadj = vadj.clone();
        Rc::new(move |band: usize| {
            while let Some(c) = container.first_child() {
                container.remove(&c);
            }
            let center = HSL_CENTERS[band];
            let (_, hue_set, sat_set, lum_set) = HSL_BANDS[band];
            let cur = store.borrow()[band];
            let hue_cell = Rc::new(Cell::new(cur[0]));
            let sat_cell = Rc::new(Cell::new(cur[1]));

            let emit = |set: Setter, scale: f64, s: ComponentSender<AppModel>| {
                move |v: f64| {
                    s.input(AppMsg::Adjust(crate::Adjust { set, value: (v / scale) as f32 }))
                }
            };

            // Build lum, then sat, then hue, so each can redraw later siblings.
            let (lum_box, lum_area, lum_h) = {
                let store = store.clone();
                let e = emit(lum_set, 100.0, sender.clone());
                slider_ex(
                    "Luminance", -100.0, 100.0, 1.0, 0.0,
                    Track::HslLum { base: center, hue: hue_cell.clone(), sat: sat_cell.clone() },
                    &vadj,
                    move |v| { store.borrow_mut()[band][2] = v; e(v); },
                )
            };
            let (sat_box, sat_area, sat_h) = {
                let store = store.clone();
                let sat_cell = sat_cell.clone();
                let lum_area = lum_area.clone();
                let e = emit(sat_set, 100.0, sender.clone());
                slider_ex(
                    "Saturation", -100.0, 100.0, 1.0, 0.0,
                    Track::HslSat { base: center, hue: hue_cell.clone() },
                    &vadj,
                    move |v| { store.borrow_mut()[band][1] = v; sat_cell.set(v); lum_area.queue_draw(); e(v); },
                )
            };
            let (hue_box, _, hue_h) = {
                let store = store.clone();
                let hue_cell = hue_cell.clone();
                let sat_area = sat_area.clone();
                let lum_area = lum_area.clone();
                let e = emit(hue_set, HSL_HUE_SCALE, sender.clone());
                slider_ex(
                    "Hue", -100.0, 100.0, 1.0, 0.0, Track::HslHue(center), &vadj,
                    move |v| { store.borrow_mut()[band][0] = v; hue_cell.set(v); sat_area.queue_draw(); lum_area.queue_draw(); e(v); },
                )
            };
            // Show stored values without firing change callbacks.
            hue_h.set_ui(cur[0]);
            sat_h.set_ui(cur[1]);
            lum_h.set_ui(cur[2]);

            container.append(&hue_box);
            container.append(&sat_box);
            container.append(&lum_box);
        })
    };

    let mut group: Option<gtk::ToggleButton> = None;
    for (i, &(band, _, _, _)) in HSL_BANDS.iter().enumerate() {
        let b = gtk::ToggleButton::new();
        b.add_css_class("circular");
        b.add_css_class(&format!("hsl-swatch-{i}"));
        b.set_tooltip_text(Some(band));
        match &group {
            Some(g) => b.set_group(Some(g)),
            None => group = Some(b.clone()),
        }
        if i == 0 {
            b.set_active(true);
        }
        let active = active.clone();
        let rebuild = rebuild.clone();
        b.connect_toggled(move |b| {
            if b.is_active() {
                active.set(i);
                rebuild(i);
            }
        });
        swatches.append(&b);
    }

    rebuild(0);
    wrap.append(&swatches);
    wrap.append(&container);
    wrap
}

/// One-time CSS for the 8 HSL band swatches (a coloured circle per band).
fn install_hsl_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mut css = String::new();
        for (i, &center) in HSL_CENTERS.iter().enumerate() {
            let (r, g, b) = hsl_to_rgb(center, 0.75, 0.5);
            css.push_str(&format!(
                ".hsl-swatch-{i} {{ background-image: none; background-color: rgb({r},{g},{b}); \
                 min-width: 22px; min-height: 22px; padding: 0; }} \
                 .hsl-swatch-{i}:checked {{ box-shadow: inset 0 0 0 2px white; }} "
            ));
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_data(&css);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

/// HSL (deg, 0..1, 0..1) -> sRGB u8 triple, for the swatch colours.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_row(
    label: &str,
    min: f64,
    max: f64,
    step: f64,
    scale: f64,
    default: f64,
    set: Setter,
    track: Track,
    sender: &ComponentSender<AppModel>,
    vadj: &gtk::Adjustment,
) -> gtk::Box {
    let sender = sender.clone();
    slider(label, min, max, step, default, track, vadj, move |v| {
        sender.input(AppMsg::Adjust(crate::Adjust {
            set,
            value: (v / scale) as f32,
        }));
    })
}

/// Make a widget's mouse wheel scroll the panel (`vadj`) instead of changing
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

fn build_lut_section(sender: &ComponentSender<AppModel>, vadj: &gtk::Adjustment) -> gtk::Expander {
    let lut_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    lut_box.set_margin_all(6);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let load = gtk::Button::with_label("Load .cube");
    let clear = gtk::Button::with_label("Clear");
    let export = gtk::Button::with_label("Export look as .cube");
    {
        let sender = sender.clone();
        load.connect_clicked(move |_| sender.input(AppMsg::LoadLut));
    }
    {
        let sender = sender.clone();
        clear.connect_clicked(move |_| sender.input(AppMsg::ClearLut));
    }
    {
        let sender = sender.clone();
        export.connect_clicked(move |_| sender.input(AppMsg::ExportLutDialog));
    }
    buttons.append(&load);
    buttons.append(&clear);
    lut_box.append(&buttons);
    lut_box.append(&export);

    let lbl = gtk::Label::new(Some("Intensity"));
    lbl.set_halign(gtk::Align::Start);
    lbl.add_css_class("caption");

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    scale.set_value(100.0);
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
    // Double-click resets intensity to its default (100%).
    let reset = gtk::GestureClick::new();
    {
        let scale = scale.clone();
        reset.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                scale.set_value(100.0);
            }
        });
    }
    scale.add_controller(reset);
    lut_box.append(&lbl);
    lut_box.append(&scale);

    expander("LUT", &lut_box, true)
}
