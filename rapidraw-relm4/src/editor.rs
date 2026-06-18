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

use gtk::cairo;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

use crate::settings::Background;

/// Which part of the crop rectangle a drag is manipulating.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    None,
    Move,
    N,
    S,
    E,
    W,
    Nw,
    Ne,
    Sw,
    Se,
}

/// Hit-test radius (px) for crop handles.
const HANDLE: f64 = 14.0;
const MIN_CROP: f64 = 0.04;

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
    /// Outer overlay: the scrolled canvas plus the crop layer on top.
    root: gtk::Overlay,
    /// The scrolled window holding the Fixed+Picture (the actual image view).
    sw: gtk::ScrolledWindow,
    fixed: gtk::Fixed,
    picture: gtk::Picture,
    overlay: gtk::DrawingArea,
    view: View,
    /// Crop rectangle, normalized (x, y, w, h) in image space.
    crop_rect: Rc<Cell<(f64, f64, f64, f64)>>,
    /// Crop aspect (output w/h); 0 = free.
    crop_aspect: Rc<Cell<f64>>,
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
        let sw = gtk::ScrolledWindow::new();
        sw.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        sw.set_min_content_width(0);
        sw.set_min_content_height(0);
        sw.set_has_frame(false);
        sw.set_hexpand(true);
        sw.set_vexpand(true);
        sw.set_child(Some(&fixed));
        install_bg_css();

        let view = View {
            natural: Rc::new(Cell::new((0, 0))),
            scale: Rc::new(Cell::new(1.0)),
            offset: Rc::new(Cell::new((0.0, 0.0))),
            cursor: Rc::new(Cell::new((0.0, 0.0))),
            fit: Rc::new(Cell::new(true)),
        };

        // Crop overlay: a DrawingArea layered over the canvas (auto-fills, so it
        // tracks window resizes), shown only in crop mode.
        let overlay = gtk::DrawingArea::new();
        overlay.set_visible(false);
        let crop_rect = Rc::new(Cell::new((0.0, 0.0, 1.0, 1.0)));
        let crop_aspect = Rc::new(Cell::new(0.0_f64));

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
            let overlay_w = overlay.clone();
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
                    apply(&picture, &fixed_w, &overlay_w, &view);
                }
                glib::Propagation::Stop
            });
        }
        fixed.add_controller(scroll);

        // Drag = pan (disabled in crop mode, where the overlay handles drags).
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
                let overlay_w = overlay.clone();
                let view = view.clone();
                let start = start.clone();
                drag.connect_drag_update(move |_, dx, dy| {
                    if overlay_w.is_visible() {
                        return;
                    }
                    let (sx, sy) = start.get();
                    view.offset.set((sx + dx, sy + dy));
                    view.fit.set(false);
                    apply(&picture, &fixed_w, &overlay_w, &view);
                });
            }
        }
        fixed.add_controller(drag);

        // Crop overlay draw + drag: dimmed exterior, rectangle, thirds grid and
        // handles; drag to move/resize the crop.
        {
            let view = view.clone();
            let crop_rect = crop_rect.clone();
            overlay.set_draw_func(move |_, cr, w, h| {
                draw_crop(cr, w, h, &view, crop_rect.get());
            });
        }
        let crop_drag = gtk::GestureDrag::new();
        {
            let region = Rc::new(Cell::new(Region::None));
            let start = Rc::new(Cell::new((0.0, 0.0, 1.0, 1.0)));
            {
                let view = view.clone();
                let crop_rect = crop_rect.clone();
                let region = region.clone();
                let start = start.clone();
                crop_drag.connect_drag_begin(move |g, x, y| {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    start.set(crop_rect.get());
                    region.set(hit_region(&view, crop_rect.get(), x, y));
                });
            }
            {
                let view = view.clone();
                let overlay_w = overlay.clone();
                let crop_rect = crop_rect.clone();
                let crop_aspect = crop_aspect.clone();
                let region = region.clone();
                let start = start.clone();
                crop_drag.connect_drag_update(move |_, dx, dy| {
                    let (nw, nh) = view.natural.get();
                    if nw <= 0 || nh <= 0 {
                        return;
                    }
                    let s = view.scale.get();
                    let (dnx, dny) = (dx / (nw as f64 * s), dy / (nh as f64 * s));
                    let r = resize_crop(
                        start.get(),
                        region.get(),
                        dnx,
                        dny,
                        crop_aspect.get(),
                        nw as f64,
                        nh as f64,
                    );
                    crop_rect.set(r);
                    overlay_w.queue_draw();
                });
            }
        }
        overlay.add_controller(crop_drag);

        // In crop mode the overlay is mapped and resizes with the window; re-fit
        // the image so it stays centred in its area (only fires while visible).
        {
            let picture = picture.clone();
            let fixed = fixed.clone();
            let view = view.clone();
            let overlay_w = overlay.clone();
            overlay.connect_resize(move |_, _, _| {
                if overlay_w.is_visible() {
                    view.fit.set(true);
                    fit_now(&picture, &fixed, &overlay_w, &view);
                }
            });
        }

        // Outer overlay: scrolled image + crop layer (the layer fills the canvas).
        let root = gtk::Overlay::new();
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_child(Some(&sw));
        root.add_overlay(&overlay);

        Self {
            root,
            sw,
            fixed,
            picture,
            overlay,
            view,
            crop_rect,
            crop_aspect,
        }
    }

    pub fn root(&self) -> &gtk::Overlay {
        &self.root
    }

    /// Set the canvas background: themed default, or a solid white/black.
    pub fn set_background(&self, bg: Background) {
        self.sw.remove_css_class("editor-bg-white");
        self.sw.remove_css_class("editor-bg-black");
        match bg {
            Background::White => self.sw.add_css_class("editor-bg-white"),
            Background::Black => self.sw.add_css_class("editor-bg-black"),
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
        apply(&self.picture, &self.fixed, &self.overlay, &self.view);
    }

    /// Drop the current image (blank canvas) — e.g. while the next one decodes,
    /// so the previous photo isn't shown under the new selection.
    pub fn clear(&self) {
        self.picture.set_paintable(None::<&gdk::Paintable>);
        self.view.natural.set((0, 0));
    }

    /// Show an image, fit + centered in the viewport.
    pub fn set_texture(&self, texture: &gdk::MemoryTexture) {
        self.view.natural.set((texture.width(), texture.height()));
        self.picture.set_paintable(Some(texture));
        self.view.fit.set(true);
        fit_now(&self.picture, &self.fixed, &self.overlay, &self.view);

        // The Fixed may not be allocated yet on first open (size 0); re-fit once
        // its real size lands so the initial image is correctly centered (not
        // pinned to an edge). Self-terminating: stops once fitted or the user
        // takes over the view.
        let picture = self.picture.clone();
        let fixed = self.fixed.clone();
        let overlay = self.overlay.clone();
        let view = self.view.clone();
        self.fixed.add_tick_callback(move |w, _| {
            if !view.fit.get() {
                return glib::ControlFlow::Break;
            }
            if w.width() > 0 && w.height() > 0 {
                fit_now(&picture, &fixed, &overlay, &view);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    /// Enter crop mode: show the overlay with the current crop rect (constrained
    /// to `aspect`, 0 = free).
    pub fn enter_crop(&self, aspect: f64) {
        self.crop_aspect.set(aspect);
        if aspect > 0.0 {
            self.crop_rect.set(fit_aspect(self.crop_rect.get(), aspect, &self.view));
        }
        // Fit the whole image so the crop rectangle is fully reachable.
        self.view.fit.set(true);
        fit_now(&self.picture, &self.fixed, &self.overlay, &self.view);
        self.overlay.set_visible(true);
        self.overlay.queue_draw();
    }

    /// Leave crop mode; return the final crop rect (normalized x,y,w,h).
    pub fn exit_crop(&self) -> (f64, f64, f64, f64) {
        self.overlay.set_visible(false);
        self.crop_rect.get()
    }

    /// Change the crop aspect constraint while in crop mode.
    pub fn set_crop_aspect(&self, aspect: f64) {
        self.crop_aspect.set(aspect);
        if aspect > 0.0 {
            self.crop_rect.set(fit_aspect(self.crop_rect.get(), aspect, &self.view));
        }
        self.overlay.queue_draw();
    }

    /// Reset the crop to the full image.
    pub fn reset_crop(&self) {
        self.crop_rect.set((0.0, 0.0, 1.0, 1.0));
        self.crop_aspect.set(0.0);
        self.overlay.queue_draw();
    }

    /// Set the crop rectangle (normalized), e.g. restoring saved edits.
    pub fn set_crop_rect(&self, r: (f64, f64, f64, f64)) {
        self.crop_rect.set(r);
        self.overlay.queue_draw();
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
fn fit_now(picture: &gtk::Picture, fixed: &gtk::Fixed, overlay: &gtk::DrawingArea, view: &View) {
    let (nw, nh) = view.natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    let (vw, vh) = viewport(fixed);
    let s = (vw / nw as f64).min(vh / nh as f64);
    view.scale.set(s);
    view.offset
        .set(((vw - nw as f64 * s) / 2.0, (vh - nh as f64 * s) / 2.0));
    apply(picture, fixed, overlay, view);
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
/// (and the crop overlay) to the viewport size so the wheel/cursor coordinate
/// space matches.
fn apply(picture: &gtk::Picture, fixed: &gtk::Fixed, overlay: &gtk::DrawingArea, view: &View) {
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
    // The crop overlay is a separate layer that auto-fills; just repaint it.
    overlay.queue_draw();
}

/// Image rect (full picture) on screen, in widget px: (x, y, w, h).
fn image_screen_rect(view: &View) -> (f64, f64, f64, f64) {
    let (nw, nh) = view.natural.get();
    let s = view.scale.get();
    let (ox, oy) = view.offset.get();
    (ox, oy, nw as f64 * s, nh as f64 * s)
}

/// Crop rect (normalized) -> screen px rect.
fn crop_screen_rect(view: &View, r: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
    let (ix, iy, iw, ih) = image_screen_rect(view);
    (ix + r.0 * iw, iy + r.1 * ih, r.2 * iw, r.3 * ih)
}

/// Which crop region the pointer (px,py) is over.
fn hit_region(view: &View, r: (f64, f64, f64, f64), px: f64, py: f64) -> Region {
    let (x, y, w, h) = crop_screen_rect(view, r);
    let near = |a: f64, b: f64| (a - b).abs() <= HANDLE;
    let (l, t, right, bottom) = (x, y, x + w, y + h);
    let on_l = near(px, l) && py >= t - HANDLE && py <= bottom + HANDLE;
    let on_r = near(px, right) && py >= t - HANDLE && py <= bottom + HANDLE;
    let on_t = near(py, t) && px >= l - HANDLE && px <= right + HANDLE;
    let on_b = near(py, bottom) && px >= l - HANDLE && px <= right + HANDLE;
    match (on_t, on_b, on_l, on_r) {
        (true, _, true, _) => Region::Nw,
        (true, _, _, true) => Region::Ne,
        (_, true, true, _) => Region::Sw,
        (_, true, _, true) => Region::Se,
        (true, _, _, _) => Region::N,
        (_, true, _, _) => Region::S,
        (_, _, true, _) => Region::W,
        (_, _, _, true) => Region::E,
        _ if px >= l && px <= right && py >= t && py <= bottom => Region::Move,
        _ => Region::None,
    }
}

/// Apply a drag delta (normalized) to the crop rect for `region`, clamped to
/// [0,1] with a minimum size, optionally enforcing `aspect` (output w/h) on
/// corner drags.
fn resize_crop(
    start: (f64, f64, f64, f64),
    region: Region,
    dnx: f64,
    dny: f64,
    aspect: f64,
    nw: f64,
    nh: f64,
) -> (f64, f64, f64, f64) {
    let (mut x, mut y, mut w, mut h) = start;
    let (l, t, r, b) = (x, y, x + w, y + h);
    match region {
        Region::Move => {
            x = (x + dnx).clamp(0.0, 1.0 - w);
            y = (y + dny).clamp(0.0, 1.0 - h);
        }
        Region::N => {
            let nt = (t + dny).min(b - MIN_CROP).max(0.0);
            y = nt;
            h = b - nt;
        }
        Region::S => {
            let nb = (b + dny).max(t + MIN_CROP).min(1.0);
            h = nb - t;
        }
        Region::W => {
            let nl = (l + dnx).min(r - MIN_CROP).max(0.0);
            x = nl;
            w = r - nl;
        }
        Region::E => {
            let nr = (r + dnx).max(l + MIN_CROP).min(1.0);
            w = nr - l;
        }
        Region::Nw | Region::Ne | Region::Sw | Region::Se => {
            let nl = if matches!(region, Region::Nw | Region::Sw) {
                (l + dnx).clamp(0.0, r - MIN_CROP)
            } else {
                l
            };
            let nr = if matches!(region, Region::Ne | Region::Se) {
                (r + dnx).clamp(l + MIN_CROP, 1.0)
            } else {
                r
            };
            let nt = if matches!(region, Region::Nw | Region::Ne) {
                (t + dny).clamp(0.0, b - MIN_CROP)
            } else {
                t
            };
            let nb = if matches!(region, Region::Sw | Region::Se) {
                (b + dny).clamp(t + MIN_CROP, 1.0)
            } else {
                b
            };
            x = nl;
            y = nt;
            w = nr - nl;
            h = nb - nt;
        }
        Region::None => {}
    }
    let mut rect = (x, y, w, h);
    // Enforce aspect on corner drags by anchoring the opposite corner.
    if aspect > 0.0 && matches!(region, Region::Nw | Region::Ne | Region::Sw | Region::Se) {
        rect = enforce_aspect_corner(rect, region, aspect, nw, nh);
    }
    rect
}

/// Adjust a corner-dragged rect so its pixel aspect (w*nw / h*nh) == `aspect`,
/// anchoring the corner opposite to `region`.
fn enforce_aspect_corner(
    r: (f64, f64, f64, f64),
    region: Region,
    aspect: f64,
    nw: f64,
    nh: f64,
) -> (f64, f64, f64, f64) {
    let (mut x, mut y, mut w, mut h) = r;
    // Desired normalized height for the current width: w*nw/(h*nh) = aspect.
    let new_h = (w * nw) / (aspect * nh);
    let anchor_bottom = matches!(region, Region::Nw | Region::Ne); // top edge moved
    let anchor_right = matches!(region, Region::Nw | Region::Sw); // left edge moved
    let bottom = y + h;
    let right = x + w;
    h = new_h.clamp(MIN_CROP, 1.0);
    if anchor_bottom {
        y = bottom - h;
    }
    if y < 0.0 {
        y = 0.0;
        h = bottom - y;
        w = (h * aspect * nh) / nw;
        if anchor_right {
            x = right - w;
        }
    }
    if y + h > 1.0 {
        h = 1.0 - y;
        w = (h * aspect * nh) / nw;
        if anchor_right {
            x = right - w;
        }
    }
    (x.max(0.0), y.max(0.0), w, h)
}

/// Centre a crop of the given aspect within the current rect's bounds.
fn fit_aspect(r: (f64, f64, f64, f64), aspect: f64, view: &View) -> (f64, f64, f64, f64) {
    let (nw, nh) = view.natural.get();
    if nw <= 0 || nh <= 0 {
        return r;
    }
    let (nw, nh) = (nw as f64, nh as f64);
    // Largest rect of pixel-aspect `aspect` inside the full image, centred.
    let img_aspect = nw / nh;
    let (w, h) = if aspect > img_aspect {
        (1.0, img_aspect / aspect)
    } else {
        (aspect / img_aspect, 1.0)
    };
    ((1.0 - w) / 2.0, (1.0 - h) / 2.0, w, h)
}

/// Draw the crop overlay: dimmed exterior, border, rule-of-thirds, handles.
fn draw_crop(cr: &cairo::Context, w: i32, h: i32, view: &View, r: (f64, f64, f64, f64)) {
    let (nw, nh) = view.natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    let (cx, cy, cw, ch) = crop_screen_rect(view, r);

    // Dim everything, then clear the crop region back to transparent.
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
    cr.rectangle(0.0, 0.0, w as f64, h as f64);
    let _ = cr.fill();
    cr.save().ok();
    cr.set_operator(cairo::Operator::Clear);
    cr.rectangle(cx, cy, cw, ch);
    let _ = cr.fill();
    cr.restore().ok();

    // Rule-of-thirds grid.
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
    cr.set_line_width(1.0);
    for i in 1..3 {
        let gx = cx + cw * i as f64 / 3.0;
        cr.move_to(gx, cy);
        cr.line_to(gx, cy + ch);
        let gy = cy + ch * i as f64 / 3.0;
        cr.move_to(cx, gy);
        cr.line_to(cx + cw, gy);
    }
    let _ = cr.stroke();

    // Border.
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(1.5);
    cr.rectangle(cx, cy, cw, ch);
    let _ = cr.stroke();

    // Corner handles.
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let hs = 8.0;
    for (hx, hy) in [(cx, cy), (cx + cw, cy), (cx, cy + ch), (cx + cw, cy + ch)] {
        cr.rectangle(hx - hs / 2.0, hy - hs / 2.0, hs, hs);
        let _ = cr.fill();
    }
}
