//! Right-rail "Masks" panel (P2): a list of mask containers plus the selected
//! mask's scalar adjustments. Mirrors the default UI's Masks tab.
//!
//! Geometry of a sub-mask is NOT editable here yet (no canvas) — a freshly
//! added radial/linear renders with the same centred defaults the React app
//! seeds via `createSubMask` (`src/utils/maskUtils.ts`), so the pipeline is
//! provable end to end. Brush/Flow/Color/Luminance start empty (canvas/colour
//! picking land in P3/P4).
//!
//! The list is dynamic, so unlike `AdjustPanel` it is rebuilt (`rebuild`) after
//! every mask mutation rather than wired once.

use std::sync::atomic::{AtomicU64, Ordering};

use adw::prelude::*;
use relm4::{ComponentSender, RelmWidgetExt};
use serde_json::{json, Value};

use rapidraw_core::mask_generation::{MaskDefinition, SubMask, SubMaskMode};

use crate::slider::{slider_ex, Track};
use crate::{AppModel, AppMsg};

/// Non-AI mask types offered by the "Add" menu: `(label, type-string)`. The
/// type string is the camelCase `SubMask.type` the engine dispatches on. AI
/// types (ai-subject/foreground/sky/depth, quick-eraser) are deferred to P5.
pub const MASK_TYPES: &[(&str, &str)] = &[
    ("Radial", "radial"),
    ("Linear", "linear"),
    ("Brush", "brush"),
    ("Flow", "flow"),
    ("Color", "color"),
    ("Luminance", "luminance"),
    ("All", "all"),
];

/// Per-mask scalar adjustments: `(label, json-key, min, max, step, default)`.
/// Values are stored raw (UI units) in the mask's `adjustments` JSON; the engine
/// divides by `image_processing::SCALES` in `get_mask_adjustments_from_json`, so
/// ranges mirror the global `controls.rs` rows. Curves/HSL/colour-grading are
/// deferred to a later increment.
type AdjRow = (&'static str, &'static str, f64, f64, f64, f64);
const MASK_ADJ: &[AdjRow] = &[
    // Basic
    ("Exposure", "exposure", -5.0, 5.0, 0.01, 0.0),
    ("Brightness", "brightness", -100.0, 100.0, 1.0, 0.0),
    ("Contrast", "contrast", -100.0, 100.0, 1.0, 0.0),
    ("Highlights", "highlights", -100.0, 100.0, 1.0, 0.0),
    ("Shadows", "shadows", -100.0, 100.0, 1.0, 0.0),
    ("Whites", "whites", -100.0, 100.0, 1.0, 0.0),
    ("Blacks", "blacks", -100.0, 100.0, 1.0, 0.0),
    // Color
    ("Temperature", "temperature", -100.0, 100.0, 1.0, 0.0),
    ("Tint", "tint", -100.0, 100.0, 1.0, 0.0),
    ("Vibrance", "vibrance", -100.0, 100.0, 1.0, 0.0),
    ("Saturation", "saturation", -100.0, 100.0, 1.0, 0.0),
    ("Hue", "hue", -180.0, 180.0, 1.0, 0.0),
    // Details
    ("Sharpness", "sharpness", -100.0, 100.0, 1.0, 0.0),
    ("Sharpness Threshold", "sharpnessThreshold", 0.0, 80.0, 1.0, 15.0),
    ("Clarity", "clarity", -100.0, 100.0, 1.0, 0.0),
    ("Dehaze", "dehaze", -100.0, 100.0, 1.0, 0.0),
    ("Structure", "structure", -100.0, 100.0, 1.0, 0.0),
    ("Luminance NR", "lumaNoiseReduction", 0.0, 100.0, 1.0, 0.0),
    ("Color NR", "colorNoiseReduction", 0.0, 100.0, 1.0, 0.0),
    // Effects
    ("Glow", "glowAmount", 0.0, 100.0, 1.0, 0.0),
    ("Halation", "halationAmount", 0.0, 100.0, 1.0, 0.0),
    ("Light Flares", "flareAmount", 0.0, 100.0, 1.0, 0.0),
];

/// Per-sub-mask geometry field: `(label, json-key, min, max, step, digits, mult,
/// default)`. Ranges/`default` are in DISPLAY units matching the React UI
/// (`SUB_MASK_CONFIG` in `MasksPanel.tsx`); the stored JSON value is `display /
/// mult` (e.g. radial feather shows 0..100 default 50 but stores 0.5). Coordinate
/// rows (center/radius/target/endpoints) are a numeric fallback the React UI
/// places on the canvas instead — the canvas lands in P4.
type GeoRow = (&'static str, &'static str, f64, f64, f64, u32, f64, f64);

