//! Editor canvas: a `gtk::Picture` wrapped in a `gtk::ScrolledWindow` with
//! scroll-to-zoom and drag-to-pan.
//!
//! This is intentionally *not* a relm4 component. The AppModel is a single
//! `Component`, and the editor page is plain GTK widgets owned by an
//! `EditorCanvas` value that the model holds. The widget is added into the
//! Stack's "editor" page from `init`, and the model talks to the canvas
//! directly (`set_texture`) from `update_cmd`.
//!
//! Display is GPU-accelerated end to end: the engine renders on wgpu, the
//! result is uploaded into a `gdk::MemoryTexture`, and GTK paints it through
//! its GL/Vulkan (GSK) renderer.
//!
//! // ponytail: zoom/pan via ScrolledWindow + size_request is the cheap path;
//! // the wgpu->CPU->gdk readback round-trip is the known non-zero-copy
//! // ceiling — switch to a GLArea/dmabuf bridge only if preview latency hurts.

use std::cell::Cell;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;

const ZOOM_MIN: f64 = 0.05;
const ZOOM_MAX: f64 = 20.0;
const ZOOM_STEP: f64 = 1.1;

/// Owns the editor-page widget tree (a `ScrolledWindow` containing a
/// `Picture`) plus the zoom/pan state.
pub struct EditorCanvas {
    root: gtk::ScrolledWindow,
    picture: gtk::Picture,
    /// Natural (unscaled) pixel size of the current texture.
    natural: Rc<Cell<(i32, i32)>>,
    /// Current zoom factor (only meaningful when `fit` is false).
    zoom: Rc<Cell<f64>>,
    /// When true the image is scaled to fit the viewport (whole image visible);
    /// the first scroll switches to explicit zoom starting from the fit scale.
    fit: Rc<Cell<bool>>,
}

impl EditorCanvas {
    pub fn new() -> Self {
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_halign(gtk::Align::Center);
        picture.set_valign(gtk::Align::Center);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hexpand(true);
        scrolled.set_child(Some(&picture));

        let natural = Rc::new(Cell::new((0, 0)));
        let zoom = Rc::new(Cell::new(1.0_f64));
        let fit = Rc::new(Cell::new(true));

        // Re-fit when the viewport size changes (e.g. window resize, first
        // allocation). The adjustments' page_size tracks the viewport.
        for adj in [scrolled.hadjustment(), scrolled.vadjustment()] {
            let picture = picture.clone();
            let scrolled_w = scrolled.clone();
            let natural = natural.clone();
            let zoom = zoom.clone();
            let fit = fit.clone();
            adj.connect_changed(move |_| {
                relayout(&picture, &scrolled_w, &natural, &zoom, &fit);
            });
        }

        // --- Zoom: scroll wheel scales the image. ---
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        {
            let picture = picture.clone();
            let scrolled_w = scrolled.clone();
            let natural = natural.clone();
            let zoom = zoom.clone();
            let fit = fit.clone();
            scroll.connect_scroll(move |_, _dx, dy| {
                // Leaving fit-mode: seed zoom with the current fit scale so the
                // first wheel step continues smoothly from what's on screen.
                if fit.get() {
                    zoom.set(current_fit_scale(&scrolled_w, &natural).unwrap_or(1.0));
                    fit.set(false);
                }
                let factor = if dy < 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                zoom.set((zoom.get() * factor).clamp(ZOOM_MIN, ZOOM_MAX));
                relayout(&picture, &scrolled_w, &natural, &zoom, &fit);
                gtk::glib::Propagation::Stop
            });
        }
        scrolled.add_controller(scroll);

        // --- Pan: drag adjusts the ScrolledWindow's scroll adjustments. ---
        let drag = gtk::GestureDrag::new();
        {
            let scrolled_w = scrolled.clone();
            let start = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
            {
                let scrolled_w = scrolled_w.clone();
                let start = start.clone();
                drag.connect_drag_begin(move |_, _x, _y| {
                    start.set((
                        scrolled_w.hadjustment().value(),
                        scrolled_w.vadjustment().value(),
                    ));
                });
            }
            {
                let scrolled_w = scrolled_w.clone();
                let start = start.clone();
                drag.connect_drag_update(move |_, ox, oy| {
                    let (h0, v0) = start.get();
                    scrolled_w.hadjustment().set_value(h0 - ox);
                    scrolled_w.vadjustment().set_value(v0 - oy);
                });
            }
        }
        scrolled.add_controller(drag);

        Self {
            root: scrolled,
            picture,
            natural,
            zoom,
            fit,
        }
    }

    /// The widget to insert into the Stack's "editor" page.
    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Display an image. Resets to fit-the-window so the whole image is visible.
    pub fn set_texture(&self, texture: &gdk::MemoryTexture) {
        self.natural.set((texture.width(), texture.height()));
        self.fit.set(true);
        self.zoom.set(1.0);
        self.picture.set_paintable(Some(texture));
        relayout(&self.picture, &self.root, &self.natural, &self.zoom, &self.fit);
    }
}

impl Default for EditorCanvas {
    fn default() -> Self {
        Self::new()
    }
}

/// Scale that fits the natural image fully inside the current viewport.
fn current_fit_scale(
    scrolled: &gtk::ScrolledWindow,
    natural: &Rc<Cell<(i32, i32)>>,
) -> Option<f64> {
    let (nw, nh) = natural.get();
    if nw <= 0 || nh <= 0 {
        return None;
    }
    let vw = scrolled.width().max(1) as f64;
    let vh = scrolled.height().max(1) as f64;
    Some((vw / nw as f64).min(vh / nh as f64))
}

/// Size the Picture: fit-to-viewport when `fit`, else natural * zoom. A
/// shrinkable Picture in a ScrolledWindow collapses to 0x0 without an explicit
/// size request, so we always set one.
fn relayout(
    picture: &gtk::Picture,
    scrolled: &gtk::ScrolledWindow,
    natural: &Rc<Cell<(i32, i32)>>,
    zoom: &Rc<Cell<f64>>,
    fit: &Rc<Cell<bool>>,
) {
    let (nw, nh) = natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    let scale = if fit.get() {
        match current_fit_scale(scrolled, natural) {
            Some(s) => s,
            None => return,
        }
    } else {
        zoom.get()
    };
    let w = ((nw as f64) * scale).round() as i32;
    let h = ((nh as f64) * scale).round() as i32;
    picture.set_size_request(w.max(1), h.max(1));
}
