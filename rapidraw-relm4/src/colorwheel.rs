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

pub struct ColorWheel {
    root: gtk::Box,
}

impl ColorWheel {
    /// `hue_set`/`sat_set`/`lum_set` write the three components of one
    /// `color_grading_*` field.
    pub fn new(
        title: &str,
        sender: &ComponentSender<AppModel>,
        hue_set: Setter,
        sat_set: Setter,
        lum_set: Setter,
    ) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.set_halign(gtk::Align::Center);

        let label = gtk::Label::new(Some(title));
        label.add_css_class("caption");
        root.append(&label);

        // (hue degrees, saturation 0..1) for drawing the handle.
        let handle = Rc::new(Cell::new((0.0_f64, 0.0_f64)));

        let area = gtk::DrawingArea::new();
        area.set_content_width(DISC);
        area.set_content_height(DISC);
        {
            let handle = handle.clone();
            area.set_draw_func(move |_, cr, w, h| draw_wheel(cr, w, h, handle.get()));
        }

        // Drag the handle: set hue/sat from the pointer.
        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
        {
            let start = start.clone();
            drag.connect_drag_begin(move |_, x, y| start.set((x, y)));
        }
        {
            let area_w = area.clone();
            let handle = handle.clone();
            let sender = sender.clone();
            let start = start.clone();
            drag.connect_drag_update(move |_, ox, oy| {
                let (sx, sy) = start.get();
                let (hue, sat) = point_to_hue_sat(&area_w, sx + ox, sy + oy);
                handle.set((hue, sat));
                area_w.queue_draw();
                sender.input(AppMsg::Adjust(crate::Adjust {
                    set: hue_set,
                    value: hue as f32,
                }));
                sender.input(AppMsg::Adjust(crate::Adjust {
                    set: sat_set,
                    value: (sat * 100.0) as f32,
                }));
            });
        }
        area.add_controller(drag);

        let lum = gtk::Scale::with_range(gtk::Orientation::Horizontal, -100.0, 100.0, 1.0);
        lum.set_hexpand(true);
        lum.set_draw_value(true);
        lum.set_value(0.0);
        {
            let sender = sender.clone();
            lum.connect_value_changed(move |s| {
                sender.input(AppMsg::Adjust(crate::Adjust {
                    set: lum_set,
                    value: s.value() as f32,
                }));
            });
        }

        // Double-click anywhere on the wheel resets all three components.
        let reset = gtk::GestureClick::new();
        {
            let area_w = area.clone();
            let handle = handle.clone();
            let lum = lum.clone();
            let sender = sender.clone();
            reset.connect_pressed(move |_, n, _, _| {
                if n == 2 {
                    handle.set((0.0, 0.0));
                    area_w.queue_draw();
                    lum.set_value(0.0); // fires lum change -> sets field + render
                    sender.input(AppMsg::Adjust(crate::Adjust {
                        set: hue_set,
                        value: 0.0,
                    }));
                    sender.input(AppMsg::Adjust(crate::Adjust {
                        set: sat_set,
                        value: 0.0,
                    }));
                }
            });
        }
        area.add_controller(reset);

        root.append(&area);
        root.append(&lum);
        Self { root }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }
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
