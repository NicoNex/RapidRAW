//! Editor canvas: a `gtk::Picture` positioned in a `gtk::Fixed`.
//!
//! Not a relm4 component — the AppModel owns this value and calls `set_texture`
//! directly. Using a `Fixed` (rather than a `ScrolledWindow`) means the mouse
//! wheel is ours alone: it always zooms (toward the cursor) and never scrolls.
//! Panning is drag-only. Display is GPU-accelerated end to end: wgpu renders,
//! the result becomes a `gdk::MemoryTexture`, GTK paints it via GSK (GL/Vulkan).
//!
//! // ponytail: no live re-fit on window resize (only on open); add a resize
//! // hook if "fit" should track the window continuously.

use std::cell::Cell;
use std::rc::Rc;

use gtk::gdk;
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
    root: gtk::Fixed,
    picture: gtk::Picture,
    view: View,
}

impl EditorCanvas {
    pub fn new() -> Self {
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Fill); // exact size set below; aspect preserved

        let root = gtk::Fixed::new();
        root.set_overflow(gtk::Overflow::Hidden);
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.put(&picture, 0.0, 0.0);
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
        root.add_controller(motion);

        // Wheel = zoom toward cursor. Capture phase + Stop so nothing else
        // (kinetic scroll, etc.) ever sees it.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let picture = picture.clone();
            let root_w = root.clone();
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
                    apply(&picture, &root_w, &view);
                }
                gtk::glib::Propagation::Stop
            });
        }
        root.add_controller(scroll);

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
                let root_w = root.clone();
                let view = view.clone();
                let start = start.clone();
                drag.connect_drag_update(move |_, dx, dy| {
                    let (sx, sy) = start.get();
                    view.offset.set((sx + dx, sy + dy));
                    view.fit.set(false);
                    apply(&picture, &root_w, &view);
                });
            }
        }
        root.add_controller(drag);

        Self {
            root,
            picture,
            view,
        }
    }

    pub fn root(&self) -> &gtk::Fixed {
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

    /// Show an image, fit + centered in the viewport.
    pub fn set_texture(&self, texture: &gdk::MemoryTexture) {
        self.view.natural.set((texture.width(), texture.height()));
        self.picture.set_paintable(Some(texture));
        self.view.fit.set(true);
        fit_now(&self.picture, &self.root, &self.view);

        // The Fixed may not be allocated yet on first open (size 0); re-fit once
        // after layout settles so the initial image lands centered.
        let picture = self.picture.clone();
        let root = self.root.clone();
        let view = self.view.clone();
        gtk::glib::idle_add_local_once(move || {
            if view.fit.get() {
                fit_now(&picture, &root, &view);
            }
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
fn fit_now(picture: &gtk::Picture, root: &gtk::Fixed, view: &View) {
    let (nw, nh) = view.natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    let vw = root.width().max(1) as f64;
    let vh = root.height().max(1) as f64;
    let s = (vw / nw as f64).min(vh / nh as f64);
    view.scale.set(s);
    view.offset
        .set(((vw - nw as f64 * s) / 2.0, (vh - nh as f64 * s) / 2.0));
    apply(picture, root, view);
}

/// Size and position the Picture from the current scale/offset.
fn apply(picture: &gtk::Picture, root: &gtk::Fixed, view: &View) {
    let (nw, nh) = view.natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    let s = view.scale.get();
    let w = ((nw as f64) * s).round() as i32;
    let h = ((nh as f64) * s).round() as i32;
    picture.set_size_request(w.max(1), h.max(1));
    let (ox, oy) = view.offset.get();
    root.move_(picture, ox, oy);
}
