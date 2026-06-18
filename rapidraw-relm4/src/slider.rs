//! Custom adjustment slider, mirroring the original UI's `Slider.tsx`.
//!
//! Why not `gtk::Scale`? Two reasons the original relies on and `Scale` can't do:
//!   1. The fill (highlight) runs from the slider's *default* position to the
//!      current value — so a bipolar control (default 0, range ±100) lights up
//!      *from the centre* as you move away, with no misleading half-fill at
//!      rest. `Scale` only fills from `lower`.
//!   2. Gradient tracks (temperature, tint, hue, HSL hue) that show the user
//!      what the control does. Colours copied verbatim from `src/styles.css`.
//!
//! Built from a `DrawingArea` (track + fill + thumb) plus a header (label +
//! value). Drag to set, double-click or label-click to reset to default.

use std::cell::Cell;
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;

#[derive(Clone, Copy)]
pub enum Track {
    Plain,
    Temperature,
    Tint,
    /// Full-spectrum hue ramp (`hue-range-track`).
    Hue,
    /// HSL band hue: 3-stop ramp centred on `deg` (band centre hue).
    HslHue(f64),
}

const AREA_H: i32 = 20;
const TRACK_H: f64 = 6.0;
const THUMB_R: f64 = 6.5;
/// Fill overlay colour (semi-transparent accent), drawn over the track.
const ACCENT: (f64, f64, f64, f64) = (0.40, 0.62, 1.0, 0.55);

/// Build a slider row. `on_change` receives the snapped UI value on every change.
#[allow(clippy::too_many_arguments)]
pub fn slider(
    label: &str,
    min: f64,
    max: f64,
    step: f64,
    default: f64,
    track: Track,
    vadj: &gtk::Adjustment,
    on_change: impl Fn(f64) + 'static,
) -> gtk::Box {
    let decimals = if step < 1.0 { 2 } else { 0 };
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_halign(gtk::Align::Start);
    lbl.set_hexpand(true);
    lbl.add_css_class("caption");
    lbl.set_tooltip_text(Some("Click to reset"));
    let val_lbl = gtk::Label::new(Some(&fmt(default, decimals)));
    val_lbl.set_halign(gtk::Align::End);
    val_lbl.set_width_chars(5);
    val_lbl.add_css_class("caption");
    header.append(&lbl);
    header.append(&val_lbl);

    let area = gtk::DrawingArea::new();
    area.set_content_height(AREA_H);
    area.set_hexpand(true);

    let value = Rc::new(Cell::new(default));
    {
        let value = value.clone();
        area.set_draw_func(move |_, cr, w, h| {
            draw(cr, w, h, min, max, default, value.get(), track);
        });
    }

    let on_change = Rc::new(on_change);

    // Apply a new value: store, update label, redraw, notify.
    let apply = {
        let value = value.clone();
        let area = area.clone();
        let val_lbl = val_lbl.clone();
        let on_change = on_change.clone();
        Rc::new(move |v: f64| {
            value.set(v);
            val_lbl.set_text(&fmt(v, decimals));
            area.queue_draw();
            on_change(v);
        })
    };

    let from_x = {
        let area = area.clone();
        let apply = apply.clone();
        move |x: f64| {
            let frac = (x / (area.width().max(1) as f64)).clamp(0.0, 1.0);
            apply(snap(min + frac * (max - min), min, max, step));
        }
    };

    // Drag (and initial press) to set the value.
    let drag = gtk::GestureDrag::new();
    {
        let startx = Rc::new(Cell::new(0.0));
        let from_x = from_x.clone();
        {
            let startx = startx.clone();
            let from_x = from_x.clone();
            drag.connect_drag_begin(move |_, x, _| {
                startx.set(x);
                from_x(x);
            });
        }
        {
            let startx = startx.clone();
            drag.connect_drag_update(move |_, dx, _| from_x(startx.get() + dx));
        }
    }
    area.add_controller(drag);

    // Double-click the track resets to default.
    let dbl = gtk::GestureClick::new();
    {
        let apply = apply.clone();
        dbl.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                apply(default);
            }
        });
    }
    area.add_controller(dbl);

    // Click the label resets to default (reliable; matches the original).
    let lbl_click = gtk::GestureClick::new();
    {
        let apply = apply.clone();
        lbl_click.connect_released(move |_, _, _, _| apply(default));
    }
    lbl.add_controller(lbl_click);

    crate::controls::forward_wheel(&area, vadj);

    root.append(&header);
    root.append(&area);
    root
}

fn fmt(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}")
}

fn snap(v: f64, min: f64, max: f64, step: f64) -> f64 {
    let snapped = ((v - min) / step).round() * step + min;
    snapped.clamp(min, max)
}

