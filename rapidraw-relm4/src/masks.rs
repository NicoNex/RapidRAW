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

/// Mask types offered by the "Add" menu: `(label, type-string)`. The type
/// string is the camelCase `SubMask.type` the engine dispatches on. AI types
/// run ONNX models via [`crate::ai_masks`]; their mask is generated on demand.
pub const MASK_TYPES: &[(&str, &str)] = &[
    ("Radial", "radial"),
    ("Linear", "linear"),
    ("Brush", "brush"),
    ("Flow", "flow"),
    ("Color", "color"),
    ("Luminance", "luminance"),
    ("All", "all"),
    ("AI Subject", "ai-subject"),
    ("AI Foreground", "ai-foreground"),
    ("AI Sky", "ai-sky"),
    ("AI Depth", "ai-depth"),
];

/// Primary masks-panel create cards, mirroring Tauri `MASK_PANEL_CREATION_TYPES`.
/// The "Others" card is appended by `create_grid` (it has no single type).
pub const MASK_CREATE_GRID: &[(&str, &str)] = &[
    ("Subject", "ai-subject"),
    ("Sky", "ai-sky"),
    ("Foreground", "ai-foreground"),
    ("Linear", "linear"),
    ("Radial", "radial"),
];

/// Secondary types shown in the "Others" popover (Tauri `OTHERS_MASK_TYPES`).
pub const OTHERS_TYPES: &[(&str, &str)] = &[
    ("Depth", "ai-depth"),
    ("Color", "color"),
    ("Luminance", "luminance"),
    ("Brush", "brush"),
    ("Flow", "flow"),
    ("Whole Image", "all"),
];

/// relm4-icon name for a mask type's create card / row, or None for label-only.
pub fn mask_icon(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "ai-subject" | "luminance" => "sparkle-regular",
        "ai-sky" => "cloud-regular",
        "ai-foreground" => "person-regular",
        "linear" => "line-horizontal-4-regular",
        "radial" | "color" => "circle-regular",
        "brush" | "flow" => "paint-brush-regular",
        "ai-depth" => "layer-diagonal-regular",
        "all" => "crop-regular",
        "quick-eraser" => "eraser",
        _ => return None,
    })
}

/// True for mask types whose bitmap comes from an ONNX model.
pub fn is_ai_type(t: &str) -> bool {
    matches!(t, "ai-subject" | "ai-foreground" | "ai-sky" | "ai-depth" | "quick-eraser")
}

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
        // AI types: empty mask until generated; grow/feather refine the result.
        "ai-subject" | "ai-foreground" | "quick-eraser" => {
            json!({ "maskDataBase64": null, "grow": 0.0, "feather": 0.0 })
        }
        "ai-sky" => json!({ "maskDataBase64": null, "grow": 0.0, "feather": 0.0 }),
        "ai-depth" => json!({
            "maskDataBase64": null,
            "minDepth": 20.0, "maxDepth": 100.0,
            "minFade": 15.0, "maxFade": 15.0,
            "grow": 0.0, "feather": 15.0,
        }),
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

/// Deep-clone a mask container with fresh ids (container + every sub-mask),
/// mirroring Tauri `cloneMaskContainerData`. `invert` flips the container's
/// invert flag (for "Duplicate & Invert").
pub fn clone_mask(m: &MaskDefinition, invert: bool) -> MaskDefinition {
    let mut c = m.clone();
    c.id = next_id("mask");
    for sm in &mut c.sub_masks {
        sm.id = next_id("sub");
    }
    if invert {
        c.invert = !c.invert;
    }
    c
}

