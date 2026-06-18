//! Editor canvas: a `gtk::Picture` positioned in a `gtk::Fixed`, wrapped in a
//! `gtk::ScrolledWindow` (policy `Never`).
//!
//! Not a relm4 component — the AppModel owns this value and calls `set_texture`
//! directly. The `Fixed` gives us absolute positioning (manual zoom/pan); the
//! mouse wheel is ours alone (capture-phase controller): it always zooms toward
//! the cursor, never scrolls. Panning is drag-only.
//!
//! The `ScrolledWindow` exists only to stop size propagation: a bare `Fixed`
//! reports its (scaled) child's size as its natural size, which bubbles up the
//! `Paned` and grows the window when you zoom in. `ScrolledWindow` does not
//! propagate child natural size, so the window stays fixed; we force the inner
//! `Fixed` to exactly the viewport size each layout and clip (overflow Hidden).
//!
//! Display is GPU-accelerated end to end: wgpu renders, the result becomes a
//! `gdk::MemoryTexture`, GTK paints it via GSK (GL/Vulkan).
//!
//! // ponytail: no live re-fit on window resize (only on open); add a resize
//! // hook if "fit" should track the window continuously.

use std::cell::Cell;
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

use crate::settings::Background;

const ZOOM_MIN: f64 = 0.002;
const ZOOM_MAX: f64 = 20.0;
const ZOOM_STEP: f64 = 1.1;

/// Shared zoom/pan state (Rc<Cell> so it can be moved into event closures).
#[derive(Clone)]
struct View {
    /// Natural (unscaled) texture size in pixels.
    natural: Rc<Cell<(i32, i32)>>,
    /// Absolute scale: on-screen px = natural px * scale (1.0 = 100%).
    scale: Rc<Cell<f64>>,
    /// Top-left of the picture within the Fixed, in widget px.
    offset: Rc<Cell<(f64, f64)>>,
    /// Last pointer position within the Fixed (for zoom-to-cursor).
    cursor: Rc<Cell<(f64, f64)>>,
    /// True until the user zooms/pans: keeps the image fit+centered.
    fit: Rc<Cell<bool>>,
}

pub struct EditorCanvas {
    root: gtk::ScrolledWindow,
    fixed: gtk::Fixed,
    picture: gtk::Picture,
    view: View,
}

impl EditorCanvas {
    pub fn new() -> Self {
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Fill); // exact size set below; aspect preserved

        let fixed = gtk::Fixed::new();
        fixed.set_overflow(gtk::Overflow::Hidden);
        fixed.put(&picture, 0.0, 0.0);

        // ScrolledWindow caps natural-size propagation so zooming never resizes
        // the window (and the window stays manually resizable). `External` hides
        // its scrollbars and, unlike `Never`, does NOT request the child's full
        // size — so a zoomed (huge) picture is clipped, not grown into. We never
        // actually scroll it (adjustments stay 0; we pan via the Fixed).
        let root = gtk::ScrolledWindow::new();
        root.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        root.set_min_content_width(0);
        root.set_min_content_height(0);
        root.set_has_frame(false);
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_child(Some(&fixed));
        install_bg_css();

        let view = View {
            natural: Rc::new(Cell::new((0, 0))),
            scale: Rc::new(Cell::new(1.0)),
            offset: Rc::new(Cell::new((0.0, 0.0))),
            cursor: Rc::new(Cell::new((0.0, 0.0))),
            fit: Rc::new(Cell::new(true)),
        };

        // Track the pointer so the wheel can zoom toward it.
        let motion = gtk::EventControllerMotion::new();
        {
            let view = view.clone();
            motion.connect_motion(move |_, x, y| view.cursor.set((x, y)));
        }
        fixed.add_controller(motion);

