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
    Waveform,
    Vectorscope,
}

const WF_W: usize = 256;
const WF_H: usize = 128;
const VS: usize = 160;

struct Data {
    hist: [[u32; 256]; 3],
    wf: Vec<u32>, // WF_W * WF_H
    wf_max: u32,
    vs: Vec<u32>, // VS * VS
    vs_max: u32,
}

impl Data {
    fn empty() -> Self {
        Self {
            hist: [[0; 256]; 3],
            wf: vec![0; WF_W * WF_H],
            wf_max: 1,
            vs: vec![0; VS * VS],
            vs_max: 1,
        }
    }
}

pub struct Scopes {
    root: gtk::Box,
    area: gtk::DrawingArea,
    data: Rc<RefCell<Data>>,
}

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
        for (label, m) in [
            ("Histogram", Mode::Histogram),
            ("Waveform", Mode::Waveform),
            ("Vectorscope", Mode::Vectorscope),
        ] {
            let b = gtk::ToggleButton::with_label(label);
            b.add_css_class("caption");
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

        root.append(&toggles);
        root.append(&area);
        Self { root, area, data }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
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

            // Waveform: column = x position, row = luma.
            let x = i % (w as usize);
            let col = x * WF_W / (w as usize);
            let row = ((1.0 - luma) * (WF_H as f32 - 1.0)) as usize;
            let wi = row * WF_W + col.min(WF_W - 1);
            d.wf[wi] += 1;
            d.wf_max = d.wf_max.max(d.wf[wi]);

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
    match mode {
        Mode::Histogram => draw_histogram(cr, w as f64, h as f64, &d.hist),
        Mode::Waveform => draw_grid(cr, w, h, &d.wf, d.wf_max, WF_W, WF_H, (0.9, 0.9, 0.9)),
        Mode::Vectorscope => draw_grid(cr, w, h, &d.vs, d.vs_max, VS, VS, (0.4, 0.9, 0.4)),
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
    for (ch, col) in colors.iter().enumerate() {
        cr.set_source_rgba(col.0, col.1, col.2, 0.5);
        cr.move_to(0.0, hf);
        for i in 0..256 {
            let x = i as f64 / 255.0 * wf;
            let y = hf - ((hist[ch][i] as f64).sqrt() / peak.sqrt()).min(1.0) * hf;
            cr.line_to(x, y);
        }
        cr.line_to(wf, hf);
        cr.close_path();
        let _ = cr.fill();
    }
}

/// Draw an intensity grid (waveform/vectorscope) scaled to the widget.
fn draw_grid(
    cr: &cairo::Context,
    w: i32,
    h: i32,
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
        for y in 0..gh {
            for x in 0..gw {
                let v = (grid[y * gw + x] as f64).sqrt() / maxf;
                let a = (v.min(1.0) * 255.0) as u8;
                let i = y * stride + x * 4;
                // Premultiplied ARGB (native-endian -> B,G,R,A).
                buf[i] = (tint.2 * a as f64) as u8;
                buf[i + 1] = (tint.1 * a as f64) as u8;
                buf[i + 2] = (tint.0 * a as f64) as u8;
                buf[i + 3] = a;
            }
        }
    }
    surface.mark_dirty();
    cr.scale(w as f64 / gw as f64, h as f64 / gh as f64);
    let _ = cr.set_source_surface(&surface, 0.0, 0.0);
    let _ = cr.paint();
}