/// Normalized (0..1) drawable shapes for a mask's visible radial/linear
/// sub-masks, for the canvas overlay. `(w, h)` is the full image size (params are
/// full-res pixels). Brush/flow/color/luminance/all have no drawable shape.
pub fn overlay_shapes(sub_masks: &[SubMask], w: f64, h: f64) -> Vec<crate::editor::MaskShape> {
    use crate::editor::MaskShape;
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let g = |p: &Value, k: &str| p.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    sub_masks
        .iter()
        .enumerate()
        .filter(|(_, sm)| sm.visible)
        .filter_map(|(sub, sm)| {
            let p = &sm.parameters;
            match sm.mask_type.as_str() {
                "radial" => Some(MaskShape::Radial {
                    sub,
                    cx: g(p, "centerX") / w,
                    cy: g(p, "centerY") / h,
                    rx: g(p, "radiusX") / w,
                    ry: g(p, "radiusY") / h,
                    rot: g(p, "rotation"),
                }),
                "linear" => Some(MaskShape::Linear {
                    sub,
                    x1: g(p, "startX") / w,
                    y1: g(p, "startY") / h,
                    x2: g(p, "endX") / w,
                    y2: g(p, "endY") / h,
                }),
                _ => None,
            }
        })
        .collect()
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

        // Hovering the editing controls hides the coverage overlay (so the value
        // changes are visible); leaving the panel shows it again. Matches the
        // original's behaviour.
        let motion = gtk::EventControllerMotion::new();
        {
            let sender = sender.clone();
            motion.connect_enter(move |_, _, _| {
                sender.input(AppMsg::SetMaskOverlayShown(false));
            });
        }
        {
            let sender = sender.clone();
            motion.connect_leave(move |_| {
                sender.input(AppMsg::SetMaskOverlayShown(true));
            });
        }
        root.add_controller(motion);

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

        // Header: title + reset-all.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_bottom(4);
        let title = gtk::Label::new(Some("Masking"));
        title.add_css_class("title-4");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        header.append(&title);
        let reset = gtk::Button::from_icon_name("arrow-counterclockwise-regular");
        reset.add_css_class("flat");
        reset.set_tooltip_text(Some("Reset all masks"));
        reset.set_sensitive(!masks.is_empty());
        {
            let sender = sender.clone();
            reset.connect_clicked(move |_| sender.input(AppMsg::ResetAllMasks));
        }
        header.append(&reset);
        self.body.append(&header);

        if masks.is_empty() {
            let heading = gtk::Label::new(Some("Create New Mask"));
            heading.add_css_class("heading");
            heading.set_halign(gtk::Align::Start);
            heading.set_margin_bottom(2);
            self.body.append(&heading);
            self.body.append(&create_grid(sender));
            return;
        }

        let heading = gtk::Label::new(Some("Masks"));
        heading.add_css_class("heading");
        heading.set_halign(gtk::Align::Start);
        heading.set_margin_bottom(2);
        self.body.append(&heading);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        list.add_css_class("card");
        list.set_margin_top(4);
        for (i, m) in masks.iter().enumerate() {
            list.append(&mask_row(i, m, selected == Some(i), sender));
        }
        self.body.append(&list);

        // "Add new mask" → popover containing the same create grid.
        let add = gtk::MenuButton::new();
        add.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("add-regular")
                .label("Add new mask")
                .build(),
        ));
        add.add_css_class("flat");
        add.set_margin_top(2);
        let pop = gtk::Popover::new();
        pop.set_child(Some(&create_grid(sender)));
        add.set_popover(Some(&pop));
        self.body.append(&add);

        if let Some(i) = selected {
            if let Some(m) = masks.get(i) {
                self.body.append(&mask_details(i, m, &self.vadj, sender));
            }
        }
    }
}

