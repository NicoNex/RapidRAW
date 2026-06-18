//! An interactive tone-curve editor, mirroring the original RapidRAW UI.
//!
//! A row of four channel toggles (Luma/Red/Green/Blue) selects which curve is
//! shown in a square `DrawingArea`. Each channel holds a list of control points
//! in the 0..255 domain; the curve drawn between them uses the same monotone
//! cubic Hermite interpolation as the engine shader's `apply_curve`, so the
//! preview line matches the rendered result.
//!
//! Interaction: drag a dot to move it, click empty space to add a point,
//! double-click an interior dot to remove it. Endpoints (x=0 and x=255) keep
//! their x fixed and only move vertically. After any change the active
//! channel's points are forwarded via `AppMsg::CurveChanged`.

use std::cell::{Cell, RefCell};
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;
use relm4::ComponentSender;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Luma,
    Red,
    Green,
    Blue,
}

/// Square draw area edge, in px. Also used as the content size fallback.
const SIZE: i32 = 240;
/// Pointer distance (px) within which a click/drag grabs an existing dot.
const HIT_RADIUS: f64 = 10.0;
/// Number of x samples used to draw the smooth curve.
const SAMPLES: usize = 128;

fn ch_idx(ch: Channel) -> usize {
    match ch {
        Channel::Luma => 0,
        Channel::Red => 1,
        Channel::Green => 2,
        Channel::Blue => 3,
    }
}

/// Parametric curve regions (each -100..100), plus black/white level offsets.
/// Ported from the original `ParametricCurveSettings` / `buildParametricPoints`.
#[derive(Clone, Copy, Default)]
struct ParamSettings {
    highlights: f64,
    lights: f64,
    darks: f64,
    shadows: f64,
    whites: f64,
    blacks: f64,
}

/// Per-channel manual control points (0..255, sorted by x) and parametric
/// settings. The active curve sent to the engine is the parametric one in
/// parametric mode, else the manual points.
struct State {
    points: [Vec<(f64, f64)>; 4],
    param: [ParamSettings; 4],
}

impl State {
    fn new() -> Self {
        let identity = || vec![(0.0, 0.0), (255.0, 255.0)];
        Self {
            points: [identity(), identity(), identity(), identity()],
            param: [ParamSettings::default(); 4],
        }
    }

    fn points(&self, ch: Channel) -> &Vec<(f64, f64)> {
        &self.points[ch_idx(ch)]
    }

    fn points_mut(&mut self, ch: Channel) -> &mut Vec<(f64, f64)> {
        &mut self.points[ch_idx(ch)]
    }
}

/// Port of the original `buildParametricPoints` (splits fixed at 25/50/75).
fn build_parametric(s: &ParamSettings) -> Vec<(f64, f64)> {
    let (v_h, v_l, v_d, v_s) = (
        s.highlights / 100.0,
        s.lights / 100.0,
        s.darks / 100.0,
        s.shadows / 100.0,
    );
    let (s1, s2, s3) = (0.25, 0.50, 0.75);
    let x_h = (s3 + 1.0) / 2.0;
    let x_s = s1 / 2.0;
    let xs = [0.0, x_s, s1, s2, s3, x_h, 1.0];

    let response = |v: f64, x: f64| {
        let headroom = if v >= 0.0 { 1.0 - x } else { x };
        (v * 1.2).tanh() * 0.35 * headroom.sqrt()
    };
    let ys = [
        0.0,
        x_s + response(v_s, x_s),
        s1 + (response(v_s, s1) + response(v_d, s1)) / 2.0,
        s2 + (response(v_d, s2) + response(v_l, s2)) / 2.0,
        s3 + (response(v_l, s3) + response(v_h, s3)) / 2.0,
        x_h + response(v_h, x_h),
        1.0,
    ];

    let mut pts: Vec<(f64, f64)> = xs
        .iter()
        .zip(ys.iter())
        .map(|(&x, &y)| (x * 255.0, y.clamp(0.0, 1.0) * 255.0))
        .collect();
    let last = pts.len() - 1;
    pts[0].1 = (pts[0].1 + s.blacks).clamp(0.0, 255.0);
    pts[last].1 = (pts[last].1 + s.whites).clamp(0.0, 255.0);
    pts
}

