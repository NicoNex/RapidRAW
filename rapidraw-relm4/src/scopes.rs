//! Preview scopes shown at the top of the controls: RGB histogram, luma
//! waveform, and vectorscope. A small toggle picks which one is visible.
//!
//! All three are recomputed from each rendered preview (sparsely sampled so the
//! UI thread stays responsive) and drawn with cairo.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;
use image::RgbaImage;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Histogram,
    /// Luma waveform (white).
    WaveLuma,
    /// R/G/B waveforms overlaid (additive, tinted).
    WaveRgb,
    /// R/G/B waveforms side by side (parade).
    WaveParade,
    Vectorscope,
}

const WF_W: usize = 256;
const WF_H: usize = 128;
const VS: usize = 160;
/// Waveform channels: 0 = luma, 1 = R, 2 = G, 3 = B.
const WF_CH: usize = 4;

struct Data {
    hist: [[u32; 256]; 3],
    /// Per-channel waveforms, each `WF_W * WF_H`.
    wf: [Vec<u32>; WF_CH],
    wf_max: [u32; WF_CH],
    vs: Vec<u32>, // VS * VS
    vs_max: u32,
}

impl Data {
    fn empty() -> Self {
        Self {
            hist: [[0; 256]; 3],
            wf: std::array::from_fn(|_| vec![0; WF_W * WF_H]),
            wf_max: [1; WF_CH],
            vs: vec![0; VS * VS],
            vs_max: 1,
        }
    }
}

/// Callback fired when the clipping toggle changes (the model overlays
/// blown/crushed pixels on the preview).
type ClipCb = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

pub struct Scopes {
    root: gtk::Box,
    area: gtk::DrawingArea,
    data: Rc<RefCell<Data>>,
    clip_cb: ClipCb,
}

/// Scope area height bounds (px), user-resizable via the grip.
const SCOPE_H_MIN: i32 = 80;
const SCOPE_H_MAX: i32 = 400;
const SCOPE_H_DEFAULT: i32 = 110;

impl Scopes {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        // Inset to line up edge-to-edge with the `.card` widgets in the panel
        // below (their container has a 6px margin).
        root.set_margin_start(6);
        root.set_margin_end(6);
        root.set_margin_top(6);
        let data = Rc::new(RefCell::new(Data::empty()));
        let mode = Rc::new(Cell::new(Mode::Histogram));

        let area = gtk::DrawingArea::new();
        area.set_content_height(110);
        area.set_hexpand(true);
        // Rounded corners: clip the cairo fill to a rounded rect so the scope
        // matches the `.card` widgets below it.
        install_scope_css();
        area.add_css_class("scope-area");
        area.set_overflow(gtk::Overflow::Hidden);
        {
            let data = data.clone();
            let mode = mode.clone();
            area.set_draw_func(move |_, cr, w, h| draw(cr, w, h, &data.borrow(), mode.get()));
        }

        // Mode toggles.
        let toggles = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        toggles.add_css_class("linked");
        toggles.set_halign(gtk::Align::Center);
        let mut group: Option<gtk::ToggleButton> = None;
        for (label, tip, m) in [
            ("L", "Luma waveform", Mode::WaveLuma),
            ("RGB", "RGB waveform (overlay)", Mode::WaveRgb),
            ("P", "Parade (R/G/B side by side)", Mode::WaveParade),
            ("V", "Vectorscope", Mode::Vectorscope),
            ("H", "Histogram", Mode::Histogram),
        ] {
            let b = gtk::ToggleButton::with_label(label);
            b.add_css_class("caption");
            b.set_tooltip_text(Some(tip));
            match &group {
                Some(g) => b.set_group(Some(g)),
                None => group = Some(b.clone()),
            }
            if m == Mode::Histogram {
                b.set_active(true);
            }
            let mode = mode.clone();
            let area = area.clone();
            b.connect_toggled(move |b| {
                if b.is_active() {
                    mode.set(m);
                    area.queue_draw();
                }
            });
            toggles.append(&b);
        }

        // Clipping toggle (separate from the mode group): highlights blown/
        // crushed pixels on the preview via a model callback.
        let clip_cb: ClipCb = Rc::new(RefCell::new(None));
        let clip_btn = gtk::ToggleButton::new();
        clip_btn.set_icon_name("dialog-warning-symbolic");
        clip_btn.add_css_class("flat");
        clip_btn.set_tooltip_text(Some("Show clipped highlights/shadows"));
        clip_btn.set_margin_start(6);
        {
            let clip_cb = clip_cb.clone();
            clip_btn.connect_toggled(move |b| {
                if let Some(cb) = clip_cb.borrow().as_ref() {
                    cb(b.is_active());
                }
            });
        }
        let toggle_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        toggle_row.set_halign(gtk::Align::Center);
        toggle_row.append(&toggles);
        toggle_row.append(&clip_btn);