/// 3-col "Create New Mask" card grid. Primary cards add their mask; the final
/// "Others" card opens a popover listing [`OTHERS_TYPES`]. Shared by the empty
/// state and the "Add new mask" popover.
fn create_grid(sender: &ComponentSender<AppModel>) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(6);
    grid.set_column_homogeneous(true);

    let card = |icon: Option<&str>, label: &str| {
        let b = gtk::Button::new();
        b.add_css_class("card");
        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        if let Some(icon) = icon {
            let img = gtk::Image::from_icon_name(icon);
            img.set_pixel_size(22);
            content.append(&img);
        }
        let lbl = gtk::Label::new(Some(label));
        lbl.set_wrap(true);
        lbl.set_justify(gtk::Justification::Center);
        content.append(&lbl);
        b.set_child(Some(&content));
        b
    };

    for (idx, &(label, ty)) in MASK_CREATE_GRID.iter().enumerate() {
        let b = card(mask_icon(ty), label);
        let sender = sender.clone();
        b.connect_clicked(move |_| sender.input(AppMsg::AddMask(ty)));
        grid.attach(&b, (idx % 3) as i32, (idx / 3) as i32, 1, 1);
    }

    // "Others" popover card.
    let others = gtk::MenuButton::new();
    others.add_css_class("card");
    let oc = gtk::Box::new(gtk::Orientation::Vertical, 4);
    oc.set_margin_top(12);
    oc.set_margin_bottom(12);
    oc.append(&gtk::Image::from_icon_name("more-horizontal-regular"));
    let ol = gtk::Label::new(Some("Others"));
    ol.set_wrap(true);
    ol.set_justify(gtk::Justification::Center);
    oc.append(&ol);
    others.set_child(Some(&oc));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.set_margin_all(4);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&list));
    for &(label, ty) in OTHERS_TYPES {
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
    others.set_popover(Some(&pop));
    let n = MASK_CREATE_GRID.len();
    grid.attach(&others, (n % 3) as i32, (n / 3) as i32, 1, 1);

    grid
}

/// "Add sub-mask" menu for a container (non-AI types), emitting `AddSubMask`.
fn sub_add_menu(mask_i: usize, sender: &ComponentSender<AppModel>) -> gtk::MenuButton {
    let btn = gtk::MenuButton::new();
    btn.set_child(Some(
        &adw::ButtonContent::builder()
            .icon_name("add-regular")
            .label("Add sub-mask")
            .build(),
    ));
    btn.add_css_class("flat");
    btn.set_margin_start(6);
    btn.set_margin_end(6);

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
            sender.input(AppMsg::AddSubMask(mask_i, ty));
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
        "eye-regular"
    } else {
        "eye-off-regular"
    });
    eye.set_active(m.visible);
    eye.add_css_class("flat");
    eye.set_tooltip_text(Some("Toggle visibility"));
    {
        let sender = sender.clone();
        eye.connect_clicked(move |_| sender.input(AppMsg::ToggleMaskVisible(i)));
    }
    row.append(&eye);

    // Name: single-click selects; double-click renames (Stack swaps to an Entry).
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

    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.add_named(&name, Some("label"));
    let entry = gtk::Entry::new();
    entry.set_text(&m.name);
    entry.set_hexpand(true);
    stack.add_named(&entry, Some("edit"));
    stack.set_visible_child_name("label");

    let dbl = gtk::GestureClick::new();
    dbl.set_button(gtk::gdk::BUTTON_PRIMARY);
    {
        let stack = stack.clone();
        let entry = entry.clone();
        dbl.connect_pressed(move |g, n, _, _| {
            if n == 2 {
                g.set_state(gtk::EventSequenceState::Claimed);
                stack.set_visible_child_name("edit");
                entry.grab_focus();
            }
        });
    }
    name.add_controller(dbl);

    // Commit on Enter or focus-out (idempotent if both fire).
    let commit = {
        let sender = sender.clone();
        let stack = stack.clone();
        std::rc::Rc::new(move |e: &gtk::Entry| {
            sender.input(AppMsg::RenameMask(i, e.text().to_string()));
            stack.set_visible_child_name("label");
        })
    };
    {
        let commit = commit.clone();
        entry.connect_activate(move |e| commit(e));
    }
    {
        let entry2 = entry.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |_| commit(&entry2));
        entry.add_controller(focus);
    }
    row.append(&stack);

    let del = gtk::Button::from_icon_name("user-trash-symbolic");
    del.add_css_class("flat");
    del.set_tooltip_text(Some("Delete mask"));
    {
        let sender = sender.clone();
        del.connect_clicked(move |_| sender.input(AppMsg::DeleteMask(i)));
    }
    row.append(&del);

    // Right-click context menu: duplicate / invert / copy / paste / delete.
    let menu = gtk::Popover::new();
    menu.set_has_arrow(false);
    menu.set_parent(&row);
    let items = gtk::Box::new(gtk::Orientation::Vertical, 2);
    items.set_margin_all(4);
    // AppMsg isn't Clone, so each item carries a builder closure (captures `i`,
    // which is Copy) instead of a prebuilt message.
    let add_item = |label: &str, build: Box<dyn Fn() -> AppMsg>| {
        let b = gtk::Button::with_label(label);
        b.add_css_class("flat");
        b.set_halign(gtk::Align::Fill);
        let sender = sender.clone();
        let menu = menu.clone();
        b.connect_clicked(move |_| {
            menu.popdown();
            sender.input(build());
        });
        b
    };
    items.append(&add_item("Duplicate", Box::new(move || AppMsg::DuplicateMask(i))));
    items.append(&add_item(
        "Duplicate & Invert",
        Box::new(move || AppMsg::DuplicateMaskInvert(i)),
    ));
    items.append(&add_item("Copy mask", Box::new(move || AppMsg::CopyMask(i))));
    // ponytail: Paste always enabled; handler no-ops when clipboard empty.
    items.append(&add_item("Paste mask", Box::new(|| AppMsg::PasteMask)));
    items.append(&add_item("Delete", Box::new(move || AppMsg::DeleteMask(i))));
    menu.set_child(Some(&items));

    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    {
        let menu = menu.clone();
        click.connect_pressed(move |_, _, x, y| {
            menu.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            menu.popup();
        });
    }
    row.add_controller(click);

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
    card.append(&sub_add_menu(i, sender));

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

    card.append(&build_mask_hsl(i, m, vadj, sender));
    card.append(&build_mask_grading(i, m, vadj, sender));
    card.append(&build_mask_curves(i, m, vadj, sender));
    card
}

