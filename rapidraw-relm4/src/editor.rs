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

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::cairo;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

use crate::settings::Background;

/// A selected mask's geometric sub-mask, in normalized image coords (0..1), for
/// the canvas overlay. `sub` is the index into the mask's `sub_masks` (so a drag
/// maps back to the right one). Brush/flow/color/luminance have no shape.
#[derive(Clone, Copy, Debug)]
pub enum MaskShape {
    /// Centre + radii normalized; rotation in degrees.
    Radial {
        sub: usize,
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        rot: f64,
    },
    Linear {
        sub: usize,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
}

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

/// Callback invoked live during a mask-handle drag with the edited shape.
type MaskEditCb = Rc<RefCell<Option<Box<dyn Fn(MaskShape)>>>>;

/// Callback fired when a brush/flow stroke finishes: `(sub index, normalized
/// points, erase)`. The model denormalizes and appends a line to the sub-mask.
type PaintSink = Rc<RefCell<Option<Box<dyn Fn(usize, Vec<(f64, f64)>, bool)>>>>;

/// Armed brush/flow painting: which sub-mask, the brush radius (normalized to
/// image width, for the live preview), and whether it erases.
#[derive(Clone, Copy)]
struct PaintArm {
    sub: usize,
    size_norm: f64,
    erase: bool,
}

/// What a mask drag manipulates.
#[derive(Clone, Copy)]
enum MaskGrab {
    RadialMove,
    RadialResize,
    LinearStart,
    LinearEnd,
}

/// In-progress mask drag: which grab, the shape at press, and the press point in
/// normalized image coords.
#[derive(Clone, Copy)]
struct MaskDrag {
    grab: MaskGrab,
    orig: MaskShape,
    press: (f64, f64),
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
    /// Overlay drawing (and hit-testing) the selected mask's geometric shapes.
    mask_overlay: gtk::DrawingArea,
    mask_shapes: Rc<RefCell<Vec<MaskShape>>>,
    /// True while the Masks tab is active with a geometric mask selected — gates
    /// whether canvas drags edit a mask handle (vs. pan).
    mask_active: Rc<Cell<bool>>,
    /// Callback to the model with an edited shape (live, during drag).
    mask_edit: MaskEditCb,
    /// Armed brush/flow painting (drag paints a stroke instead of panning).
    paint: Rc<Cell<Option<PaintArm>>>,
    paint_sink: PaintSink,
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