/// The curve currently driving the engine for `ch` (manual or parametric).
fn active_curve(s: &State, ch: Channel, parametric: bool) -> Vec<(f64, f64)> {
    if parametric {
        build_parametric(&s.param[ch_idx(ch)])
    } else {
        s.points(ch).clone()
    }
}

pub struct CurveEditor {
    root: gtk::Box,
}

impl CurveEditor {
    pub fn new(sender: &ComponentSender<crate::AppModel>, vadj: &gtk::Adjustment) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);

        let state = Rc::new(RefCell::new(State::new()));
        let active = Rc::new(Cell::new(Channel::Luma));
        let dragging = Rc::new(Cell::new(None::<usize>));
        // false = manual point curve, true = parametric region sliders.
        let parametric = Rc::new(Cell::new(false));

        let area = gtk::DrawingArea::new();
        area.set_content_width(SIZE);
        area.set_content_height(SIZE);

        // Channel selector: linked toggle buttons sharing one logical group.
        let toggles = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        toggles.add_css_class("linked");
        toggles.set_halign(gtk::Align::Center);

        let channels = [
            ("Luma", Channel::Luma),
            ("Red", Channel::Red),
            ("Green", Channel::Green),
            ("Blue", Channel::Blue),
        ];
        let mut group: Option<gtk::ToggleButton> = None;
        let mut channel_btns: Vec<(gtk::ToggleButton, Channel)> = Vec::new();
        for (label, ch) in channels {
            let btn = gtk::ToggleButton::with_label(label);
            if let Some(ref g) = group {
                btn.set_group(Some(g));
            } else {
                group = Some(btn.clone());
            }
            if ch == Channel::Luma {
                btn.set_active(true);
            }
            toggles.append(&btn);
            channel_btns.push((btn, ch));
        }
        root.append(&toggles);

        // Mode toggle: Point vs Parametric.
        let mode_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        mode_box.add_css_class("linked");
        mode_box.set_halign(gtk::Align::Center);
        let point_btn = gtk::ToggleButton::with_label("Point");
        let param_btn = gtk::ToggleButton::with_label("Parametric");
        param_btn.set_group(Some(&point_btn));
        point_btn.set_active(true);
        mode_box.append(&point_btn);
        mode_box.append(&param_btn);
        root.append(&mode_box);

        // Draw the active channel's curve, grid, identity diagonal and dots.
        {
            let state = state.clone();
            let active = active.clone();
            let parametric = parametric.clone();
            area.set_draw_func(move |_, cr, w, h| {
                let pts = active_curve(&state.borrow(), active.get(), parametric.get());
                draw_curve(cr, w, h, &pts, parametric.get());
            });
        }

        // Parametric region sliders (active channel). Built here so their
        // handles can be reloaded on channel/mode switch.
        let param_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        param_box.set_margin_start(4);
        param_box.set_margin_end(4);
        let param_rows: [(&str, f64, fn(&mut ParamSettings) -> &mut f64); 6] = [
            ("Highlights", 100.0, |p| &mut p.highlights),
            ("Lights", 100.0, |p| &mut p.lights),
            ("Darks", 100.0, |p| &mut p.darks),
            ("Shadows", 100.0, |p| &mut p.shadows),
            ("Whites", 50.0, |p| &mut p.whites),
            ("Blacks", 50.0, |p| &mut p.blacks),
        ];
        let mut handles: Vec<crate::slider::SliderHandle> = Vec::new();
        for (label, range, field) in param_rows {
            let state = state.clone();
            let active = active.clone();
            let parametric = parametric.clone();
            let area = area.clone();
            let sender = sender.clone();
            let (row, _, handle) = crate::slider::slider_ex(
                label, -range, range, 1.0, 0.0, crate::slider::Track::Plain, vadj,
                move |v| {
                    {
                        let mut s = state.borrow_mut();
                        *field(&mut s.param[ch_idx(active.get())]) = v;
                    }
                    emit_active(&sender, &state, active.get(), parametric.get());
                    area.queue_draw();
                },
            );
            handles.push(handle);
            param_box.append(&row);
        }
        let handles = Rc::new(handles);

        // Reload the slider UI from the active channel's stored settings.
        let reload = {
            let state = state.clone();
            let active = active.clone();
            let handles = handles.clone();
            Rc::new(move || {
                let p = state.borrow().param[ch_idx(active.get())];
                let vals = [p.highlights, p.lights, p.darks, p.shadows, p.whites, p.blacks];
                for (h, v) in handles.iter().zip(vals) {
                    h.set_ui(v);
                }
            })
        };

        // Channel switch: redraw and (in parametric mode) reload sliders + emit.
        for (btn, ch) in &channel_btns {
            let active = active.clone();
            let area = area.clone();
            let parametric = parametric.clone();
            let reload = reload.clone();
            let state = state.clone();
            let sender = sender.clone();
            let ch = *ch;
            btn.connect_toggled(move |b| {
                if b.is_active() {
                    active.set(ch);
                    if parametric.get() {
                        reload();
                        emit_active(&sender, &state, ch, true);
                    }
                    area.queue_draw();
                }
            });
        }

        // Mode switch: show/hide sliders, reload, emit the now-active curve.
        {
            let parametric = parametric.clone();
            let param_box = param_box.clone();
            let area = area.clone();
            let reload = reload.clone();
            let state = state.clone();
            let active = active.clone();
            let sender = sender.clone();
            param_btn.connect_toggled(move |b| {
                let on = b.is_active();
                parametric.set(on);
                param_box.set_visible(on);
                if on {
                    reload();
                }
                emit_active(&sender, &state, active.get(), on);
                area.queue_draw();
            });
        }
        param_box.set_visible(false);

        // Drag: grab the nearest dot on begin, move it on update.
        let drag = gtk::GestureDrag::new();
        // Pointer position at drag start (widget coords).
        let start = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
        {
            let start = start.clone();
            let dragging = dragging.clone();
            let state = state.clone();
            let active = active.clone();
            let area = area.clone();
            let parametric = parametric.clone();
            drag.connect_drag_begin(move |_, x, y| {
                if parametric.get() {
                    dragging.set(None);
                    return;
                }
                start.set((x, y));
                let pts = {
                    let s = state.borrow();
                    s.points(active.get()).clone()
                };
                dragging.set(nearest_dot(&area, &pts, x, y));
            });
        }
        {
            let start = start.clone();
            let dragging = dragging.clone();
            let state = state.clone();
            let active = active.clone();
            let area = area.clone();
            let sender = sender.clone();
            drag.connect_drag_update(move |_, ox, oy| {
                let Some(idx) = dragging.get() else { return };
                let (sx, sy) = start.get();
                let (px, py) = (sx + ox, sy + oy);
                let ch = active.get();
                let (mut nx, mut ny) = px_to_curve(&area, px, py);
                ny = ny.clamp(0.0, 255.0);
                {
                    let mut s = state.borrow_mut();
                    let pts = s.points_mut(ch);
                    let last = pts.len() - 1;
                    if idx == 0 {
                        // First point: x stays 0, only y moves.
                        pts[0].1 = ny;
                    } else if idx == last {
                        // Last point: x stays 255, only y moves.
                        pts[last].1 = ny;
                    } else {
                        // Interior point: clamp strictly between neighbors.
                        let lo = pts[idx - 1].0 + 1.0;
                        let hi = pts[idx + 1].0 - 1.0;
                        nx = nx.clamp(lo.min(hi), hi.max(lo));
                        pts[idx] = (nx, ny);
                    }
                }
                emit(&sender, &state, ch);
                area.queue_draw();
            });
        }
        {
            let dragging = dragging.clone();
            drag.connect_drag_end(move |_, _, _| dragging.set(None));
        }
        area.add_controller(drag);

        // Click: add a point on empty space; double-click removes an interior dot.
        let click = gtk::GestureClick::new();
        {
            let state = state.clone();
            let active = active.clone();
            let area = area.clone();
            let sender = sender.clone();
            let parametric = parametric.clone();
            click.connect_pressed(move |_, n, x, y| {
                if parametric.get() {
                    return;
                }
                let ch = active.get();
                let pts = {
                    let s = state.borrow();
                    s.points(ch).clone()
                };
                let hit = nearest_dot(&area, &pts, x, y);

                if n >= 2 {
                    // Double-click on an interior dot removes it (keep >= 2).
                    if let Some(idx) = hit {
                        let mut s = state.borrow_mut();
                        let pts = s.points_mut(ch);
                        let last = pts.len() - 1;
                        if idx != 0 && idx != last && pts.len() > 2 {
                            pts.remove(idx);
                            drop(s);
                            emit(&sender, &state, ch);
                            area.queue_draw();
                        }
                    }
                    return;
                }

                // Single click on empty area adds a point at that x, sorted.
                if hit.is_none() {
                    let (nx, ny) = px_to_curve(&area, x, y);
                    let nx = nx.clamp(0.0, 255.0);
                    let ny = ny.clamp(0.0, 255.0);
                    let mut s = state.borrow_mut();
                    let pts = s.points_mut(ch);
                    // Find sorted insertion index; skip if x coincides with one.
                    let insert = pts.iter().position(|p| p.0 >= nx);
                    match insert {
                        Some(i) if (pts[i].0 - nx).abs() < 1.0 => {}
                        Some(i) => pts.insert(i, (nx, ny)),
                        None => pts.push((nx, ny)),
                    }
                    drop(s);
                    emit(&sender, &state, ch);
                    area.queue_draw();
                }
            });
        }
        area.add_controller(click);

        root.append(&area);
        root.append(&param_box);
        Self { root }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }
}