/// Manual control points for one curve channel, read from the mask's
/// `adjustments.curves.<key>` JSON. Falls back to the identity line.
fn curve_seed(curves: Option<&Value>, key: &str) -> Vec<(f64, f64)> {
    let identity = || vec![(0.0, 0.0), (255.0, 255.0)];
    let Some(arr) = curves.and_then(|c| c.get(key)).and_then(Value::as_array) else {
        return identity();
    };
    let pts: Vec<(f64, f64)> = arr
        .iter()
        .filter_map(|p| Some((p.get("x")?.as_f64()?, p.get("y")?.as_f64()?)))
        .collect();
    if pts.len() < 2 {
        identity()
    } else {
        pts
    }
}

/// Per-mask tone curves: the shared `CurveEditor`, seeded from the mask's stored
/// points and writing back to `adjustments.curves.<channel>` JSON.
fn build_mask_curves(
    i: usize,
    m: &MaskDefinition,
    vadj: &gtk::Adjustment,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
    use crate::curves::CurveEditor;

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 4);
    wrap.set_margin_all(6);
    let head = gtk::Label::new(Some("Curves"));
    head.set_halign(gtk::Align::Start);
    head.add_css_class("heading");
    wrap.append(&head);

    let curves = m.adjustments.get("curves");
    // Channel order matches CurveEditor's: [luma, red, green, blue].
    let seed = [
        curve_seed(curves, "luma"),
        curve_seed(curves, "red"),
        curve_seed(curves, "green"),
        curve_seed(curves, "blue"),
    ];
    let sender = sender.clone();
    let editor = CurveEditor::with_sink(vadj, seed, move |channel, points| {
        sender.input(AppMsg::MaskCurve { index: i, channel, points });
    });
    wrap.append(editor.root());
    wrap
}

/// HSL bands: `(display, json key)`, in core's band order.
const HSL_BANDS: &[(&str, &str)] = &[
    ("Reds", "reds"),
    ("Oranges", "oranges"),
    ("Yellows", "yellows"),
    ("Greens", "greens"),
    ("Aquas", "aquas"),
    ("Blues", "blues"),
    ("Purples", "purples"),
    ("Magentas", "magentas"),
];

