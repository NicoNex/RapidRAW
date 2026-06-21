//! A color-grading wheel: a hue/saturation disc with a draggable handle plus a
//! luminance slider, mirroring the default UI's color wheels. Angle = hue
//! [0,360), radius = saturation [0,100]; the slider is luminance [-100,100].
//!
//! Each component writes one `GlobalAdjustments` field through a fn-pointer
//! setter (same `AppMsg::Adjust` path the sliders use). Double-click resets.

use std::cell::Cell;
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;
use relm4::ComponentSender;

use crate::{AppModel, AppMsg};
use rapidraw_core::image_processing::GlobalAdjustments;

type Setter = fn(&mut GlobalAdjustments, f32);

const DISC: i32 = 110;
/// Divisor for color-grading saturation/luminance (matches `SCALES`), so the
/// engine receives the same magnitude as the original UI.
const CG_SCALE: f64 = 500.0;

/// Sink fired on any wheel change with the three live components:
/// `(hue degrees, saturation 0..1, luminance -100..100)`.
type Emit3 = Rc<dyn Fn(f64, f64, f64)>;

#[derive(Clone)]
pub struct ColorWheel {
    root: gtk::Box,
    /// Hue + Saturation sliders, hidden by default; the panel's "toggle sliders"
    /// button reveals them (the disc always edits hue/sat too).
    sliders: gtk::Box,
}

impl ColorWheel {
    /// Global color-grading wheel: `hue_set`/`sat_set`/`lum_set` write one
    /// `color_grading_*` field (engine units) via `AppMsg::Adjust`.
    pub fn new(
        title: &str,
        sender: &ComponentSender<AppModel>,
        vadj: &gtk::Adjustment,
        hue_set: Setter,
        sat_set: Setter,
        lum_set: Setter,
    ) -> Self {
        let sender = sender.clone();
        let emit: Emit3 = Rc::new(move |hue, sat, lum| {
            sender.input(AppMsg::Adjust(crate::Adjust { set: hue_set, value: hue as f32 }));
            sender.input(AppMsg::Adjust(crate::Adjust {
                set: sat_set,
                value: (sat * 100.0 / CG_SCALE) as f32,
            }));
            sender.input(AppMsg::Adjust(crate::Adjust {
                set: lum_set,
                value: (lum / CG_SCALE) as f32,
            }));
        });
        let (root, sliders) = build(title, vadj, (0.0, 0.0, 0.0), emit, true);
        Self { root, sliders }
    }

    /// Generic wheel for non-global targets (e.g. per-mask color grading): seeded
    /// with `initial` (hue°, sat 0..1, lum -100..100), reporting changes to
    /// `on_change`. No global reset-hook registration.
    pub fn with_sink(
        title: &str,
        vadj: &gtk::Adjustment,
        initial: (f64, f64, f64),
        on_change: impl Fn(f64, f64, f64) + 'static,
    ) -> Self {
        let (root, sliders) = build(title, vadj, initial, Rc::new(on_change), false);
        Self { root, sliders }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// Show/hide the Hue + Saturation sliders (the panel toggles all wheels).
    pub fn set_sliders_visible(&self, visible: bool) {
        self.sliders.set_visible(visible);
    }
}

/// Build the wheel. `emit(hue°, sat01, lum)` fires on any change; `register_reset`
/// opts into the global panel's reset registry. Returns `(root, hue+sat box)`;
/// the second is hidden by default and toggled by the panel.
fn build(
    title: &str,
    vadj: &gtk::Adjustment,
    initial: (f64, f64, f64),
    emit: Emit3,
    register_reset: bool,
) -> (gtk::Box, gtk::Box) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
    root.set_halign(gtk::Align::Center);

    let label = gtk::Label::new(Some(title));
    label.add_css_class("caption");
    root.append(&label);

    // (hue degrees, saturation 0..1) for drawing the handle; luminance tracked
    // separately so `emit` can always report all three.
    let handle = Rc::new(Cell::new((initial.0, initial.1)));
    let lum_val = Rc::new(Cell::new(initial.2));

    // Live hue/sat for the luminance track gradient (matches the original's
    // `cg-lum-gradient`, black->tint->white at the wheel's colour). `Track::HslLum`
    // wants hue in degrees and sat as -100..100, so map sat 0..1 -> ±100
    // (es = (sat+100)/200 then recovers 0..1).
    let track_hue = Rc::new(Cell::new(initial.0));
    let track_sat = Rc::new(Cell::new(initial.1 * 200.0 - 100.0));