const GEO_RADIAL: &[GeoRow] = &[
    ("Center X", "centerX", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("Center Y", "centerY", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("Radius X", "radiusX", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("Radius Y", "radiusY", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("Rotation", "rotation", -180.0, 180.0, 1.0, 0, 1.0, 0.0),
    // React: 0..100 default 50, multiplier 100 -> stored 0.5.
    ("Feather", "feather", 0.0, 100.0, 1.0, 0, 100.0, 50.0),
];
const GEO_LINEAR: &[GeoRow] = &[
    ("Start X", "startX", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("Start Y", "startY", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("End X", "endX", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("End Y", "endY", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("Range", "range", 0.0, 100.0, 1.0, 0, 1.0, 50.0),
];
/// Color + Luminance both use `ParametricMaskParameters` (React defaults:
/// tolerance 1..100 = 20, grow -100..100 = 0, feather 0..100 = 35). targetX/Y are
/// a numeric fallback (React picks them on the canvas).
const GEO_PARAMETRIC: &[GeoRow] = &[
    ("Target X", "targetX", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("Target Y", "targetY", 0.0, 100_000.0, 1.0, 0, 1.0, 0.0),
    ("Tolerance", "tolerance", 1.0, 100.0, 1.0, 0, 1.0, 20.0),
    ("Grow", "grow", -100.0, 100.0, 1.0, 0, 1.0, 0.0),
    ("Feather", "feather", 0.0, 100.0, 1.0, 0, 1.0, 35.0),
];

fn geo_rows(mask_type: &str) -> &'static [GeoRow] {
    match mask_type {
        "radial" => GEO_RADIAL,
        "linear" => GEO_LINEAR,
        "color" | "luminance" => GEO_PARAMETRIC,
        _ => &[], // brush/flow (canvas, P4), all (no geometry)
    }
}

fn mode_index(m: SubMaskMode) -> u32 {
    match m {
        SubMaskMode::Additive => 0,
        SubMaskMode::Subtractive => 1,
        SubMaskMode::Intersect => 2,
    }
}

pub fn mode_from_index(i: u32) -> SubMaskMode {
    match i {
        1 => SubMaskMode::Subtractive,
        2 => SubMaskMode::Intersect,
        _ => SubMaskMode::Additive,
    }
}

static MASK_ID: AtomicU64 = AtomicU64::new(0);

fn next_id(prefix: &str) -> String {
    format!("{prefix}-{}", MASK_ID.fetch_add(1, Ordering::Relaxed))
}

/// Seed a new mask container's `adjustments` from the [`MASK_ADJ`] defaults so
/// every slider starts where the JSON says (no slider/engine drift, e.g.
/// sharpnessThreshold = 15).
fn default_adjustments() -> Value {
    let mut o = serde_json::Map::new();
    for &(_, key, _, _, _, default) in MASK_ADJ {
        o.insert(key.to_string(), json!(default));
    }
    Value::Object(o)
}

/// Default sub-mask parameters for `type`, matching `createSubMask` in the React
/// app. Geometry is in full-resolution image pixels (the engine scales it to the
/// render size). `(w, h)` is the full image size.
fn default_sub_params(mask_type: &str, w: f32, h: f32) -> Value {
    match mask_type {
        "radial" => json!({
            "centerX": w / 2.0, "centerY": h / 2.0,
            "radiusX": w / 4.0, "radiusY": w / 4.0,
            "rotation": 0.0, "feather": 0.5,
        }),
        "linear" => json!({
            "startX": w * 0.25, "startY": h / 2.0,
            "endX": w * 0.75, "endY": h / 2.0, "range": 50.0,
        }),
        "flow" => json!({ "lines": [], "flow": 10.0 }),
        "brush" => json!({ "lines": [] }),
        // color/luminance/all: serde defaults; these need canvas/colour input
        // (P3/P4) to produce a non-empty mask.
        _ => json!({}),
    }
}

/// Build a new mask container with one sub-mask of `mask_type`, mirroring the
/// React `handleAddMaskContainer` flow. `(w, h)` is the full image size for the
/// sub-mask's default geometry.
pub fn new_mask(label: &str, mask_type: &str, w: f32, h: f32) -> MaskDefinition {
    MaskDefinition {
        id: next_id("mask"),
        name: label.to_string(),
        visible: true,
        invert: false,
        opacity: 100.0,
        adjustments: default_adjustments(),
        sub_masks: vec![SubMask {
            id: next_id("sub"),
            mask_type: mask_type.to_string(),
            visible: true,
            invert: false,
            opacity: 100.0,
            mode: SubMaskMode::Additive,
            parameters: default_sub_params(mask_type, w, h),
        }],
    }
}

pub struct MasksPanel {
    root: gtk::ScrolledWindow,
    /// Mask list (one row per container) + selected mask's controls below.
    body: gtk::Box,
    vadj: gtk::Adjustment,
}

impl MasksPanel {
    pub fn new(sender: &ComponentSender<AppModel>) -> Self {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        body.set_margin_all(6);

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&body));
        root.set_hexpand(false);
        root.set_vexpand(true);
        root.set_width_request(320);
        let vadj = root.vadjustment();

        let panel = Self { root, body, vadj };
        panel.rebuild(&[], None, sender);
        panel
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Clear and repopulate the list + the selected mask's controls. Called
    /// after every mask mutation (add/delete/select/toggle).
    pub fn rebuild(
        &self,
        masks: &[MaskDefinition],
        selected: Option<usize>,
        sender: &ComponentSender<AppModel>,
    ) {
        while let Some(c) = self.body.first_child() {
            self.body.remove(&c);
        }

        self.body.append(&add_menu(sender));

        if masks.is_empty() {
            let hint = gtk::Label::new(Some("No masks. Add one above."));
            hint.add_css_class("dim-label");
            hint.set_margin_top(8);
            self.body.append(&hint);
            return;
        }

        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        list.add_css_class("card");
        list.set_margin_top(4);
        for (i, m) in masks.iter().enumerate() {
            list.append(&mask_row(i, m, selected == Some(i), sender));
        }
        self.body.append(&list);

        if let Some(i) = selected {
            if let Some(m) = masks.get(i) {
                self.body.append(&mask_details(i, m, &self.vadj, sender));
            }
        }
    }
}

/// The "Add mask" menu button (popover of non-AI types).
fn add_menu(sender: &ComponentSender<AppModel>) -> gtk::MenuButton {
    let btn = gtk::MenuButton::new();
    btn.set_label("Add mask");
    btn.add_css_class("flat");

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.set_margin_all(4);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&list));
    for &(label, ty) in MASK_TYPES {
        let item = gtk::Button::with_label(label);
        item.add_css_class("flat");
        item.set_halign(gtk::Align::Fill);
        let sender = sender.clone();
        let pop = pop.clone();
        item.connect_clicked(move |_| {
            pop.popdown();
            sender.input(AppMsg::AddMask(ty));
        });
        list.append(&item);
    }
    btn.set_popover(Some(&pop));
    btn
}

/// One mask-list row: visibility toggle | name (selects) | delete.
fn mask_row(
    i: usize,
    m: &MaskDefinition,
    is_selected: bool,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_margin_all(2);

    let eye = gtk::ToggleButton::new();
    eye.set_icon_name(if m.visible {
        "display-brightness-symbolic"
    } else {
        "weather-clear-night-symbolic"
    });
    eye.set_active(m.visible);
    eye.add_css_class("flat");
    eye.set_tooltip_text(Some("Toggle visibility"));
    {
        let sender = sender.clone();
        eye.connect_clicked(move |_| sender.input(AppMsg::ToggleMaskVisible(i)));
    }
    row.append(&eye);

    let name = gtk::Button::with_label(&m.name);
    name.add_css_class("flat");
    name.set_hexpand(true);
    name.set_halign(gtk::Align::Fill);
    if is_selected {
        name.add_css_class("suggested-action");
    }
    {
        let sender = sender.clone();
        name.connect_clicked(move |_| {
            sender.input(AppMsg::SelectMask(if is_selected { None } else { Some(i) }))
        });
    }
    row.append(&name);

    let del = gtk::Button::from_icon_name("user-trash-symbolic");
    del.add_css_class("flat");
    del.set_tooltip_text(Some("Delete mask"));
    {
        let sender = sender.clone();
        del.connect_clicked(move |_| sender.input(AppMsg::DeleteMask(i)));
    }
    row.append(&del);
    row
}

/// The selected mask's controls: invert + opacity, then the scalar adjustments.
fn mask_details(
    i: usize,
    m: &MaskDefinition,
    vadj: &gtk::Adjustment,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.add_css_class("card");
    card.set_margin_top(6);
    card.set_margin_bottom(4);
    card.set_margin_start(2);
    card.set_margin_end(2);

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.set_margin_all(6);
    let invert = gtk::CheckButton::with_label("Invert");
    invert.set_active(m.invert);
    {
        let sender = sender.clone();
        invert.connect_toggled(move |_| sender.input(AppMsg::ToggleMaskInvert(i)));
    }
    head.append(&invert);
    card.append(&head);

    // Opacity (0..100).
    let (op_row, _, op_h) = slider_ex(
        "Opacity", 0.0, 100.0, 1.0, 100.0, Track::Plain, vadj,
        {
            let sender = sender.clone();
            move |v| sender.input(AppMsg::SetMaskOpacity(i, v))
        },
    );
    op_h.set_ui(m.opacity as f64);
    op_row.set_margin_start(6);
    op_row.set_margin_end(6);
    card.append(&op_row);

    // Sub-mask geometry + compositing mode (one group per sub-mask).
    for (si, sm) in m.sub_masks.iter().enumerate() {
        card.append(&submask_editor(i, si, sm, sender));
    }

    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.set_margin_top(4);
    card.append(&sep);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 2);
    body.set_margin_all(6);
    for &(label, key, min, max, step, default) in MASK_ADJ {
        let cur = m.adjustments.get(key).and_then(Value::as_f64).unwrap_or(default);
        let (sl, _, h) = slider_ex(label, min, max, step, default, Track::Plain, vadj, {
            let sender = sender.clone();
            move |v| sender.input(AppMsg::MaskAdjust { index: i, key, value: v })
        });
        h.set_ui(cur);
        body.append(&sl);
    }
    card.append(&body);
    card
}

/// Geometry + compositing-mode editor for one sub-mask (libadwaita rows). Brush/
/// flow show a canvas hint (P4); "all" has no geometry.
fn submask_editor(
    mask_i: usize,
    sub_i: usize,
    sm: &SubMask,
    sender: &ComponentSender<AppModel>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(&pretty_type(&sm.mask_type));
    group.set_margin_start(6);
    group.set_margin_end(6);
    group.set_margin_top(4);

    // Compositing mode.
    let mode = adw::ComboRow::new();
    mode.set_title("Mode");
    mode.set_model(Some(&gtk::StringList::new(&[
        "Additive",
        "Subtractive",
        "Intersect",
    ])));
    mode.set_selected(mode_index(sm.mode));
    {
        let sender = sender.clone();
        mode.connect_selected_notify(move |r| {
            sender.input(AppMsg::SetSubMaskMode {
                mask: mask_i,
                sub: sub_i,
                mode: r.selected(),
            });
        });
    }
    group.add(&mode);

    let rows = geo_rows(&sm.mask_type);
    if rows.is_empty() {
        let hint = adw::ActionRow::new();
        hint.set_title(if matches!(sm.mask_type.as_str(), "brush" | "flow") {
            "Paint on canvas (coming soon)"
        } else {
            "No geometry"
        });
        hint.add_css_class("dim-label");
        group.add(&hint);
        return group;
    }

    for &(label, key, min, max, step, digits, mult, default) in rows {
        // `default`/ranges are display units; JSON stores `display / mult`.
        let stored_default = default / mult;
        let stored = sm
            .parameters
            .get(key)
            .and_then(Value::as_f64)
            .unwrap_or(stored_default);
        let row = adw::SpinRow::with_range(min, max, step);
        row.set_title(label);
        row.set_digits(digits);
        row.set_value(stored * mult);
        // Connect AFTER set_value so the initial seed doesn't emit a change.
        let sender = sender.clone();
        row.connect_changed(move |r| {
            sender.input(AppMsg::SetSubMaskParam {
                mask: mask_i,
                sub: sub_i,
                key,
                value: r.value() / mult,
            });
        });
        group.add(&row);
    }
    group
}

/// Title-case a mask type string for display (e.g. "color" -> "Color").
fn pretty_type(ty: &str) -> String {
    MASK_TYPES
        .iter()
        .find(|(_, t)| *t == ty)
        .map(|(l, _)| l.to_string())
        .unwrap_or_else(|| ty.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_radial_mask_has_centred_default_geometry() {
        let m = new_mask("Radial", "radial", 1000.0, 800.0);
        assert_eq!(m.sub_masks.len(), 1);
        let p = &m.sub_masks[0].parameters;
        assert_eq!(p["centerX"], json!(500.0));
        assert_eq!(p["centerY"], json!(400.0));
        assert_eq!(p["radiusX"], json!(250.0));
        // adjustments seeded so sliders match JSON (no drift)
        assert_eq!(m.adjustments["sharpnessThreshold"], json!(15.0));
        assert_eq!(m.adjustments["exposure"], json!(0.0));
    }

    #[test]
    fn geometry_defaults_match_react_ui() {
        // Radial feather: UI shows 0..100 default 50, multiplier 100 -> stored 0.5.
        let feather = GEO_RADIAL.iter().find(|r| r.1 == "feather").unwrap();
        let (_, _, min, max, _, _, mult, default) = *feather;
        assert_eq!((min, max, mult, default), (0.0, 100.0, 100.0, 50.0));
        assert_eq!(default / mult, 0.5); // stored value == createSubMask seed

        // Parametric tolerance: React min is 1 (not 0), default 20.
        let tol = GEO_PARAMETRIC.iter().find(|r| r.1 == "tolerance").unwrap();
        assert_eq!((tol.2, tol.7), (1.0, 20.0));
        // Parametric feather default 35 (core ParametricMaskParameters default).
        let pf = GEO_PARAMETRIC.iter().find(|r| r.1 == "feather").unwrap();
        assert_eq!(pf.7, 35.0);
    }
}