/// Per-mask HSL mixer: a band selector + Hue/Sat/Lum sliders for the chosen
/// band, writing `adjustments.hsl.<band>` JSON. Sliders rebuild on band switch,
/// seeded from stored values.
fn build_mask_hsl(
    i: usize,
    m: &MaskDefinition,
    vadj: &gtk::Adjustment,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 4);
    wrap.set_margin_all(6);
    let head = gtk::Label::new(Some("HSL"));
    head.set_halign(gtk::Align::Start);
    head.add_css_class("heading");
    wrap.append(&head);

    let band = adw::ComboRow::new();
    band.set_title("Band");
    band.set_model(Some(&gtk::StringList::new(
        &HSL_BANDS.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
    )));
    wrap.append(&band);

    let sliders = gtk::Box::new(gtk::Orientation::Vertical, 2);
    wrap.append(&sliders);

    // Snapshot adjustments so the rebuild closure can seed from JSON.
    let adj = m.adjustments.clone();
    let vadj = vadj.clone();
    let sender = sender.clone();
    let rebuild = move |b: usize| {
        while let Some(c) = sliders.first_child() {
            sliders.remove(&c);
        }
        let (_, key) = HSL_BANDS[b];
        let zone = adj.get("hsl").and_then(|h| h.get(key));
        for (label, comp) in [
            ("Hue", "hue"),
            ("Saturation", "saturation"),
            ("Luminance", "luminance"),
        ] {
            let cur = zone
                .and_then(|z| z.get(comp))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let (sl, _, h) = slider_ex(label, -100.0, 100.0, 1.0, 0.0, Track::Plain, &vadj, {
                let sender = sender.clone();
                move |v| sender.input(AppMsg::MaskHsl { index: i, band: key, comp, value: v })
            });
            h.set_ui(cur);
            sliders.append(&sl);
        }
    };
    let rebuild = std::rc::Rc::new(rebuild);
    {
        let rebuild = rebuild.clone();
        band.connect_selected_notify(move |r| rebuild(r.selected() as usize));
    }
    rebuild(0);
    wrap
}

/// Per-mask color grading: 4 wheels (shadows/midtones/highlights/global) + the
/// blending/balance sliders, writing the mask's `adjustments.colorGrading` JSON.
fn build_mask_grading(
    i: usize,
    m: &MaskDefinition,
    vadj: &gtk::Adjustment,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
    use crate::colorwheel::ColorWheel;

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 4);
    wrap.set_margin_all(6);
    let head = gtk::Label::new(Some("Color Grading"));
    head.set_halign(gtk::Align::Start);
    head.add_css_class("heading");
    wrap.append(&head);

    let cg = m.adjustments.get("colorGrading");
    // Read a zone's stored (hue°, sat 0..1, lum) from JSON for seeding the wheel.
    let seed = |zone: &str| -> (f64, f64, f64) {
        let z = cg.and_then(|c| c.get(zone));
        let g = |k: &str| z.and_then(|z| z.get(k)).and_then(Value::as_f64).unwrap_or(0.0);
        (g("hue"), g("saturation") / 100.0, g("luminance"))
    };

    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_column_spacing(4);
    flow.set_row_spacing(4);
    flow.set_homogeneous(true);
    for (label, zone) in [
        ("Shadows", "shadows"),
        ("Midtones", "midtones"),
        ("Highlights", "highlights"),
        ("Global", "global"),
    ] {
        let sender = sender.clone();
        let w = ColorWheel::with_sink(label, vadj, seed(zone), move |hue, sat, lum| {
            sender.input(AppMsg::MaskGrade { index: i, zone, hue, sat, lum })
        });
        flow.append(w.root());
    }
    wrap.append(&flow);

    // Blending (default 50) + Balance (default 0).
    for (label, key, min, max, default) in [
        ("Blending", "blending", 0.0, 100.0, 50.0),
        ("Balance", "balance", -100.0, 100.0, 0.0),
    ] {
        let cur = cg
            .and_then(|c| c.get(key))
            .and_then(Value::as_f64)
            .unwrap_or(default);
        let (sl, _, h) = slider_ex(label, min, max, 1.0, default, Track::Plain, vadj, {
            let sender = sender.clone();
            move |v| sender.input(AppMsg::MaskGradeScalar { index: i, key, value: v })
        });
        h.set_ui(cur);
        wrap.append(&sl);
    }
    wrap
}