        // Wheel = zoom toward cursor. Capture phase + Stop so nothing else
        // (kinetic scroll, etc.) ever sees it.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let picture = picture.clone();
            let fixed_w = fixed.clone();
            let view = view.clone();
            scroll.connect_scroll(move |_, _dx, dy| {
                let (nw, nh) = view.natural.get();
                if nw > 0 && nh > 0 {
                    let old = view.scale.get();
                    let factor = if dy < 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                    let new = (old * factor).clamp(ZOOM_MIN, ZOOM_MAX);
                    // Keep the image point under the cursor stationary.
                    let (cx, cy) = view.cursor.get();
                    let (ox, oy) = view.offset.get();
                    let ratio = new / old;
                    view.offset
                        .set((cx - (cx - ox) * ratio, cy - (cy - oy) * ratio));
                    view.scale.set(new);
                    view.fit.set(false);
                    apply(&picture, &fixed_w, &view);
                }
                glib::Propagation::Stop
            });
        }
        fixed.add_controller(scroll);

        // Drag = pan.
        let drag = gtk::GestureDrag::new();
        {
            let start = Rc::new(Cell::new((0.0, 0.0)));
            {
                let view = view.clone();
                let start = start.clone();
                drag.connect_drag_begin(move |_, _x, _y| start.set(view.offset.get()));
            }
            {
                let picture = picture.clone();
                let fixed_w = fixed.clone();
                let view = view.clone();
                let start = start.clone();
                drag.connect_drag_update(move |_, dx, dy| {
                    let (sx, sy) = start.get();
                    view.offset.set((sx + dx, sy + dy));
                    view.fit.set(false);
                    apply(&picture, &fixed_w, &view);
                });
            }
        }
        fixed.add_controller(drag);

        Self {
            root,
            fixed,
            picture,
            view,
        }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Set the canvas background: themed default, or a solid white/black.
    pub fn set_background(&self, bg: Background) {
        self.root.remove_css_class("editor-bg-white");
        self.root.remove_css_class("editor-bg-black");
        match bg {
            Background::White => self.root.add_css_class("editor-bg-white"),
            Background::Black => self.root.add_css_class("editor-bg-black"),
            Background::Default => {}
        }
    }

    /// Swap in a new preview of the SAME image without changing the view:
    /// keeps zoom/pan, compensating if the preview resolution differs.
    pub fn update_texture(&self, texture: &gdk::MemoryTexture) {
        let (nw, nh) = (texture.width(), texture.height());
        let (onw, _) = self.view.natural.get();
        if onw > 0 && nw > 0 {
            // Keep the on-screen size (natural*scale) constant.
            self.view
                .scale
                .set(self.view.scale.get() * onw as f64 / nw as f64);
        }
        self.view.natural.set((nw, nh));
        self.picture.set_paintable(Some(texture));
        apply(&self.picture, &self.fixed, &self.view);
    }

    /// Show an image, fit + centered in the viewport.
    pub fn set_texture(&self, texture: &gdk::MemoryTexture) {
        self.view.natural.set((texture.width(), texture.height()));
        self.picture.set_paintable(Some(texture));
        self.view.fit.set(true);
        fit_now(&self.picture, &self.fixed, &self.view);

        // The Fixed may not be allocated yet on first open (size 0); re-fit once
        // its real size lands so the initial image is correctly centered (not
        // pinned to an edge). Self-terminating: stops once fitted or the user
        // takes over the view.
        let picture = self.picture.clone();
        let fixed = self.fixed.clone();
        let view = self.view.clone();
        self.fixed.add_tick_callback(move |w, _| {
            if !view.fit.get() {
                return glib::ControlFlow::Break;
            }
            if w.width() > 0 && w.height() > 0 {
                fit_now(&picture, &fixed, &view);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }
}

impl Default for EditorCanvas {
    fn default() -> Self {
        Self::new()
    }
}

/// Install the canvas background CSS once for the default display.
fn install_bg_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(
            ".editor-bg-white { background-color: #ffffff; } \
             .editor-bg-black { background-color: #000000; }",
        );
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

/// Compute and apply the fit-to-viewport scale, centered.
fn fit_now(picture: &gtk::Picture, fixed: &gtk::Fixed, view: &View) {
    let (nw, nh) = view.natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    let (vw, vh) = viewport(fixed);
    let s = (vw / nw as f64).min(vh / nh as f64);
    view.scale.set(s);
    view.offset
        .set(((vw - nw as f64 * s) / 2.0, (vh - nh as f64 * s) / 2.0));
    apply(picture, fixed, view);
}

/// Viewport size = the ScrolledWindow's allocation (the Fixed is forced to match
/// in `apply`). Falls back to the Fixed's own size before that's happened.
fn viewport(fixed: &gtk::Fixed) -> (f64, f64) {
    let w = fixed
        .parent()
        .and_then(|vp| vp.parent()) // Fixed -> Viewport -> ScrolledWindow
        .map(|sw| sw.width())
        .filter(|&w| w > 0)
        .unwrap_or_else(|| fixed.width());
    let h = fixed
        .parent()
        .and_then(|vp| vp.parent())
        .map(|sw| sw.height())
        .filter(|&h| h > 0)
        .unwrap_or_else(|| fixed.height());
    (w.max(1) as f64, h.max(1) as f64)
}

/// Size and position the Picture from the current scale/offset. Pins the Fixed
/// to the viewport size so the wheel/cursor coordinate space matches.
fn apply(picture: &gtk::Picture, fixed: &gtk::Fixed, view: &View) {
    let (nw, nh) = view.natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    let (vw, vh) = viewport(fixed);
    fixed.set_size_request(vw as i32, vh as i32);
    let s = view.scale.get();
    let w = ((nw as f64) * s).round() as i32;
    let h = ((nh as f64) * s).round() as i32;
    picture.set_size_request(w.max(1), h.max(1));
    let (ox, oy) = view.offset.get();
    fixed.move_(picture, ox, oy);
}
