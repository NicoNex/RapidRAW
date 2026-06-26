# Masks & Inpaint Panel Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the relm4 Masks and Inpaint right-rail panels to UX/layout parity with the original Tauri UI — card-grid create buttons, canvas drawing for inpaint, plus rename/copy/paste/duplicate.

**Architecture:** All UI lives in `rapidraw-relm4/src/{masks,inpaint,editor}.rs`; state and message handling in `main.rs`. New behaviour follows the existing `AppMsg` → `AppModel` handler → `rebuild` → `RequestRender` pattern. UI panels are rebuilt imperatively (`MasksPanel::rebuild`), so most changes are additive functions called from `rebuild`.

**Tech Stack:** Rust, relm4 / gtk4-rs / libadwaita, relm4-icons (build-time gresource via `icons.toml`), serde_json for mask params.

## Global Constraints

- Mask type strings are the camelCase engine identifiers (e.g. `ai-subject`, `quick-eraser`) — never invent new ones; reuse `masks::MASK_TYPES`.
- New model state (`copied_mask`) is NOT persisted to the sidecar.
- Icons: prefer an existing relm4-icon name; if none fits, fall back to a GTK symbolic icon (`gtk::Image::from_icon_name("…-symbolic")`). Do not add many new SVGs.
- No GTK unit-test harness exists. Pure-logic functions get `#[cfg(test)]` tests; UI changes are verified by `cargo build` + the user running the app.
- Build/verify command: `cargo build -p rapidraw-relm4` from repo root (or `cd rapidraw-relm4 && cargo build`).

---

## File Structure

- `rapidraw-relm4/icons.toml` — add mask-grid glyph names.
- `rapidraw-relm4/src/masks.rs` — create-grid tables + `create_grid()`, header reset button, sub-mask icons, rename/context-menu hooks, `clone_mask()` helper.
- `rapidraw-relm4/src/inpaint.rs` — brush Add/Erase segmented toggle (shared brush UI lives in masks.rs `brush_controls`).
- `rapidraw-relm4/src/editor.rs` — canvas draw/arm fixes (Phase 2, runtime-driven).
- `rapidraw-relm4/src/main.rs` — new `AppMsg` variants + handlers + `AppModel.copied_mask`.

---

## Task 1: Masks create-grid data + helper

**Files:**
- Modify: `rapidraw-relm4/icons.toml`
- Modify: `rapidraw-relm4/src/masks.rs` (add tables + `create_grid` + `mask_icon`, near `MASK_TYPES` line 27 and `add_menu` line 337)
- Test: inline `#[cfg(test)]` in `masks.rs`

**Interfaces:**
- Produces:
  - `const MASK_CREATE_GRID: &[(&str, &str)]` — `(label, type)` for the 5 primary cards.
  - `const OTHERS_TYPES: &[(&str, &str)]` — `(label, type)` for the Others popover.
  - `fn mask_icon(ty: &str) -> Option<&'static str>` — relm4-icon name or None.
  - `fn create_grid(sender: &ComponentSender<AppModel>) -> gtk::Grid` — 3-col card grid; primary cards emit `AppMsg::AddMask(ty)`, the Others card opens a popover of `OTHERS_TYPES`.

- [ ] **Step 1: Add glyph names to icons.toml**