/// Geometry + compositing-mode editor for one sub-mask (libadwaita rows). Brush/
/// flow show a canvas hint (P4); "all" has no geometry.
/// Build the editor group for one sub-mask. Shared with the inpaint panel:
/// `mask_i` is the container index (a mask or, when the inpaint panel is active,
/// a patch) — the handlers route by the model's `edit_patch` flag.
pub fn submask_editor(
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

    // Header controls: type icon, then visibility, invert, delete.
    let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    if let Some(icon) = mask_icon(&sm.mask_type) {
        let img = gtk::Image::from_icon_name(icon);
        img.set_pixel_size(16);
        img.set_margin_end(4);
        suffix.append(&img);
    }
    let eye = gtk::ToggleButton::new();
    eye.set_icon_name(if sm.visible {
        "eye-regular"
    } else {
        "eye-off-regular"
    });
    eye.set_active(sm.visible);
    eye.add_css_class("flat");
    eye.set_tooltip_text(Some("Toggle sub-mask visibility"));
    {
        let sender = sender.clone();
        eye.connect_clicked(move |_| {
            sender.input(AppMsg::ToggleSubMaskVisible {
                mask: mask_i,
                sub: sub_i,
            })
        });
    }
    let inv = gtk::ToggleButton::new();
    inv.set_icon_name("object-flip-horizontal-symbolic");
    inv.set_active(sm.invert);
    inv.add_css_class("flat");
    inv.set_tooltip_text(Some("Invert sub-mask"));
    {
        let sender = sender.clone();
        inv.connect_clicked(move |_| {
            sender.input(AppMsg::ToggleSubMaskInvert {
                mask: mask_i,
                sub: sub_i,
            })
        });
    }
    let del = gtk::Button::from_icon_name("user-trash-symbolic");
    del.add_css_class("flat");
    del.set_tooltip_text(Some("Delete sub-mask"));
    {
        let sender = sender.clone();
        del.connect_clicked(move |_| {
            sender.input(AppMsg::DeleteSubMask {
                mask: mask_i,
                sub: sub_i,
            })
        });
    }
    suffix.append(&eye);
    suffix.append(&inv);
    suffix.append(&del);
    group.set_header_suffix(Some(&suffix));

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

    if is_ai_type(&sm.mask_type) {
        ai_controls(&group, mask_i, sub_i, sm, sender);
        return group;
    }

    let rows = geo_rows(&sm.mask_type);
    if rows.is_empty() {
        if matches!(sm.mask_type.as_str(), "brush" | "flow") {
            brush_controls(&group, sub_i, sm, sender);
        } else {
            let hint = adw::ActionRow::new();
            hint.set_title("No geometry");
            hint.add_css_class("dim-label");
            group.add(&hint);
        }
        return group;
    }

    // Color/luminance masks sample a point on the image; offer canvas picking
    // alongside the numeric Target X/Y fallback.
    if matches!(sm.mask_type.as_str(), "color" | "luminance") {
        let pick = adw::SwitchRow::new();
        pick.set_title("Pick target on image");
        pick.set_subtitle("Click the colour/tone to sample");
        // Seed ON to match the auto-arm on creation (set before connect: no emit).
        pick.set_active(true);
        let sender = sender.clone();
        pick.connect_active_notify(move |r| {
            sender.input(AppMsg::ArmPick(r.is_active().then_some(sub_i)));
        });
        group.add(&pick);
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

/// Brush/flow painting controls: brush size, a Paint arm toggle, and Clear.
/// Stroke count shown so the user sees painting took effect.
fn brush_controls(
    group: &adw::PreferencesGroup,
    sub_i: usize,
    sm: &SubMask,
    sender: &ComponentSender<AppModel>,
) {
    let size = adw::SpinRow::with_range(1.0, 1000.0, 1.0);
    size.set_title("Brush size (px)");
    size.set_value(50.0);
    {
        let sender = sender.clone();
        size.connect_changed(move |r| sender.input(AppMsg::SetBrushSize(r.value())));
    }
    group.add(&size);

    let feather = adw::SpinRow::with_range(0.0, 100.0, 1.0);
    feather.set_title("Feather");
    feather.set_value(50.0);
    {
        let sender = sender.clone();
        feather.connect_changed(move |r| sender.input(AppMsg::SetBrushFeather(r.value())));
    }
    group.add(&feather);

    // Add | Erase as a linked segmented toggle (Tauri BrushTools), not a switch.
    let seg = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    seg.add_css_class("linked");
    seg.set_homogeneous(true);
    seg.set_hexpand(true);
    let add_btn = gtk::ToggleButton::with_label("Add");
    let erase_btn = gtk::ToggleButton::with_label("Erase");
    erase_btn.set_group(Some(&add_btn));
    add_btn.set_active(true); // default = paint (add)
    {
        let sender = sender.clone();
        add_btn.connect_toggled(move |b| {
            if b.is_active() {
                sender.input(AppMsg::SetBrushErase(false));
            }
        });
    }
    {
        let sender = sender.clone();
        erase_btn.connect_toggled(move |b| {
            if b.is_active() {
                sender.input(AppMsg::SetBrushErase(true));
            }
        });
    }
    seg.append(&add_btn);
    seg.append(&erase_btn);
    let erase_row = adw::ActionRow::new();
    erase_row.set_title("Mode");
    erase_row.add_suffix(&seg);
    group.add(&erase_row);

    let paint = adw::SwitchRow::new();
    paint.set_title("Paint");
    paint.set_subtitle("Drag on the image to paint this mask");
    // Seed ON to match the auto-arm on creation. Set BEFORE connecting so the
    // seed doesn't emit ArmPaint. ponytail: a brush mask disarmed then rebuilt
    // shows ON-but-idle until re-toggled; not worth threading armed state through
    // every rebuild to fix that rare case.
    paint.set_active(true);
    {
        let sender = sender.clone();
        paint.connect_active_notify(move |r| {
            sender.input(AppMsg::ArmPaint(r.is_active().then_some(sub_i)));
        });
    }
    group.add(&paint);

    let strokes = sm
        .parameters
        .get("lines")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let clear = adw::ActionRow::new();
    clear.set_title("Clear strokes");
    clear.set_subtitle(&format!("{strokes} painted"));
    let clear_btn = gtk::Button::from_icon_name("user-trash-symbolic");
    clear_btn.add_css_class("flat");
    clear_btn.set_valign(gtk::Align::Center);
    {
        let sender = sender.clone();
        clear_btn.connect_clicked(move |_| sender.input(AppMsg::ClearStrokes(sub_i)));
    }
    clear.add_suffix(&clear_btn);
    group.add(&clear);
}

/// AI sub-mask controls: a Generate button (runs the ONNX model), grow/feather
/// refinement, plus depth-range rows for `ai-depth`. The generated bitmap lives
/// in `maskDataBase64`; everything else just refines it at render time.
fn ai_controls(
    group: &adw::PreferencesGroup,
    mask_i: usize,
    sub_i: usize,
    sm: &SubMask,
    sender: &ComponentSender<AppModel>,
) {
    let has_mask = sm
        .parameters
        .get("maskDataBase64")
        .map(|v| !v.is_null())
        .unwrap_or(false);

    let gen = adw::ActionRow::new();
    gen.set_title("Generate");
    gen.set_subtitle(if has_mask { "Generated" } else { "Not generated yet" });
    let gen_btn = gtk::Button::with_label(if has_mask { "Regenerate" } else { "Generate" });
    gen_btn.add_css_class("suggested-action");
    gen_btn.set_valign(gtk::Align::Center);
    {
        let sender = sender.clone();
        gen_btn.connect_clicked(move |_| sender.input(AppMsg::GenerateAiMask(sub_i)));
    }
    gen.add_suffix(&gen_btn);
    group.add(&gen);

    // ai-subject can be refined by drawing a box prompt on the canvas; the box
    // re-runs SAM. Other AI types segment the whole frame, so no box needed.
    if matches!(sm.mask_type.as_str(), "ai-subject" | "quick-eraser") {
        let pick = adw::SwitchRow::new();
        pick.set_title("Draw box on image");
        pick.set_subtitle("Drag a rectangle around the subject");
        let sender = sender.clone();
        pick.connect_active_notify(move |r| {
            sender.input(AppMsg::ArmPick(r.is_active().then_some(sub_i)));
        });
        group.add(&pick);
    }

    // Depth range (0..100) before grow/feather.
    if sm.mask_type == "ai-depth" {
        for (label, key) in [
            ("Min depth", "minDepth"),
            ("Max depth", "maxDepth"),
            ("Min fade", "minFade"),
            ("Max fade", "maxFade"),
        ] {
            ai_param_row(group, mask_i, sub_i, sm, sender, label, key, 0.0, 100.0, 0.0);
        }
    }

    ai_param_row(group, mask_i, sub_i, sm, sender, "Grow", "grow", -100.0, 100.0, 0.0);
    ai_param_row(group, mask_i, sub_i, sm, sender, "Feather", "feather", 0.0, 100.0, 0.0);
}

/// One AI-refinement SpinRow bound to `sm.parameters[key]` via `SetSubMaskParam`.
#[allow(clippy::too_many_arguments)]
fn ai_param_row(
    group: &adw::PreferencesGroup,
    mask_i: usize,
    sub_i: usize,
    sm: &SubMask,
    sender: &ComponentSender<AppModel>,
    label: &str,
    key: &'static str,
    min: f64,
    max: f64,
    default: f64,
) {
    let row = adw::SpinRow::with_range(min, max, 1.0);
    row.set_title(label);
    row.set_value(sm.parameters.get(key).and_then(Value::as_f64).unwrap_or(default));
    let sender = sender.clone();
    row.connect_changed(move |r| {
        sender.input(AppMsg::SetSubMaskParam {
            mask: mask_i,
            sub: sub_i,
            key,
            value: r.value(),
        });
    });
    group.add(&row);
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
    fn every_create_grid_type_is_a_known_mask_type() {
        for &(_, ty) in MASK_CREATE_GRID.iter().chain(OTHERS_TYPES.iter()) {
            assert!(
                MASK_TYPES.iter().any(|(_, t)| *t == ty),
                "create-grid type {ty} not in MASK_TYPES"
            );
        }
        // The two tables together offer each type at most once.
        let mut seen: Vec<&str> = MASK_CREATE_GRID
            .iter()
            .chain(OTHERS_TYPES.iter())
            .map(|(_, t)| *t)
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            MASK_CREATE_GRID.len() + OTHERS_TYPES.len(),
            "duplicate type across tables"
        );
    }

    #[test]
    fn clone_mask_gives_fresh_ids_and_keeps_data() {
        let m = new_mask("Radial", "radial", 1000.0, 800.0);
        let c = clone_mask(&m, false);
        assert_ne!(c.id, m.id, "container id must be fresh");
        assert_eq!(c.sub_masks.len(), m.sub_masks.len());
        assert_ne!(c.sub_masks[0].id, m.sub_masks[0].id, "sub id must be fresh");
        assert_eq!(c.adjustments, m.adjustments, "adjustments preserved");
        assert_eq!(c.invert, m.invert);
        let inv = clone_mask(&m, true);
        assert_eq!(inv.invert, !m.invert);
    }

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

    #[test]
    fn curve_seed_parses_and_falls_back() {
        // Missing / no curves -> identity.
        assert_eq!(curve_seed(None, "luma"), vec![(0.0, 0.0), (255.0, 255.0)]);
        // Valid points round-trip.
        let curves = json!({ "red": [{"x": 0.0, "y": 10.0}, {"x": 128.0, "y": 100.0}, {"x": 255.0, "y": 255.0}] });
        assert_eq!(
            curve_seed(Some(&curves), "red"),
            vec![(0.0, 10.0), (128.0, 100.0), (255.0, 255.0)]
        );
        // Degenerate single point -> identity (engine needs >= 2 to be non-identity).
        let bad = json!({ "blue": [{"x": 0.0, "y": 0.0}] });
        assert_eq!(curve_seed(Some(&bad), "blue"), vec![(0.0, 0.0), (255.0, 255.0)]);
    }
}
