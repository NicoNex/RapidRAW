//! Editor canvas: a `gtk::Picture` wrapped in a `gtk::ScrolledWindow` with
//! scroll-to-zoom and drag-to-pan.
//!
//! This is intentionally *not* a relm4 component. The AppModel is a single
//! `Component`, and the editor page is plain GTK widgets owned by an
//! `EditorCanvas` value that the model holds. The widget is added into the
//! Stack's "editor" page from `init`, and the model talks to the canvas
//! directly (`set_texture`) from `update_cmd`. This integrates more cleanly
//! than nesting a second component just to forward one texture.
//!
//! // ponytail: ScrolledWindow-based zoom is the cheapest path; revisit only
//! // if pixel-accurate 1:1 zoom is needed.

use std::cell::Cell;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;

const ZOOM_MIN: f64 = 0.05;
const ZOOM_MAX: f64 = 20.0;
const ZOOM_STEP: f64 = 1.1;

/// Owns the editor-page widget tree (a `ScrolledWindow` containing a
/// `Picture`) plus the zoom/pan state. The root widget is appended to the
/// Stack's "editor" page; `set_texture` swaps in a newly opened image.
pub struct EditorCanvas {
    /// Outer widget added to the Stack page.
    root: gtk::ScrolledWindow,
    picture: gtk::Picture,
    /// Natural (unscaled) pixel size of the current texture.
    natural: Rc<Cell<(i32, i32)>>,
    /// Current zoom factor, clamped to [ZOOM_MIN, ZOOM_MAX].
    zoom: Rc<Cell<f64>>,
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

        // --- Zoom: scroll wheel scales the Picture's size request. ---
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        {
            let picture = picture.clone();
            let natural = natural.clone();
            let zoom = zoom.clone();
            scroll.connect_scroll(move |_, _dx, dy| {
                // dy < 0 -> wheel up -> zoom in; dy > 0 -> zoom out.
                let factor = if dy < 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                let next = (zoom.get() * factor).clamp(ZOOM_MIN, ZOOM_MAX);
                zoom.set(next);
                apply_zoom(&picture, &natural, next);
                gtk::glib::Propagation::Stop
            });
        }
        scrolled.add_controller(scroll);

        // --- Pan: drag adjusts the ScrolledWindow's scroll adjustments. ---
        let drag = gtk::GestureDrag::new();
        {
            let scrolled_w = scrolled.clone();
            // Anchor adjustment values captured at drag-begin.
            let start = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
            {
                let scrolled_w = scrolled_w.clone();
                let start = start.clone();
                drag.connect_drag_begin(move |_, _x, _y| {
                    let h = scrolled_w.hadjustment().value();
                    let v = scrolled_w.vadjustment().value();
                    start.set((h, v));
                });
            }
            {
                let scrolled_w = scrolled_w.clone();
                let start = start.clone();
                drag.connect_drag_update(move |_, ox, oy| {
                    let (h0, v0) = start.get();
                    // Drag right -> content should follow the cursor -> scroll left.
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
        }
    }

    /// The widget to insert into the Stack's "editor" page.
    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Display a freshly opened image. Resets zoom to fit (1.0).
    pub fn set_texture(&self, texture: &gdk::MemoryTexture) {
        let w = texture.width();
        let h = texture.height();
        self.natural.set((w, h));
        self.zoom.set(1.0);
        self.picture.set_paintable(Some(texture));
        // At zoom 1.0 let content-fit drive sizing: clear any size request.
        self.picture.set_size_request(-1, -1);
    }
}

impl Default for EditorCanvas {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply `zoom` by setting an explicit size request on the Picture. At zoom
/// 1.0 we clear the request so `ContentFit::Contain` fits the image to the
/// viewport; above 1.0 the Picture grows past the viewport and the
/// ScrolledWindow provides pan.
fn apply_zoom(picture: &gtk::Picture, natural: &Rc<Cell<(i32, i32)>>, zoom: f64) {
    let (nw, nh) = natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    if (zoom - 1.0).abs() < f64::EPSILON {
        picture.set_size_request(-1, -1);
        return;
    }
    let w = ((nw as f64) * zoom).round() as i32;
    let h = ((nh as f64) * zoom).round() as i32;
    picture.set_size_request(w.max(1), h.max(1));
}