/// Forward the active channel's manual points (as f32 0..255) to the model.
fn emit(sender: &ComponentSender<crate::AppModel>, state: &Rc<RefCell<State>>, ch: Channel) {
    let pts: Vec<(f32, f32)> = state
        .borrow()
        .points(ch)
        .iter()
        .map(|&(x, y)| (x as f32, y as f32))
        .collect();
    sender.input(crate::AppMsg::CurveChanged(ch, pts));
}

/// Forward whichever curve is active (manual or parametric) for `ch`.
fn emit_active(
    sender: &ComponentSender<crate::AppModel>,
    state: &Rc<RefCell<State>>,
    ch: Channel,
    parametric: bool,
) {
    let pts: Vec<(f32, f32)> = active_curve(&state.borrow(), ch, parametric)
        .iter()
        .map(|&(x, y)| (x as f32, y as f32))
        .collect();
    sender.input(crate::AppMsg::CurveChanged(ch, pts));
}

/// Allocated size, falling back to the configured content size.
fn area_size(area: &gtk::DrawingArea) -> (f64, f64) {
    let w = if area.width() > 0 {
        area.width()
    } else {
        SIZE
    };
    let h = if area.height() > 0 {
        area.height()
    } else {
        SIZE
    };
    (w as f64, h as f64)
}