fn draw(cr: &cairo::Context, w: i32, h: i32, min: f64, max: f64, default: f64, value: f64, track: Track) {
    if w <= 0 || h <= 0 || max <= min {
        return;
    }
    let wf = w as f64;
    let cy = h as f64 / 2.0;
    let ty = cy - TRACK_H / 2.0;
    let r = TRACK_H / 2.0;

    // Track background (solid or gradient), clipped to the rounded shape.
    rounded_rect(cr, 0.0, ty, wf, TRACK_H, r);
    cr.save().ok();
    cr.clip();
    match track {
        Track::Plain => {
            cr.set_source_rgb(0.26, 0.26, 0.28);
            cr.paint().ok();
        }
        _ => paint_gradient(cr, wf, track),
    }
    cr.restore().ok();

    // Fill overlay: from the default position to the current value.
    let o = ((default - min) / (max - min)).clamp(0.0, 1.0);
    let v = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let (x0, x1) = (o.min(v) * wf, o.max(v) * wf);
    if x1 - x0 > 0.5 {
        rounded_rect(cr, 0.0, ty, wf, TRACK_H, r);
        cr.save().ok();
        cr.clip();
        cr.rectangle(x0, ty, x1 - x0, TRACK_H);
        let (rr, gg, bb, aa) = ACCENT;
        cr.set_source_rgba(rr, gg, bb, aa);
        cr.fill().ok();
        cr.restore().ok();
    }

    // Thumb.
    let tx = (v * wf).clamp(THUMB_R, wf - THUMB_R);
    cr.arc(tx, cy, THUMB_R, 0.0, TAU);
    cr.set_source_rgb(0.95, 0.95, 0.97);
    cr.fill().ok();
    cr.arc(tx, cy, THUMB_R, 0.0, TAU);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
    cr.set_line_width(1.0);
    cr.stroke().ok();
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
}

fn paint_gradient(cr: &cairo::Context, wf: f64, track: Track) {
    let grad = cairo::LinearGradient::new(0.0, 0.0, wf, 0.0);
    match track {
        Track::Temperature => {
            // styles.css .temperature-gradient-track (dark variant).
            for (o, c) in [
                (0.0, (0x2d, 0x4a, 0x74)),
                (0.25, (0x49, 0x93, 0xb1)),
                (0.5, (0x8a, 0x8a, 0x8a)),
                (0.75, (0xc7, 0xc5, 0x49)),
                (1.0, (0xc7, 0x86, 0x3c)),
            ] {
                add_rgb(&grad, o, c);
            }
        }
        Track::Tint => {
            for (o, c) in [
                (0.0, (0x45, 0x8d, 0x43)),
                (0.25, (0x57, 0xce, 0x57)),
                (0.5, (0x8a, 0x8a, 0x8a)),
                (0.75, (0x9c, 0x54, 0x8a)),
                (1.0, (0xbe, 0x40, 0x9f)),
            ] {
                add_rgb(&grad, o, c);
            }
        }
        Track::Hue => {
            // Full spectrum (Slider.tsx hue-range-track).
            let stops = [
                0xff0000u32, 0xff8000, 0xffff00, 0x80ff00, 0x00ff00, 0x00ff80, 0x00ffff, 0x0080ff,
                0x0000ff, 0x8000ff, 0xff00ff, 0xff0080, 0xff0000,
            ];
            let n = (stops.len() - 1) as f64;
            for (i, hex) in stops.iter().enumerate() {
                let c = (((hex >> 16) & 0xff) as u8, ((hex >> 8) & 0xff) as u8, (hex & 0xff) as u8);
                add_rgb(&grad, i as f64 / n, c);
            }
        }
        Track::HslHue(center) => {
            // 3-stop ramp centred on the band hue (styles.css hsl hue sliders).
            add_rgb(&grad, 0.0, hsl_to_rgb((center - 100.0).rem_euclid(360.0), 0.5, 0.5));
            add_rgb(&grad, 0.5, hsl_to_rgb(center.rem_euclid(360.0), 0.5, 0.5));
            add_rgb(&grad, 1.0, hsl_to_rgb((center + 100.0).rem_euclid(360.0), 0.5, 0.5));
        }
        Track::Plain => {}
    }
    cr.set_source(&grad).ok();
    cr.paint().ok();
}

fn add_rgb(grad: &cairo::LinearGradient, offset: f64, c: (u8, u8, u8)) {
    grad.add_color_stop_rgb(
        offset,
        c.0 as f64 / 255.0,
        c.1 as f64 / 255.0,
        c.2 as f64 / 255.0,
    );
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    )
}