    let area = gtk::DrawingArea::new();
    area.set_content_width(DISC);
    area.set_content_height(DISC);
    {
        let handle = handle.clone();
        area.set_draw_func(move |_, cr, w, h| draw_wheel(cr, w, h, handle.get()));
    }

    // Luminance uses the shared custom slider (centre-origin fill, double-click
    // reset, value readout — matching the other panel sliders, exactly as the
    // original UI, which reuses its `Slider` here too, with the same dynamic
    // black->tint->white track). Built without panel registration so it doesn't
    // shift the snapshot index mapping; the wheel handles its own reset.
    let (lum_row, lum_area, lum_handle) = crate::slider::without_registration(|| {
        let handle = handle.clone();
        let lum_val = lum_val.clone();
        let emit = emit.clone();
        crate::slider::slider_ex(
            "Luminance",
            -100.0,
            100.0,
            1.0,
            0.0,
            crate::slider::Track::HslLum {
                base: 0.0,
                hue: track_hue.clone(),
                sat: track_sat.clone(),
            },
            vadj,
            move |v| {
                lum_val.set(v);
                let (hue, sat) = handle.get();
                emit(hue, sat, v);
            },
        )
    });
    lum_handle.set_ui(initial.2); // seed display without firing the callback

    // Saturation slider: grey -> saturated at the live hue (cg-sat-gradient).
    let (sat_row, sat_area, sat_handle) = crate::slider::without_registration(|| {
        let handle = handle.clone();
        let lum_val = lum_val.clone();
        let emit = emit.clone();
        let track_sat = track_sat.clone();
        let area = area.clone();
        let lum_area = lum_area.clone();
        crate::slider::slider_ex(
            "Saturation",
            0.0,
            100.0,
            1.0,
            0.0,
            crate::slider::Track::HslSat {
                base: 0.0,
                hue: track_hue.clone(),
            },
            vadj,
            move |v| {
                let sat = v / 100.0;
                let (hue, _) = handle.get();
                handle.set((hue, sat));
                track_sat.set(sat * 200.0 - 100.0);
                area.queue_draw();
                lum_area.queue_draw();
                emit(hue, sat, lum_val.get());
            },
        )
    });
    sat_handle.set_ui(initial.1 * 100.0);

    // Hue slider: full spectrum (cg-hue-gradient).
    let (hue_row, _hue_area, hue_handle) = crate::slider::without_registration(|| {
        let handle = handle.clone();
        let lum_val = lum_val.clone();
        let emit = emit.clone();
        let track_hue = track_hue.clone();
        let area = area.clone();
        let lum_area = lum_area.clone();
        let sat_area = sat_area.clone();
        crate::slider::slider_ex(
            "Hue",
            0.0,
            360.0,
            1.0,
            0.0,
            crate::slider::Track::Hue,
            vadj,
            move |v| {
                let (_, sat) = handle.get();
                handle.set((v, sat));
                track_hue.set(v);
                area.queue_draw();
                lum_area.queue_draw();
                sat_area.queue_draw(); // sat gradient follows hue
                emit(v, sat, lum_val.get());
            },
        )
    });
    hue_handle.set_ui(initial.0);

    // Hue + Saturation live in a collapsible box (hidden until the panel toggles
    // it); the disc edits the same values, syncing these sliders on drag.
    let sliders = gtk::Box::new(gtk::Orientation::Vertical, 2);
    sliders.set_visible(false);
    sliders.append(&hue_row);
    sliders.append(&sat_row);

    // Drag the handle: set hue/sat from the pointer (and refresh the lum track).
    let drag = gtk::GestureDrag::new();
    let start = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    {
        let start = start.clone();
        drag.connect_drag_begin(move |_, x, y| start.set((x, y)));
    }
    {
        let area_w = area.clone();
        let handle = handle.clone();
        let lum_val = lum_val.clone();
        let emit = emit.clone();
        let start = start.clone();
        let track_hue = track_hue.clone();
        let track_sat = track_sat.clone();
        let lum_area = lum_area.clone();
        let sat_area = sat_area.clone();
        let hue_handle = hue_handle.clone();
        let sat_handle = sat_handle.clone();
        drag.connect_drag_update(move |_, ox, oy| {
            let (sx, sy) = start.get();
            let (hue, sat) = point_to_hue_sat(&area_w, sx + ox, sy + oy);
            handle.set((hue, sat));
            track_hue.set(hue);
            track_sat.set(sat * 200.0 - 100.0);
            hue_handle.set_ui(hue); // keep the H/S sliders in sync
            sat_handle.set_ui(sat * 100.0);
            area_w.queue_draw();
            lum_area.queue_draw();
            sat_area.queue_draw();
            emit(hue, sat, lum_val.get());
        });
    }
    area.add_controller(drag);