/// Curve coords (0..255) -> widget px. y axis points up.
fn curve_to_px(w: f64, h: f64, x: f64, y: f64) -> (f64, f64) {
    (x / 255.0 * w, h - y / 255.0 * h)
}

/// Widget px -> curve coords (0..255). Inverse of `curve_to_px`.
fn px_to_curve(area: &gtk::DrawingArea, px: f64, py: f64) -> (f64, f64) {
    let (w, h) = area_size(area);
    let x = px / w * 255.0;
    let y = (h - py) / h * 255.0;
    (x, y)
}

/// Index of the control point whose dot is within `HIT_RADIUS` px of (px,py),
/// or `None`. Picks the closest if several are in range.
fn nearest_dot(area: &gtk::DrawingArea, pts: &[(f64, f64)], px: f64, py: f64) -> Option<usize> {
    let (w, h) = area_size(area);
    let mut best: Option<(usize, f64)> = None;
    for (i, &(x, y)) in pts.iter().enumerate() {
        let (dx, dy) = curve_to_px(w, h, x, y);
        let dist = ((dx - px).powi(2) + (dy - py).powi(2)).sqrt();
        if dist <= HIT_RADIUS && best.map_or(true, |(_, b)| dist < b) {
            best = Some((i, dist));
        }
    }
    best.map(|(i, _)| i)
}

/// Hermite basis evaluation, ported from the shader's `interpolate_cubic_hermite`.
fn interpolate_cubic_hermite(x: f64, p1: (f64, f64), p2: (f64, f64), m1: f64, m2: f64) -> f64 {
    let dx = p2.0 - p1.0;
    if dx <= 0.0 {
        return p1.1;
    }
    let t = (x - p1.0) / dx;
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    h00 * p1.1 + h10 * m1 * dx + h01 * p2.1 + h11 * m2 * dx
}

