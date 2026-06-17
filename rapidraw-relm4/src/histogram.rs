//! RGB histogram of the current preview, drawn at the top of the controls.
//!
//! ponytail: just the RGB histogram (the main graph). Add waveform/vectorscope
//! later if needed.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;
use image::RgbaImage;

pub struct Histogram {
    root: gtk::DrawingArea,
    bins: Rc<RefCell<[[u32; 256]; 3]>>,
}

impl Histogram {
    pub fn new() -> Self {
        let bins = Rc::new(RefCell::new([[0u32; 256]; 3]));
        let root = gtk::DrawingArea::new();
        root.set_content_height(90);
        root.set_hexpand(true);
        {
            let bins = bins.clone();
            root.set_draw_func(move |_, cr, w, h| draw(cr, w, h, &bins.borrow()));
        }
        Self { root, bins }
    }

    pub fn root(&self) -> &gtk::DrawingArea {
        &self.root
    }

    /// Recompute the histogram from a rendered preview.
    pub fn set_data(&self, rgba: &RgbaImage) {
        let mut b = [[0u32; 256]; 3];
        for px in rgba.pixels() {
            b[0][px[0] as usize] += 1;
            b[1][px[1] as usize] += 1;
            b[2][px[2] as usize] += 1;
        }
        *self.bins.borrow_mut() = b;
        self.root.queue_draw();
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

fn draw(cr: &cairo::Context, w: i32, h: i32, bins: &[[u32; 256]; 3]) {
    if w <= 0 || h <= 0 {
        return;
    }
    let (wf, hf) = (w as f64, h as f64);
    cr.set_source_rgb(0.10, 0.10, 0.11);
    let _ = cr.paint();

    // sqrt scaling so small counts stay visible; ignore pure 0/255 spikes when
    // finding the peak so a clipped channel doesn't flatten everything.
    let peak = bins
        .iter()
        .flat_map(|c| c[1..255].iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let scale = |v: u32| (v as f64).sqrt() / peak.sqrt();

    let colors = [(0.90, 0.25, 0.25), (0.25, 0.85, 0.30), (0.35, 0.55, 1.0)];
    for (ch, col) in colors.iter().enumerate() {
        cr.set_source_rgba(col.0, col.1, col.2, 0.5);
        cr.move_to(0.0, hf);
        for i in 0..256 {
            let x = i as f64 / 255.0 * wf;
            let y = hf - scale(bins[ch][i]).min(1.0) * hf;
            cr.line_to(x, y);
        }
        cr.line_to(wf, hf);
        cr.close_path();
        let _ = cr.fill();
    }
}