In `rapidraw-relm4/icons.toml`, under `# Inpaint create-grid cards` (or a new `# Masks create-grid` block), add any missing names. Required for masks grid: `sparkle-regular` (have), `person-regular` (have), `line-horizontal-4-regular` (have), `circle-regular` (have), plus `cloud-regular` (Sky) and `more-horizontal-regular` (Others). For the Others popover also try `color-regular` (Color), `brightness-high-regular` (Luminance). If a name is rejected at build time (relm4-icons doesn't ship it), remove it here and return `None` from `mask_icon` for that type (the card renders label-only). Add:

```toml
    # Masks create-grid cards
    "cloud-regular",          # Sky
    "more-horizontal-regular",# Others
```

- [ ] **Step 2: Write the failing test**

Add to the `tests` module at the bottom of `masks.rs`:

```rust
#[test]
fn every_create_grid_type_is_a_known_mask_type() {
    for &(_, ty) in MASK_CREATE_GRID.iter().chain(OTHERS_TYPES.iter()) {
        assert!(
            MASK_TYPES.iter().any(|(_, t)| *t == ty),
            "create-grid type {ty} not in MASK_TYPES"
        );
    }
    // The two tables together cover every offered mask type exactly once.
    let mut seen: Vec<&str> = MASK_CREATE_GRID.iter().chain(OTHERS_TYPES.iter()).map(|(_, t)| *t).collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), MASK_CREATE_GRID.len() + OTHERS_TYPES.len(), "duplicate type across tables");
}
```

- [ ] **Step 3: Run test, verify it fails**

Run: `cargo test -p rapidraw-relm4 every_create_grid_type_is_a_known_mask_type`
Expected: FAIL — `MASK_CREATE_GRID` not found (does not compile yet).

- [ ] **Step 4: Add the tables + helpers**

After `MASK_TYPES` (masks.rs:39) add:

```rust
/// Primary masks-panel create cards, mirroring Tauri `MASK_PANEL_CREATION_TYPES`.
/// The "Others" card is appended by `create_grid` (it has no single type).
pub const MASK_CREATE_GRID: &[(&str, &str)] = &[
    ("Subject", "ai-subject"),
    ("Sky", "ai-sky"),
    ("Foreground", "ai-foreground"),
    ("Linear", "linear"),
    ("Radial", "radial"),
];

/// Secondary types shown in the "Others" popover (Tauri `OTHERS_MASK_TYPES`).
pub const OTHERS_TYPES: &[(&str, &str)] = &[
    ("Depth", "ai-depth"),
    ("Color", "color"),
    ("Luminance", "luminance"),
    ("Brush", "brush"),
    ("Flow", "flow"),
    ("Whole Image", "all"),
];

/// relm4-icon name for a mask type's create card / row, or None for label-only.
pub fn mask_icon(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "ai-subject" | "luminance" => "sparkle-regular",
        "ai-sky" => "cloud-regular",
        "ai-foreground" => "person-regular",
        "linear" => "line-horizontal-4-regular",
        "radial" | "color" => "circle-regular",
        "brush" | "flow" => "paint-brush-regular",
        "ai-depth" => "layer-diagonal-regular",
        "all" => "crop-regular",
        "quick-eraser" => "eraser",
        _ => return None,
    })
}
```

Then add `create_grid` (model it on `inpaint::create_grid`, 3 columns, with the trailing Others card):

```rust
/// 3-col "Create New Mask" card grid. Primary cards add their mask; the final
/// "Others" card opens a popover listing `OTHERS_TYPES`.
fn create_grid(sender: &ComponentSender<AppModel>) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(6);
    grid.set_column_homogeneous(true);

    let card = |icon: Option<&str>, label: &str| {
        let b = gtk::Button::new();
        b.add_css_class("card");
        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        if let Some(icon) = icon {
            let img = gtk::Image::from_icon_name(icon);
            img.set_pixel_size(22);
            content.append(&img);
        }
        let lbl = gtk::Label::new(Some(label));
        lbl.set_wrap(true);
        lbl.set_justify(gtk::Justification::Center);
        content.append(&lbl);
        b.set_child(Some(&content));
        b
    };

    for (idx, &(label, ty)) in MASK_CREATE_GRID.iter().enumerate() {
        let b = card(mask_icon(ty), label);
        let sender = sender.clone();
        b.connect_clicked(move |_| sender.input(AppMsg::AddMask(ty)));
        grid.attach(&b, (idx % 3) as i32, (idx / 3) as i32, 1, 1);
    }

    // "Others" popover card.
    let others = gtk::MenuButton::new();
    others.add_css_class("card");
    let oc = gtk::Box::new(gtk::Orientation::Vertical, 4);
    oc.set_margin_top(12);
    oc.set_margin_bottom(12);
    oc.append(&gtk::Image::from_icon_name("more-horizontal-regular"));
    let ol = gtk::Label::new(Some("Others"));
    ol.set_wrap(true);
    ol.set_justify(gtk::Justification::Center);
    oc.append(&ol);
    others.set_child(Some(&oc));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.set_margin_all(4);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&list));
    for &(label, ty) in OTHERS_TYPES {
        let item = gtk::Button::with_label(label);
        item.add_css_class("flat");
        item.set_halign(gtk::Align::Fill);
        let sender = sender.clone();
        let pop = pop.clone();
        item.connect_clicked(move |_| {
            pop.popdown();
            sender.input(AppMsg::AddMask(ty));
        });
        list.append(&item);
    }
    others.set_popover(Some(&pop));
    let n = MASK_CREATE_GRID.len();
    grid.attach(&others, (n % 3) as i32, (n / 3) as i32, 1, 1);

    grid
}
```

- [ ] **Step 5: Run test, verify it passes**

Run: `cargo test -p rapidraw-relm4 every_create_grid_type_is_a_known_mask_type`
Expected: PASS.

- [ ] **Step 6: Build (create_grid is dead until Task 2 — allow it)**

Run: `cargo build -p rapidraw-relm4`
Expected: builds; `create_grid` may warn "never used" until Task 2. Add `#[allow(dead_code)]` on `create_grid` temporarily if the build denies warnings, else leave it.

- [ ] **Step 7: Commit**

```bash
git add rapidraw-relm4/icons.toml rapidraw-relm4/src/masks.rs
git commit -m "feat(relm4): masks create-grid tables + helper"
```

---

## Task 2: Wire create-grid into masks panel + reset-all

**Files:**
- Modify: `rapidraw-relm4/src/masks.rs` — `MasksPanel::rebuild` (line 300-333), remove/replace `add_menu` (line 337)
- Modify: `rapidraw-relm4/src/main.rs` — `AppMsg` enum (~line 234), handler (~line 2399 area), reuse delete-all-masks logic

**Interfaces:**
- Consumes: `create_grid` (Task 1).
- Produces: `AppMsg::ResetAllMasks` variant + handler that clears `self.session.masks`, resets `selected_mask`/`edit_patch`, rebuilds, re-renders.

- [ ] **Step 1: Add the message variant**

In `main.rs` near `AddMask` (line 234) add:

```rust
    ResetAllMasks,
```

- [ ] **Step 2: Add the handler**

In the match in `main.rs`, near the `AddMask` arm (line 2399), add:

```rust
            AppMsg::ResetAllMasks => {
                self.session.masks.clear();
                self.selected_mask = None;
                self.edit_patch = None;
                self.canvas.set_mask_draw(None);
                self.masks_panel.rebuild(&self.session.masks, None, &sender);
                self.refresh_mask_overlay();
                self.schedule_history(&sender);
                sender.input(AppMsg::RequestRender);
            }
```

(If `refresh_mask_overlay` is private/named differently, match the call used in the `DeleteMask` arm at line 2441.)

- [ ] **Step 3: Replace add_menu with the grid + header in rebuild**

In `masks.rs` `rebuild` (line 300), replace the body that calls `add_menu` with: a header row (title + reset button), then grid-when-empty / list+add-row-when-populated. Change the first lines after clearing children (currently lines 310-333):

```rust
        // Header: title + reset-all.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_bottom(4);
        let title = gtk::Label::new(Some("Masking"));
        title.add_css_class("title-4");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        header.append(&title);
        let reset = gtk::Button::from_icon_name("arrow-counterclockwise-regular");
        reset.add_css_class("flat");
        reset.set_tooltip_text(Some("Reset all masks"));
        reset.set_sensitive(!masks.is_empty());
        {
            let sender = sender.clone();
            reset.connect_clicked(move |_| sender.input(AppMsg::ResetAllMasks));
        }
        header.append(&reset);
        self.body.append(&header);

        if masks.is_empty() {
            let heading = gtk::Label::new(Some("Create New Mask"));
            heading.add_css_class("heading");
            heading.set_halign(gtk::Align::Start);
            heading.set_margin_bottom(2);
            self.body.append(&heading);
            self.body.append(&create_grid(sender));
            return;
        }

        let heading = gtk::Label::new(Some("Masks"));
        heading.add_css_class("heading");
        heading.set_halign(gtk::Align::Start);
        heading.set_margin_bottom(2);
        self.body.append(&heading);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        list.add_css_class("card");
        list.set_margin_top(4);
        for (i, m) in masks.iter().enumerate() {
            list.append(&mask_row(i, m, selected == Some(i), sender));
        }
        self.body.append(&list);

        // "Add new mask" → popover containing the same grid.
        let add = gtk::MenuButton::new();
        add.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("add-regular")
                .label("Add new mask")
                .build(),
        ));
        add.add_css_class("flat");
        add.set_margin_top(2);
        let pop = gtk::Popover::new();
        pop.set_child(Some(&create_grid(sender)));
        add.set_popover(Some(&pop));
        self.body.append(&add);

        if let Some(i) = selected {
            if let Some(m) = masks.get(i) {
                self.body.append(&mask_details(i, m, &self.vadj, sender));
            }
        }
        return;
```

Remove the now-unused `add_menu` function (masks.rs:337-365). Keep `sub_add_menu`.

- [ ] **Step 4: Build**

Run: `cargo build -p rapidraw-relm4`
Expected: builds clean (no dead-code warning for `create_grid` now; `add_menu` removed).

- [ ] **Step 5: User verifies in app**

User runs the app, opens Masks tab:
- With no masks: sees 3-col card grid (Subject/Sky/Foreground/Linear/Radial/Others); Others opens a popover.
- Clicking a card adds that mask and shows the list + "Add new mask" + reset button.
- Reset button clears all masks.

- [ ] **Step 6: Commit**

```bash
git add rapidraw-relm4/src/masks.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): masks panel card-grid create UI + reset-all"
```

---

## Task 3: Diagnose & fix inpaint canvas drawing (runtime)

**Files (inspect; fix where the defect is):**
- `rapidraw-relm4/src/editor.rs` — `GestureDrag` handlers (drag_begin ~322, plus the mask-draw / pick / paint branches), `set_mask_draw`, `set_pick`, `set_paint`.
- `rapidraw-relm4/src/main.rs` — `AddSubMask` auto-arm branch (line 2769-2789), `ArmPaint` (2627), `ArmPick` (2711), `AddBrushStroke` (2635), `set_mask_draw` callers.

This task is runtime-driven; the exact fix depends on the observed symptom. Do NOT pre-write a speculative code change.

- [ ] **Step 1: Reproduce (user)**

User runs the app, Inpaint tab, clicks a create card (try each: Brush, Subject, Quick Erase, Radial), then tries to draw on the image. User reports for each tool: does a stroke/box/shape appear? Any stderr output/panic? Does the cursor change? Does Generate become enabled?

- [ ] **Step 2: Locate the break from the report**

Map the symptom to the code path:
- Nothing arms (no draw possible for any tool) → check `AddPatch` → `AddSubMask` routing: `edit_patch` set before `AddSubMask` runs? (main.rs 2194/2206). Confirm `container_subs_mut(mask)` returns the patch's subs when `edit_patch.is_some()`.
- Brush doesn't paint but radial/linear draws → `ArmPaint`/`AddBrushStroke` path or `paint_sub` state; check the drag-begin paint branch in editor.rs.
- Radial/linear doesn't draw → `set_mask_draw` flag / mask-draw drag branch (editor.rs ~322-360) being pre-empted by pan or crop mode.
- Box (Subject) doesn't draw → `set_pick` / `PickArm.is_box` / pick drag branch.
- Draws but Generate stays disabled → `has_region` in `inpaint::patch_details` (line 252) — sub_masks not landing on the patch.

- [ ] **Step 3: Apply the minimal fix**

Edit the single identified site. Re-run the same reproduction.

- [ ] **Step 4: User verifies**

User: each inpaint tool draws on the canvas; drawing a region then Generate runs the engine and the result composites onto the image.

- [ ] **Step 5: Commit**

```bash
git add rapidraw-relm4/src/editor.rs rapidraw-relm4/src/main.rs
git commit -m "fix(relm4): inpaint canvas drawing arms and triggers generate"
```

---

## Task 4: clone_mask + copy/paste/duplicate/invert (logic)

**Files:**
- Modify: `rapidraw-relm4/src/masks.rs` — add `clone_mask`
- Modify: `rapidraw-relm4/src/main.rs` — `AppModel.copied_mask` field (~line 685), init (~line 1530), `AppMsg` variants (~234), handlers
- Test: inline `#[cfg(test)]` in `masks.rs`

**Interfaces:**
- Produces:
  - `pub fn clone_mask(m: &MaskDefinition, invert: bool) -> MaskDefinition` — deep clone with fresh `next_id` for container + every sub-mask; if `invert`, flips the container's `invert` field.
  - `AppMsg::CopyMask(usize)`, `AppMsg::PasteMask`, `AppMsg::DuplicateMask(usize)`, `AppMsg::DuplicateMaskInvert(usize)`.
  - `AppModel.copied_mask: Option<MaskDefinition>`.

- [ ] **Step 1: Write the failing test**

Add to `masks.rs` tests:

```rust
#[test]
fn clone_mask_gives_fresh_ids_and_keeps_data() {
    let m = new_mask("Radial", "radial", 1000.0, 800.0);
    let c = clone_mask(&m, false);
    assert_ne!(c.id, m.id, "container id must be fresh");
    assert_eq!(c.sub_masks.len(), m.sub_masks.len());
    assert_ne!(c.sub_masks[0].id, m.sub_masks[0].id, "sub id must be fresh");
    assert_eq!(c.adjustments, m.adjustments, "adjustments preserved");
    assert_eq!(c.invert, m.invert);
    // Invert variant flips the flag.
    let inv = clone_mask(&m, true);
    assert_eq!(inv.invert, !m.invert);
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test -p rapidraw-relm4 clone_mask_gives_fresh_ids_and_keeps_data`
Expected: FAIL — `clone_mask` not found.

- [ ] **Step 3: Implement clone_mask**

In `masks.rs` after `new_mask` (line 212):

```rust
/// Deep-clone a mask container with fresh ids (container + every sub-mask),
/// mirroring Tauri `cloneMaskContainerData`. `invert` flips the container's
/// invert flag (for "Duplicate & Invert").
pub fn clone_mask(m: &MaskDefinition, invert: bool) -> MaskDefinition {
    let mut c = m.clone();
    c.id = next_id("mask");
    for sm in &mut c.sub_masks {
        sm.id = next_id("sub");
    }
    if invert {
        c.invert = !c.invert;
    }
    c
}
```

- [ ] **Step 4: Run test, verify it passes**

Run: `cargo test -p rapidraw-relm4 clone_mask_gives_fresh_ids_and_keeps_data`
Expected: PASS. (If `MaskDefinition`/`SubMask` aren't `Clone`, add `#[derive(Clone)]` in `rapidraw-core/src/mask_generation.rs` — check first; they almost certainly are, since the session is cloned for history.)

- [ ] **Step 5: Add model state + messages**

In `main.rs`: add field near `edit_patch` (line 695):

```rust
    copied_mask: Option<MaskDefinition>,
```

Initialize it where the model is built (line 1530 area, next to `edit_patch: None,`):

```rust
            copied_mask: None,
```

Add `MaskDefinition` to the `masks::` / core imports if not already in scope at that file location. Add message variants near `AddMask` (line 234):

```rust
    CopyMask(usize),
    PasteMask,
    DuplicateMask(usize),
    DuplicateMaskInvert(usize),
```

- [ ] **Step 6: Add handlers**

Near the `AddMask` arm (main.rs 2399):

```rust
            AppMsg::CopyMask(i) => {
                self.copied_mask = self.session.masks.get(i).cloned();
                self.masks_panel.rebuild(&self.session.masks, self.selected_mask, &sender);
            }
            AppMsg::PasteMask => {
                if let Some(src) = self.copied_mask.clone() {
                    self.session.masks.push(masks::clone_mask(&src, false));
                    self.selected_mask = Some(self.session.masks.len() - 1);
                    self.masks_panel.rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::DuplicateMask(i) => {
                if let Some(src) = self.session.masks.get(i).cloned() {
                    self.session.masks.insert(i + 1, masks::clone_mask(&src, false));
                    self.selected_mask = Some(i + 1);
                    self.masks_panel.rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
            AppMsg::DuplicateMaskInvert(i) => {
                if let Some(src) = self.session.masks.get(i).cloned() {
                    self.session.masks.insert(i + 1, masks::clone_mask(&src, true));
                    self.selected_mask = Some(i + 1);
                    self.masks_panel.rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.refresh_mask_overlay();
                    self.schedule_history(&sender);
                    sender.input(AppMsg::RequestRender);
                }
            }
```

(Match `refresh_mask_overlay`/`schedule_history` to the exact names used by the `AddMask`/`DeleteMask` arms.)

- [ ] **Step 7: Build**

Run: `cargo build -p rapidraw-relm4`
Expected: builds (handlers may be unused until Task 5 wires the menu — `CopyMask` etc. are referenced only there; the `AppMsg` variants won't warn since they're constructed in Task 5. If "never constructed" warnings block the build, proceed to Task 5 before the final build, or `#[allow(dead_code)]`).

- [ ] **Step 8: Commit**

```bash
git add rapidraw-relm4/src/masks.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): clone_mask + copy/paste/duplicate mask messages"
```

---

## Task 5: Container right-click context menu

**Files:**
- Modify: `rapidraw-relm4/src/masks.rs` — `mask_row` (line 401)

**Interfaces:**
- Consumes: `AppMsg::{CopyMask, PasteMask, DuplicateMask, DuplicateMaskInvert, DeleteMask}` (Task 4), `has_copy: bool` passed into the row.
- Produces: a right-click `gtk::PopoverMenu` on each mask row.

- [ ] **Step 1: Thread a `has_copy` flag into mask_row**

Change `mask_row` signature (line 401) to accept whether a mask is on the clipboard, and pass `self.copied_mask.is_some()` from `rebuild` (the `mask_row` call in Task 2's list loop). Update both call sites.

```rust
fn mask_row(
    i: usize,
    m: &MaskDefinition,
    is_selected: bool,
    has_copy: bool,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
```

In `rebuild`'s list loop: `list.append(&mask_row(i, m, selected == Some(i), self_has_copy, sender));` where `self_has_copy` is a `bool` parameter you add to `rebuild` (thread `self.copied_mask.is_some()` from the caller) — OR simpler: add a `has_copy: bool` parameter to `MasksPanel::rebuild` and update all `.rebuild(...)` call sites in `main.rs` (lines 782, 901, plus the new ones in Task 2/4 handlers) to pass `self.copied_mask.is_some()`.

- [ ] **Step 2: Build the context menu in mask_row**

Before `row` is returned, add a right-click gesture opening a popover of flat buttons:

```rust
    let menu = gtk::Popover::new();
    menu.set_has_arrow(false);
    menu.set_parent(&row);
    let items = gtk::Box::new(gtk::Orientation::Vertical, 2);
    items.set_margin_all(4);
    let mk = |label: &str, msg: AppMsg, enabled: bool, sender: &ComponentSender<AppModel>, menu: &gtk::Popover| {
        let b = gtk::Button::with_label(label);
        b.add_css_class("flat");
        b.set_halign(gtk::Align::Fill);
        b.set_sensitive(enabled);
        let sender = sender.clone();
        let menu = menu.clone();
        b.connect_clicked(move |_| { menu.popdown(); sender.input(msg.clone()); });
        b
    };
    items.append(&mk("Duplicate", AppMsg::DuplicateMask(i), true, sender, &menu));
    items.append(&mk("Duplicate & Invert", AppMsg::DuplicateMaskInvert(i), true, sender, &menu));
    items.append(&mk("Copy mask", AppMsg::CopyMask(i), true, sender, &menu));
    items.append(&mk("Paste mask", AppMsg::PasteMask, has_copy, sender, &menu));
    items.append(&mk("Delete", AppMsg::DeleteMask(i), true, sender, &menu));
    menu.set_child(Some(&items));

    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    {
        let menu = menu.clone();
        click.connect_pressed(move |_, _, x, y| {
            menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            menu.popup();
        });
    }
    row.add_controller(click);
```

This requires `AppMsg: Clone` (it already is — messages are cloned through `sender.input`). Add `use gtk::gdk;` if not already imported in `masks.rs`.

- [ ] **Step 3: Build**

Run: `cargo build -p rapidraw-relm4`
Expected: builds clean; Task 4 warnings resolved (variants now constructed).

- [ ] **Step 4: User verifies**

Right-click a mask row → menu with Duplicate / Duplicate & Invert / Copy / Paste (greyed until something copied) / Delete. Each works.

- [ ] **Step 5: Commit**

```bash
git add rapidraw-relm4/src/masks.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): mask row right-click context menu"
```

---

## Task 6: Inline rename of mask containers

**Files:**
- Modify: `rapidraw-relm4/src/masks.rs` — `mask_row` (name button)
- Modify: `rapidraw-relm4/src/main.rs` — `AppMsg::RenameMask` + handler

**Interfaces:**
- Produces: `AppMsg::RenameMask(usize, String)` + handler that sets `masks[i].name` and rebuilds.

- [ ] **Step 1: Add message + handler**

In `main.rs` near `AddMask`:

```rust
    RenameMask(usize, String),
```

Handler near `AddMask` arm:

```rust
            AppMsg::RenameMask(i, name) => {
                if let Some(m) = self.session.masks.get_mut(i) {
                    m.name = name;
                    self.masks_panel.rebuild(&self.session.masks, self.selected_mask, &sender);
                    self.schedule_history(&sender);
                }
            }
```

- [ ] **Step 2: Double-click to rename in mask_row**

In `mask_row`, the name is currently a `gtk::Button` (line 425). Add a double-click gesture that swaps it for a `gtk::Entry`. Simplest: wrap name in a `gtk::Stack` (button page + entry page), double-click shows the entry, Enter/focus-out commits:

```rust
    let stack = gtk::Stack::new();
    // button page = existing `name` button (selects mask on single click)
    stack.add_named(&name, Some("label"));
    let entry = gtk::Entry::new();
    entry.set_text(&m.name);
    entry.set_hexpand(true);
    stack.add_named(&entry, Some("edit"));
    stack.set_visible_child_name("label");
    stack.set_hexpand(true);

    let dbl = gtk::GestureClick::new();
    dbl.set_button(gdk::BUTTON_PRIMARY);
    {
        let stack = stack.clone();
        let entry = entry.clone();
        dbl.connect_pressed(move |g, n_press, _, _| {
            if n_press == 2 {
                g.set_state(gtk::EventSequenceState::Claimed);
                stack.set_visible_child_name("edit");
                entry.grab_focus();
            }
        });
    }
    name.add_controller(dbl);

    let commit = {
        let sender = sender.clone();
        let stack = stack.clone();
        move |e: &gtk::Entry| {
            sender.input(AppMsg::RenameMask(i, e.text().to_string()));
            stack.set_visible_child_name("label");
        }
    };
    {
        let commit = commit.clone();
        entry.connect_activate(move |e| commit(e));
    }
    {
        let commit = commit.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |c| {
            if let Some(e) = c.widget().and_then(|w| w.downcast::<gtk::Entry>().ok()) {
                commit(&e);
            }
        });
        entry.add_controller(focus);
    }
```

Then append `stack` to `row` instead of `name` directly. (Adjust: the single-click select handler stays on `name`; the rename commit closure must be `Clone` — capture clones.)

- [ ] **Step 3: Build**

Run: `cargo build -p rapidraw-relm4`
Expected: builds clean.

- [ ] **Step 4: User verifies**

Double-click a mask name → becomes an entry; type, press Enter or click away → name updates.

- [ ] **Step 5: Commit**

```bash
git add rapidraw-relm4/src/masks.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): inline rename of mask containers"
```

---

## Task 7: Per-type icons on sub-mask rows

**Files:**
- Modify: `rapidraw-relm4/src/masks.rs` — `submask_editor` group title (line 717)

**Interfaces:**
- Consumes: `mask_icon` (Task 1).

- [ ] **Step 1: Add an icon to the sub-mask group header**

In `submask_editor` (line 717), the group `title` is set from `pretty_type`. Add a leading icon to the header by prepending an image to the header suffix box, or set the group header via a custom title widget. Minimal approach — add the icon into the existing `suffix` box (line 724) at the front:

```rust
    if let Some(icon) = mask_icon(&sm.mask_type) {
        let img = gtk::Image::from_icon_name(icon);
        img.set_pixel_size(16);
        img.set_margin_end(4);
        suffix.prepend(&img);
    }
```

(If `gtk::Box` has no `prepend`, use `suffix.insert_child_after(&img, None::<&gtk::Widget>)`.)

- [ ] **Step 2: Build**

Run: `cargo build -p rapidraw-relm4`
Expected: builds clean.

- [ ] **Step 3: User verifies**

Each sub-mask group header shows its type icon.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-relm4/src/masks.rs
git commit -m "feat(relm4): per-type icons on sub-mask rows"
```

---

## Task 8: Brush Add/Erase segmented toggle

**Files:**
- Modify: `rapidraw-relm4/src/masks.rs` — `brush_controls` (line 855)

**Interfaces:**
- Consumes: `AppMsg::SetBrushErase(bool)` (exists, line 884 area).

- [ ] **Step 1: Replace the Eraser SwitchRow with a 2-button segmented toggle**

In `brush_controls` (line 855), the current Eraser `adw::SwitchRow` (lines 879-886) toggles `SetBrushErase`. Replace with a linked button pair (Tauri "Add | Erase"):

```rust
    let seg = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    seg.add_css_class("linked");
    seg.set_margin_start(6);
    seg.set_margin_end(6);
    seg.set_homogeneous(true);
    let add_btn = gtk::ToggleButton::with_label("Add");
    let erase_btn = gtk::ToggleButton::with_label("Erase");
    erase_btn.set_group(Some(&add_btn));
    add_btn.set_active(true); // default = paint (add)
    {
        let sender = sender.clone();
        add_btn.connect_toggled(move |b| {
            if b.is_active() { sender.input(AppMsg::SetBrushErase(false)); }
        });
    }
    {
        let sender = sender.clone();
        erase_btn.connect_toggled(move |b| {
            if b.is_active() { sender.input(AppMsg::SetBrushErase(true)); }
        });
    }
    seg.append(&add_btn);
    seg.append(&erase_btn);
    // adw::PreferencesGroup takes rows; wrap the segmented box in an ActionRow,
    // or append to the group's parent. Simplest: an ActionRow with the box as child.
    let seg_row = adw::ActionRow::new();
    seg_row.set_title("Mode");
    seg_row.add_suffix(&seg);
    group.add(&seg_row);
```

Remove the old Eraser `SwitchRow` block (lines 879-886). Keep size, feather, Paint arm, and Clear.

- [ ] **Step 2: Build**

Run: `cargo build -p rapidraw-relm4`
Expected: builds clean.

- [ ] **Step 3: User verifies**

Brush sub-mask shows an Add | Erase segmented control; selecting Erase makes strokes subtract.

- [ ] **Step 4: Commit**

```bash
git add rapidraw-relm4/src/masks.rs
git commit -m "feat(relm4): brush Add/Erase segmented toggle"
```

---

## Final verification

- [ ] Run full build + tests: `cargo build -p rapidraw-relm4 && cargo test -p rapidraw-relm4`
- [ ] User runs the app and confirms: masks card grid, Others popover, reset-all, list + add-new, right-click menu (dup/invert/copy/paste/delete), inline rename, sub-mask icons, brush segmented toggle, and inpaint canvas drawing → Generate.
- [ ] Update memory `inpaint-port-status` / add a masks-overhaul note if the status changed materially.

## Notes on ordering & dependencies

- Tasks 1→2 are Phase 1 (masks grid). Task 3 is Phase 2 (inpaint drawing, runtime). Tasks 4→8 are Phase 3 (polish). 4 must precede 5 (menu uses the messages); 1 must precede 2 and 7.
- Tasks 4's "never constructed" warnings are resolved by Task 5; if the build denies warnings between them, run 4 and 5 back-to-back before a clean build.