/// Evaluate the curve at `val` (x in 0..255), returning y in 0..255.
/// Direct port of the engine shader's `apply_curve` (monotone cubic Hermite
/// with Fritsch-Carlson tangent limiting), so the drawn line matches rendering.
fn apply_curve(val: f64, points: &[(f64, f64)]) -> f64 {
    let count = points.len();
    if count < 2 {
        return val;
    }
    let x = val;
    if x <= points[0].0 {
        return points[0].1;
    }
    if x >= points[count - 1].0 {
        return points[count - 1].1;
    }
    for i in 0..count - 1 {
        let p1 = points[i];
        let p2 = points[i + 1];
        if x <= p2.0 {
            let p0 = points[i.saturating_sub(1)];
            let p3 = points[(i + 2).min(count - 1)];
            let delta_before = (p1.1 - p0.1) / (p1.0 - p0.0).max(0.001);
            let delta_current = (p2.1 - p1.1) / (p2.0 - p1.0).max(0.001);
            let delta_after = (p3.1 - p2.1) / (p3.0 - p2.0).max(0.001);

            let mut tangent_at_p1 = if i == 0 {
                delta_current
            } else if delta_before * delta_current <= 0.0 {
                0.0
            } else {
                (delta_before + delta_current) / 2.0
            };
            let mut tangent_at_p2 = if i + 1 == count - 1 {
                delta_current
            } else if delta_current * delta_after <= 0.0 {
                0.0
            } else {
                (delta_current + delta_after) / 2.0
            };

            if delta_current != 0.0 {
                let alpha = tangent_at_p1 / delta_current;
                let beta = tangent_at_p2 / delta_current;
                if alpha * alpha + beta * beta > 9.0 {
                    let tau = 3.0 / (alpha * alpha + beta * beta).sqrt();
                    tangent_at_p1 *= tau;
                    tangent_at_p2 *= tau;
                }
            }

            let result_y = interpolate_cubic_hermite(x, p1, p2, tangent_at_p1, tangent_at_p2);
            return result_y.clamp(0.0, 255.0);
        }
    }
    points[count - 1].1
}

/// Render grid, identity diagonal, the smooth curve and the control dots.
/// Dots are hidden in parametric mode (the curve isn't directly editable).
fn draw_curve(cr: &cairo::Context, w: i32, h: i32, pts: &[(f64, f64)], parametric: bool) {
    if w <= 0 || h <= 0 {
        return;
    }
    let (wf, hf) = (w as f64, h as f64);

    // Background.
    cr.set_source_rgb(0.12, 0.12, 0.13);
    let _ = cr.paint();

    // Subtle grid (quarters).
    cr.set_line_width(1.0);
    cr.set_source_rgb(0.22, 0.22, 0.24);
    for k in 1..4 {
        let gx = wf * k as f64 / 4.0;
        cr.move_to(gx, 0.0);
        cr.line_to(gx, hf);
        let gy = hf * k as f64 / 4.0;
        cr.move_to(0.0, gy);
        cr.line_to(wf, gy);
    }
    let _ = cr.stroke();

    // Faint identity diagonal.
    cr.set_source_rgb(0.3, 0.3, 0.32);
    cr.move_to(0.0, hf);
    cr.line_to(wf, 0.0);
    let _ = cr.stroke();

    // The smooth curve, sampled across x.
    cr.set_line_width(2.0);
    cr.set_source_rgb(0.95, 0.95, 0.95);
    for s in 0..=SAMPLES {
        let x = 255.0 * s as f64 / SAMPLES as f64;
        let y = apply_curve(x, pts);
        let (cx, cy) = curve_to_px(wf, hf, x, y);
        if s == 0 {
            cr.move_to(cx, cy);
        } else {
            cr.line_to(cx, cy);
        }
    }
    let _ = cr.stroke();

    // Control points as draggable dots (manual mode only).
    if !parametric {
        for &(x, y) in pts {
            let (cx, cy) = curve_to_px(wf, hf, x, y);
            cr.set_source_rgb(0.1, 0.1, 0.1);
            cr.arc(cx, cy, 5.0, 0.0, TAU);
            let _ = cr.fill();
            cr.set_source_rgb(0.95, 0.95, 0.95);
            cr.arc(cx, cy, 4.0, 0.0, TAU);
            let _ = cr.fill();
        }
    }
}