    // Double-click anywhere on the wheel disc resets all three components.
    let reset = gtk::GestureClick::new();
    {
        let area_w = area.clone();
        let handle = handle.clone();
        let lum_handle = lum_handle.clone();
        let emit = emit.clone();
        let track_hue = track_hue.clone();
        let track_sat = track_sat.clone();
        let lum_area = lum_area.clone();
        let sat_area = sat_area.clone();
        let hue_handle = hue_handle.clone();
        let sat_handle = sat_handle.clone();
        reset.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                handle.set((0.0, 0.0));
                track_hue.set(0.0);
                track_sat.set(-100.0);
                hue_handle.set_ui(0.0);
                sat_handle.set_ui(0.0);
                area_w.queue_draw();
                lum_area.queue_draw();
                sat_area.queue_draw();
                lum_handle.set_ui(0.0); // update the slider display
                emit(0.0, 0.0, 0.0);
            }
        });
    }
    area.add_controller(reset);

    // Reset hook: clears the disc + luminance when a new image opens (global only).
    if register_reset {
        let handle = handle.clone();
        let area = area.clone();
        let lum_handle = lum_handle.clone();
        crate::slider::register_reset(Rc::new(move || {
            handle.set((0.0, 0.0));
            track_hue.set(0.0);
            track_sat.set(-100.0);
            hue_handle.set_ui(0.0);
            sat_handle.set_ui(0.0);
            area.queue_draw();
            lum_area.queue_draw();
            sat_area.queue_draw();
            lum_handle.set_ui(0.0);
        }));
    }

    // Order matches the original: hue + sat (collapsible) above, luminance below.
    root.append(&area);
    root.append(&sliders);
    root.append(&lum_row);
    (root, sliders)
}

/// Map a pointer position within `area` to (hue degrees [0,360), saturation [0,1]).
fn point_to_hue_sat(area: &gtk::DrawingArea, x: f64, y: f64) -> (f64, f64) {
    let size = area.width().min(area.height()).max(1) as f64;
    let c = size / 2.0;
    let (dx, dy) = (x - c, y - c);
    let r = (dx * dx + dy * dy).sqrt();
    let hue = dy.atan2(dx).to_degrees().rem_euclid(360.0);
    let sat = (r / c).clamp(0.0, 1.0);
    (hue, sat)
}

fn draw_wheel(cr: &cairo::Context, w: i32, h: i32, handle: (f64, f64)) {
    let size = w.min(h);
    if size <= 0 {
        return;
    }
    let Ok(mut surface) = cairo::ImageSurface::create(cairo::Format::ARgb32, size, size) else {
        return;
    };
    let c = size as f64 / 2.0;
    {
        let stride = surface.stride() as usize;
        let mut data = surface.data().expect("surface data");
        for yy in 0..size {
            for xx in 0..size {
                let dx = xx as f64 - c;
                let dy = yy as f64 - c;
                let r = (dx * dx + dy * dy).sqrt();
                let i = yy as usize * stride + xx as usize * 4;
                if r <= c {
                    let hue = dy.atan2(dx).to_degrees().rem_euclid(360.0);
                    let (rr, gg, bb) = hsv_to_rgb(hue, (r / c).min(1.0), 1.0);
                    // Cairo ARGB32 is premultiplied, native-endian -> B,G,R,A.
                    data[i] = bb;
                    data[i + 1] = gg;
                    data[i + 2] = rr;
                    data[i + 3] = 255;
                } else {
                    data[i + 3] = 0;
                }
            }
        }
    }
    surface.mark_dirty();
    let _ = cr.set_source_surface(&surface, 0.0, 0.0);
    let _ = cr.paint();

    // Handle marker.
    let (hue, sat) = handle;
    let ang = hue.to_radians();
    let hx = c + ang.cos() * sat * c;
    let hy = c + ang.sin() * sat * c;
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.arc(hx, hy, 5.0, 0.0, TAU);
    let _ = cr.stroke();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.arc(hx, hy, 4.0, 0.0, TAU);
    let _ = cr.stroke();
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
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
    let m = v - c;
    (
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}