        // Read-only mask overlay: draws the selected mask's radial/linear shapes.
        // `can_target(false)` so it never steals zoom/pan input (purely visual).
        let mask_overlay = gtk::DrawingArea::new();
        mask_overlay.set_visible(false);
        mask_overlay.set_can_target(false);
        let mask_shapes: Rc<RefCell<Vec<MaskShape>>> = Rc::new(RefCell::new(Vec::new()));
        let mask_active = Rc::new(Cell::new(false));
        let mask_edit: MaskEditCb = Rc::new(RefCell::new(None));
        let paint: Rc<Cell<Option<PaintArm>>> = Rc::new(Cell::new(None));
        let stroke: Rc<RefCell<Vec<(f64, f64)>>> = Rc::new(RefCell::new(Vec::new()));
        let paint_sink: PaintSink = Rc::new(RefCell::new(None));
        {
            let view = view.clone();
            let shapes = mask_shapes.clone();
            let paint = paint.clone();
            let stroke = stroke.clone();
            mask_overlay.set_draw_func(move |_, cr, _w, _h| {
                draw_masks(cr, &view, &shapes.borrow());
                draw_stroke(cr, &view, &stroke.borrow(), paint.get());
            });
        }
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
            let mask_w = mask_overlay.clone();
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
                    apply(&picture, &fixed_w, &overlay_w, &mask_w, &view);
                }
                glib::Propagation::Stop
            });
        }
        fixed.add_controller(scroll);

        // Drag = pan, except: in crop mode the crop overlay handles it, and in
        // masks mode a press on a mask handle edits that handle instead of panning.
        let drag = gtk::GestureDrag::new();
        {
            let start = Rc::new(Cell::new((0.0, 0.0)));
            let mask_drag: Rc<Cell<Option<MaskDrag>>> = Rc::new(Cell::new(None));
            // Widget point where a paint stroke began (to rebuild absolute points
            // from the gesture's offsets).
            let paint_start = Rc::new(Cell::new((0.0, 0.0)));
            {
                let view = view.clone();
                let start = start.clone();
                let mask_drag = mask_drag.clone();
                let mask_active = mask_active.clone();
                let shapes = mask_shapes.clone();
                let paint = paint.clone();
                let stroke = stroke.clone();
                let paint_start = paint_start.clone();
                let mask_w = mask_overlay.clone();
                drag.connect_drag_begin(move |_, x, y| {
                    start.set(view.offset.get());
                    // Painting takes precedence over handle-edit and pan.
                    if paint.get().is_some() {
                        paint_start.set((x, y));
                        let mut s = stroke.borrow_mut();
                        s.clear();
                        if let Some(p) = to_norm(&view, x, y) {
                            s.push(p);
                        }
                        mask_drag.set(None);
                        mask_w.queue_draw();
                        return;
                    }
                    mask_drag.set(if mask_active.get() {
                        hit_mask(&view, &shapes.borrow(), x, y)
                    } else {
                        None
                    });
                });
            }
            {
                let picture = picture.clone();
                let fixed_w = fixed.clone();
                let overlay_w = overlay.clone();
                let mask_w = mask_overlay.clone();
                let view = view.clone();
                let start = start.clone();
                let mask_drag = mask_drag.clone();
                let mask_edit = mask_edit.clone();
                let paint = paint.clone();
                let stroke = stroke.clone();
                let paint_start = paint_start.clone();
                let mask_w2 = mask_overlay.clone();
                drag.connect_drag_update(move |_, dx, dy| {
                    if overlay_w.is_visible() {
                        return;
                    }
                    // Painting a brush/flow stroke takes precedence.
                    if paint.get().is_some() {
                        let (bx, by) = paint_start.get();
                        if let Some(p) = to_norm(&view, bx + dx, by + dy) {
                            stroke.borrow_mut().push(p);
                            mask_w2.queue_draw();
                        }
                        return;
                    }
                    // Editing a mask handle takes precedence over panning.
                    if let Some(md) = mask_drag.get() {
                        let (_, _, iw, ih) = image_screen_rect(&view);
                        if iw <= 0.0 || ih <= 0.0 {
                            return;
                        }
                        let cur = (md.press.0 + dx / iw, md.press.1 + dy / ih);
                        let shape = apply_mask_drag(md, cur);
                        if let Some(cb) = mask_edit.borrow().as_ref() {
                            cb(shape);
                        }
                        return;
                    }
                    let (sx, sy) = start.get();
                    view.offset.set((sx + dx, sy + dy));
                    view.fit.set(false);
                    apply(&picture, &fixed_w, &overlay_w, &mask_w, &view);
                });
            }
            {
                let mask_drag = mask_drag.clone();
                let paint = paint.clone();
                let stroke = stroke.clone();
                let paint_sink = paint_sink.clone();
                let mask_w = mask_overlay.clone();
                drag.connect_drag_end(move |_, _, _| {
                    if let Some(arm) = paint.get() {
                        let pts = std::mem::take(&mut *stroke.borrow_mut());
                        if !pts.is_empty() {
                            if let Some(cb) = paint_sink.borrow().as_ref() {
                                cb(arm.sub, pts, arm.erase);
                            }
                        }
                        mask_w.queue_draw();
                    }
                    mask_drag.set(None);
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
            let mask_w = mask_overlay.clone();
            overlay.connect_resize(move |_, _, _| {
                if overlay_w.is_visible() {
                    view.fit.set(true);
                    fit_now(&picture, &fixed, &overlay_w, &mask_w, &view);
                }
            });
        }

        // Outer overlay: scrolled image + crop layer (the layer fills the canvas).
        let root = gtk::Overlay::new();
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_child(Some(&sw));
        root.add_overlay(&overlay);
        root.add_overlay(&mask_overlay);

        Self {
            root,
            sw,
            fixed,
            picture,
            overlay,
            mask_overlay,
            mask_shapes,
            mask_active,
            mask_edit,
            paint,
            paint_sink,
            view,
            crop_rect,
            crop_aspect,
        }
    }

    /// Show/update the mask overlay with `shapes` (normalized coords). `on` =
    /// Masks tab active; drives both visibility and whether drags edit handles.
    pub fn set_mask_overlay(&self, shapes: Vec<MaskShape>, on: bool) {
        let show = on && !shapes.is_empty();
        *self.mask_shapes.borrow_mut() = shapes;
        self.mask_active.set(show);
        self.mask_overlay.set_visible(show);
        self.mask_overlay.queue_draw();
    }

    /// Set the callback invoked (live, during a handle drag) with the edited
    /// shape; the model writes it back to the sub-mask's parameters.
    pub fn set_mask_editor(&self, cb: impl Fn(MaskShape) + 'static) {
        *self.mask_edit.borrow_mut() = Some(Box::new(cb));
    }

    /// Arm brush/flow painting into sub-mask `sub` with brush radius `size_norm`
    /// (normalized to image width) and `erase`; `None` disarms (drag pans again).
    pub fn set_paint(&self, arm: Option<(usize, f64, bool)>) {
        self.paint.set(arm.map(|(sub, size_norm, erase)| PaintArm {
            sub,
            size_norm,
            erase,
        }));
        // The live stroke draws on the mask overlay; brush/flow masks have no
        // radial/linear shape to keep it shown, so force it visible while armed.
        if arm.is_some() {
            self.mask_overlay.set_visible(true);
        }
        self.mask_overlay.queue_draw();
    }

    /// Set the callback fired when a brush/flow stroke finishes.
    pub fn set_paint_sink(&self, cb: impl Fn(usize, Vec<(f64, f64)>, bool) + 'static) {
        *self.paint_sink.borrow_mut() = Some(Box::new(cb));
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
        let old = self.view.natural.get();
        // The "keep on-screen size" compensation below only holds when this is
        // the same image at a different resolution. When the aspect changes the
        // swap is to a different framing — entering/leaving crop swaps the
        // cropped preview for the full image, a 90° rotation swaps w/h, before
        // /after swaps the cropped edit for the full original — and the
        // width-only scale factor would mis-scale (e.g. a 65:24 crop returning
        // to the full image appeared massively zoomed with no way to zoom out).
        // Re-fit instead.
        if !aspect_preserved(old, (nw, nh)) {
            self.set_texture(texture);
            return;
        }
        let (onw, _) = old;
        // Keep the on-screen size (natural*scale) constant.
        self.view
            .scale
            .set(self.view.scale.get() * onw as f64 / nw as f64);
        self.view.natural.set((nw, nh));
        self.picture.set_paintable(Some(texture));
        apply(&self.picture, &self.fixed, &self.overlay, &self.mask_overlay, &self.view);
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
        fit_now(&self.picture, &self.fixed, &self.overlay, &self.mask_overlay, &self.view);

        // The Fixed may not be allocated yet on first open (size 0); re-fit once
        // its real size lands so the initial image is correctly centered (not
        // pinned to an edge). Self-terminating: stops once fitted or the user
        // takes over the view.
        let picture = self.picture.clone();
        let fixed = self.fixed.clone();
        let overlay = self.overlay.clone();
        let mask = self.mask_overlay.clone();
        let view = self.view.clone();
        self.fixed.add_tick_callback(move |w, _| {
            if !view.fit.get() {
                return glib::ControlFlow::Break;
            }
            if w.width() > 0 && w.height() > 0 {
                fit_now(&picture, &fixed, &overlay, &mask, &view);
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
        fit_now(&self.picture, &self.fixed, &self.overlay, &self.mask_overlay, &self.view);
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

/// Whether a texture swap from `old` to `new` natural size keeps the image's
/// aspect ratio (within 1%). `update_texture`'s "preserve on-screen size" scale
/// compensation is only valid when it does; a changed aspect means a different
/// framing (crop toggle, 90° rotation, before/after on a cropped image) and we
/// must re-fit instead of preserving zoom. A missing prior size also re-fits.
fn aspect_preserved(old: (i32, i32), new: (i32, i32)) -> bool {
    let (ow, oh) = old;
    let (nw, nh) = new;
    if ow <= 0 || oh <= 0 || nw <= 0 || nh <= 0 {
        return false;
    }
    let oa = ow as f64 / oh as f64;
    let na = nw as f64 / nh as f64;
    (oa - na).abs() <= oa * 0.01
}

/// Compute and apply the fit-to-viewport scale, centered.
fn fit_now(
    picture: &gtk::Picture,
    fixed: &gtk::Fixed,
    overlay: &gtk::DrawingArea,
    mask: &gtk::DrawingArea,
    view: &View,
) {
    let (nw, nh) = view.natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    let (vw, vh) = viewport(fixed);
    let s = (vw / nw as f64).min(vh / nh as f64);
    view.scale.set(s);
    view.offset
        .set(((vw - nw as f64 * s) / 2.0, (vh - nh as f64 * s) / 2.0));
    apply(picture, fixed, overlay, mask, view);
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
fn apply(
    picture: &gtk::Picture,
    fixed: &gtk::Fixed,
    overlay: &gtk::DrawingArea,
    mask: &gtk::DrawingArea,
    view: &View,
) {
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
    // Crop + mask overlays are separate auto-filling layers; repaint them so they
    // track the new image position/scale.
    overlay.queue_draw();
    mask.queue_draw();
}

/// Draw the selected mask's geometric shapes over the image (read-only). Coords
/// are normalized (0..1) and mapped through the same image→screen transform the
/// crop overlay uses, so shapes stay glued to the photo under zoom/pan.
fn draw_masks(cr: &cairo::Context, view: &View, shapes: &[MaskShape]) {
    let (nw, nh) = view.natural.get();
    if nw <= 0 || nh <= 0 {
        return;
    }
    let (ix, iy, iw, ih) = image_screen_rect(view);
    cr.set_line_width(1.5);
    for shape in shapes {
        match *shape {
            MaskShape::Radial { cx, cy, rx, ry, rot, .. } => {
                let (scx, scy) = (ix + cx * iw, iy + cy * ih);
                let (srx, sry) = (rx * iw, ry * ih);
                let r = rot.to_radians();
                let (cr_, sr_) = (r.cos(), r.sin());
                // Sample the rotated ellipse as a polyline (transform-free, so the
                // 1.5px stroke isn't distorted by a cairo scale).
                for i in 0..=64 {
                    let a = i as f64 / 64.0 * std::f64::consts::TAU;
                    let (ex, ey) = (srx * a.cos(), sry * a.sin());
                    let (px, py) = (scx + ex * cr_ - ey * sr_, scy + ex * sr_ + ey * cr_);
                    if i == 0 {
                        cr.move_to(px, py);
                    } else {
                        cr.line_to(px, py);
                    }
                }
                stroke_outlined(cr);
                handle(cr, scx, scy); // centre = move
            }
            MaskShape::Linear { x1, y1, x2, y2, .. } => {
                let (a, b) = ((ix + x1 * iw, iy + y1 * ih), (ix + x2 * iw, iy + y2 * ih));
                cr.move_to(a.0, a.1);
                cr.line_to(b.0, b.1);
                stroke_outlined(cr);
                handle(cr, a.0, a.1);
                handle(cr, b.0, b.1);
            }
        }
    }
}

/// Hit-test mask handles at widget point `(x, y)`. Returns the grab + the shape
/// at press + the press point in normalized image coords. Radial: centre =>
/// move, near boundary => resize. Linear: nearest endpoint.
fn hit_mask(view: &View, shapes: &[MaskShape], x: f64, y: f64) -> Option<MaskDrag> {
    let (ix, iy, iw, ih) = image_screen_rect(view);
    if iw <= 0.0 || ih <= 0.0 {
        return None;
    }
    let press = ((x - ix) / iw, (y - iy) / ih);
    let near = |px: f64, py: f64| (px - x).hypot(py - y) <= HANDLE;
    for shape in shapes {
        match *shape {
            MaskShape::Radial { cx, cy, rx, ry, rot, .. } => {
                let (scx, scy) = (ix + cx * iw, iy + cy * ih);
                if near(scx, scy) {
                    return Some(MaskDrag { grab: MaskGrab::RadialMove, orig: *shape, press });
                }
                // Boundary hit: rotate the press into the ellipse's local frame,
                // measure radial distance d (==1 on the boundary), and check the
                // pixel gap from press to the boundary along that ray.
                let r = rot.to_radians();
                let (lx, ly) = (x - scx, y - scy);
                let (ux, uy) = (lx * r.cos() + ly * r.sin(), -lx * r.sin() + ly * r.cos());
                let (srx, sry) = ((rx * iw).max(1.0), (ry * ih).max(1.0));
                let d = (ux / srx).hypot(uy / sry);
                let gap = ux.hypot(uy) * (1.0 - 1.0 / d.max(1e-6)).abs();
                if d > 0.1 && gap <= HANDLE {
                    return Some(MaskDrag { grab: MaskGrab::RadialResize, orig: *shape, press });
                }
            }
            MaskShape::Linear { x1, y1, x2, y2, .. } => {
                if near(ix + x1 * iw, iy + y1 * ih) {
                    return Some(MaskDrag { grab: MaskGrab::LinearStart, orig: *shape, press });
                }
                if near(ix + x2 * iw, iy + y2 * ih) {
                    return Some(MaskDrag { grab: MaskGrab::LinearEnd, orig: *shape, press });
                }
            }
        }
    }
    None
}

/// Produce the edited shape for a drag whose pointer is at normalized `cur`.
fn apply_mask_drag(md: MaskDrag, cur: (f64, f64)) -> MaskShape {
    let cl = |v: f64| v.clamp(0.0, 1.0);
    match (md.grab, md.orig) {
        (MaskGrab::RadialMove, MaskShape::Radial { sub, cx, cy, rx, ry, rot }) => {
            let dx = cur.0 - md.press.0;
            let dy = cur.1 - md.press.1;
            MaskShape::Radial { sub, cx: cl(cx + dx), cy: cl(cy + dy), rx, ry, rot }
        }
        (MaskGrab::RadialResize, MaskShape::Radial { sub, cx, cy, rot, .. }) => MaskShape::Radial {
            sub,
            cx,
            cy,
            rx: (cur.0 - cx).abs().max(0.005),
            ry: (cur.1 - cy).abs().max(0.005),
            rot,
        },
        (MaskGrab::LinearStart, MaskShape::Linear { sub, x2, y2, .. }) => MaskShape::Linear {
            sub,
            x1: cl(cur.0),
            y1: cl(cur.1),
            x2,
            y2,
        },
        (MaskGrab::LinearEnd, MaskShape::Linear { sub, x1, y1, .. }) => MaskShape::Linear {
            sub,
            x1,
            y1,
            x2: cl(cur.0),
            y2: cl(cur.1),
        },
        // Grab/shape mismatch can't happen (grab derived from the same shape).
        (_, s) => s,
    }
}

/// Widget point -> normalized image coords (0..1), clamped to the image.
fn to_norm(view: &View, x: f64, y: f64) -> Option<(f64, f64)> {
    let (ix, iy, iw, ih) = image_screen_rect(view);
    if iw <= 0.0 || ih <= 0.0 {
        return None;
    }
    Some((((x - ix) / iw).clamp(0.0, 1.0), ((y - iy) / ih).clamp(0.0, 1.0)))
}

/// Draw the in-progress brush/flow stroke as a thick translucent polyline.
fn draw_stroke(cr: &cairo::Context, view: &View, pts: &[(f64, f64)], arm: Option<PaintArm>) {
    let Some(arm) = arm else { return };
    if pts.is_empty() {
        return;
    }
    let (ix, iy, iw, ih) = image_screen_rect(view);
    if iw <= 0.0 {
        return;
    }
    // Brush diameter on screen = 2 * radius_norm * image_screen_width.
    let width = (arm.size_norm * 2.0 * iw).max(2.0);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_width(width);
    if arm.erase {
        cr.set_source_rgba(1.0, 0.3, 0.3, 0.35);
    } else {
        cr.set_source_rgba(0.4, 0.7, 1.0, 0.35);
    }
    let (x0, y0) = pts[0];
    cr.move_to(ix + x0 * iw, iy + y0 * ih);
    for &(x, y) in &pts[1..] {
        cr.line_to(ix + x * iw, iy + y * ih);
    }
    let _ = cr.stroke();
}

/// Draw a small grab handle (white dot, dark ring) at a screen point.
fn handle(cr: &cairo::Context, x: f64, y: f64) {
    cr.arc(x, y, 5.0, 0.0, std::f64::consts::TAU);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

/// Stroke the current path twice (dark halo, then white) so it reads on any
/// background. Consumes the path.
fn stroke_outlined(cr: &cairo::Context) {
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.6);
    cr.set_line_width(3.0);
    let _ = cr.stroke_preserve();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(1.25);
    let _ = cr.stroke();
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

#[cfg(test)]
mod tests {
    use super::aspect_preserved;

    #[test]
    fn aspect_change_forces_refit() {
        // Same image, lower preview resolution -> preserve zoom.
        assert!(aspect_preserved((4000, 3000), (2000, 1500)));
        // 65:24 crop (~2.71) returning to the full 4:3 image -> re-fit (the bug).
        assert!(!aspect_preserved((2600, 960), (4000, 3000)));
        // 90° rotation swaps w/h -> re-fit.
        assert!(!aspect_preserved((4000, 3000), (3000, 4000)));
        // No prior image -> re-fit.
        assert!(!aspect_preserved((0, 0), (4000, 3000)));
        // 1px rounding on a proportional rescale stays within tolerance.
        assert!(aspect_preserved((4000, 3000), (1333, 1000)));
    }
}