        // Drag grip below the scope to resize its height (in-session).
        let grip = gtk::DrawingArea::new();
        grip.set_content_height(8);
        grip.set_cursor_from_name(Some("ns-resize"));
        grip.set_draw_func(|_, cr, w, h| {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.25);
            let cw = (w as f64 * 0.25).min(40.0);
            cr.rectangle((w as f64 - cw) / 2.0, h as f64 / 2.0 - 1.0, cw, 2.0);
            let _ = cr.fill();
        });
        {
            let area = area.clone();
            let drag = gtk::GestureDrag::new();
            // Anchor on the pointer's surface (toplevel) Y, not the gesture offset:
            // resizing moves the grip, so an offset relative to it would feed back
            // and flicker. Surface coords are stable. (start_height, start_surface_y)
            let start = Rc::new(Cell::new((SCOPE_H_DEFAULT, 0.0_f64)));
            let surf_y = |g: &gtk::GestureDrag| -> Option<f64> {
                g.current_event().and_then(|e| e.position()).map(|(_, y)| y)
            };
            {
                let start = start.clone();
                let area = area.clone();
                drag.connect_drag_begin(move |g, _, _| {
                    let y = surf_y(g).unwrap_or(0.0);
                    start.set((area.content_height().max(SCOPE_H_MIN), y));
                });
            }
            drag.connect_drag_update(move |g, _, _| {
                let Some(y) = surf_y(g) else { return };
                let (sh, sy) = start.get();
                let h = (sh + (y - sy).round() as i32).clamp(SCOPE_H_MIN, SCOPE_H_MAX);
                area.set_content_height(h);
            });
            grip.add_controller(drag);
        }

        root.append(&toggle_row);
        root.append(&area);
        root.append(&grip);
        Self {
            root,
            area,
            data,
            clip_cb,
        }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// Set the callback invoked when the clipping toggle changes.
    pub fn set_clip_toggle(&self, cb: impl Fn(bool) + 'static) {
        *self.clip_cb.borrow_mut() = Some(Box::new(cb));
    }

    /// Recompute all scopes from a rendered preview (sampled every few pixels).
    pub fn set_data(&self, rgba: &RgbaImage) {
        let mut d = Data::empty();
        let (w, h) = rgba.dimensions();
        // Sample roughly 200k pixels max to keep the UI thread snappy.
        let total = (w as usize) * (h as usize);
        let step = (total / 200_000).max(1);
        let raw = rgba.as_raw();
        for (i, px) in raw.chunks_exact(4).enumerate().step_by(step) {
            let (r, g, b) = (px[0], px[1], px[2]);
            d.hist[0][r as usize] += 1;
            d.hist[1][g as usize] += 1;
            d.hist[2][b as usize] += 1;

            let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
            let luma = 0.299 * rf + 0.587 * gf + 0.114 * bf;

            // Waveform: column = x position, row = value. One grid per channel
            // (luma, R, G, B) so the draw step can do luma / RGB-overlay / parade.
            let x = i % (w as usize);
            let col = (x * WF_W / (w as usize)).min(WF_W - 1);
            for (ch, val) in [luma, rf, gf, bf].into_iter().enumerate() {
                let row = ((1.0 - val) * (WF_H as f32 - 1.0)) as usize;
                let wi = row.min(WF_H - 1) * WF_W + col;
                d.wf[ch][wi] += 1;
                d.wf_max[ch] = d.wf_max[ch].max(d.wf[ch][wi]);
            }

            // Vectorscope: chroma (Cb, Cr) around centre.
            let cb = -0.168 * rf - 0.331 * gf + 0.5 * bf;
            let cr = 0.5 * rf - 0.418 * gf - 0.081 * bf;
            let vx = ((cb + 0.5) * VS as f32) as isize;
            let vy = ((0.5 - cr) * VS as f32) as isize;
            if (0..VS as isize).contains(&vx) && (0..VS as isize).contains(&vy) {
                let vi = vy as usize * VS + vx as usize;
                d.vs[vi] += 1;
                d.vs_max = d.vs_max.max(d.vs[vi]);
            }
        }
        *self.data.borrow_mut() = d;
        self.area.queue_draw();
    }
}

impl Default for Scopes {
    fn default() -> Self {
        Self::new()
    }
}

/// Install the rounded-corner CSS for the scope area once for the default display.
fn install_scope_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(".scope-area { border-radius: 12px; }");
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

fn draw(cr: &cairo::Context, w: i32, h: i32, d: &Data, mode: Mode) {
    if w <= 0 || h <= 0 {
        return;
    }
    cr.set_source_rgb(0.10, 0.10, 0.11);
    let _ = cr.paint();
    // Channel tints (R, G, B) for the waveform overlay/parade.
    let rgb_tints = [(0.95, 0.30, 0.30), (0.35, 0.90, 0.40), (0.40, 0.60, 1.0)];
    match mode {
        Mode::Histogram => draw_histogram(cr, w as f64, h as f64, &d.hist),
        Mode::WaveLuma => draw_grid(cr, 0.0, 0.0, w as f64, h as f64, &d.wf[0], d.wf_max[0], WF_W, WF_H, (0.9, 0.9, 0.9)),
        Mode::WaveRgb => {
            // Overlay the three channels additively so coincident traces brighten.
            cr.set_operator(cairo::Operator::Add);
            for (ch, tint) in rgb_tints.iter().enumerate() {
                draw_grid(cr, 0.0, 0.0, w as f64, h as f64, &d.wf[ch + 1], d.wf_max[ch + 1], WF_W, WF_H, *tint);
            }
            cr.set_operator(cairo::Operator::Over);
        }
        Mode::WaveParade => {
            // R | G | B side by side, each in a horizontal third.
            let tw = w as f64 / 3.0;
            for (ch, tint) in rgb_tints.iter().enumerate() {
                draw_grid(cr, ch as f64 * tw, 0.0, tw, h as f64, &d.wf[ch + 1], d.wf_max[ch + 1], WF_W, WF_H, *tint);
            }
        }
        Mode::Vectorscope => draw_grid(cr, 0.0, 0.0, w as f64, h as f64, &d.vs, d.vs_max, VS, VS, (0.4, 0.9, 0.4)),
    }
}

fn draw_histogram(cr: &cairo::Context, wf: f64, hf: f64, hist: &[[u32; 256]; 3]) {
    let peak = hist
        .iter()
        .flat_map(|c| c[1..255].iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let colors = [(0.90, 0.25, 0.25), (0.25, 0.85, 0.30), (0.35, 0.55, 1.0)];
    // Lighten so overlapping channels brighten (mirrors the reference's
    // mix-blend lighten), each as a filled area plus a crisp top line.
    cr.set_operator(cairo::Operator::Lighten);
    let y_at = |ch: usize, i: usize| -> f64 {
        hf - ((hist[ch][i] as f64).sqrt() / peak.sqrt()).min(1.0) * hf
    };
    for (ch, col) in colors.iter().enumerate() {
        // Fill.
        cr.move_to(0.0, hf);
        for i in 0..256 {
            cr.line_to(i as f64 / 255.0 * wf, y_at(ch, i));
        }
        cr.line_to(wf, hf);
        cr.close_path();
        cr.set_source_rgba(col.0, col.1, col.2, 0.40);
        let _ = cr.fill();
        // Top line.
        cr.move_to(0.0, y_at(ch, 0));
        for i in 1..256 {
            cr.line_to(i as f64 / 255.0 * wf, y_at(ch, i));
        }
        cr.set_source_rgba(col.0, col.1, col.2, 0.95);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }
    cr.set_operator(cairo::Operator::Over);
}

/// Draw an intensity grid (waveform/vectorscope) into the widget rect `(x,y,w,h)`.
#[allow(clippy::too_many_arguments)]
fn draw_grid(
    cr: &cairo::Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    grid: &[u32],
    max: u32,
    gw: usize,
    gh: usize,
    tint: (f64, f64, f64),
) {
    let Ok(mut surface) = cairo::ImageSurface::create(cairo::Format::ARgb32, gw as i32, gh as i32)
    else {
        return;
    };
    let maxf = (max.max(1) as f64).sqrt();
    {
        let stride = surface.stride() as usize;
        let mut buf = surface.data().expect("surface data");
        for gy in 0..gh {
            for gx in 0..gw {
                let v = (grid[gy * gw + gx] as f64).sqrt() / maxf;
                let a = (v.min(1.0) * 255.0) as u8;
                let i = gy * stride + gx * 4;
                // Premultiplied ARGB (native-endian -> B,G,R,A).
                buf[i] = (tint.2 * a as f64) as u8;
                buf[i + 1] = (tint.1 * a as f64) as u8;
                buf[i + 2] = (tint.0 * a as f64) as u8;
                buf[i + 3] = a;
            }
        }
    }
    surface.mark_dirty();
    // Scale into the target rect without disturbing the caller's transform/operator.
    cr.save().ok();
    cr.translate(x, y);
    cr.scale(w / gw as f64, h / gh as f64);
    let _ = cr.set_source_surface(&surface, 0.0, 0.0);
    let _ = cr.paint();
    cr.restore().ok();
}
