//! Right-rail "AI" panel: AI inpaint patches (generative replace / quick erase).
//! Mirrors the default UI's inpaint section. A patch is a container of sub-masks
//! (brush region or AI auto-mask) plus a prompt; pressing Generate runs the
//! inpaint engine (local LaMa erase, or the external connector for prompt-driven
//! fills) and stores the result, which the render path bakes onto the base.
//!
//! Sub-mask editing reuses [`crate::masks::submask_editor`]; the shared handlers
//! route to the selected patch (vs a mask) via the model's `edit_patch` flag.

use adw::prelude::*;
use relm4::{ComponentSender, RelmWidgetExt};

use rapidraw_core::mask_generation::AiPatchDefinition;

use crate::{AppModel, AppMsg};

/// Sub-mask types offered for a patch: `(label, type-string)`. Brush paints the
/// region; the AI types segment it automatically.
const PATCH_SUB_TYPES: &[(&str, &str)] = &[
    ("Brush", "brush"),
    ("AI Subject", "ai-subject"),
    ("AI Foreground", "ai-foreground"),
];

pub struct InpaintPanel {
    root: gtk::ScrolledWindow,
    body: gtk::Box,
}

impl InpaintPanel {
    pub fn new(sender: &ComponentSender<AppModel>) -> Self {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        body.set_margin_all(6);

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&body));
        root.set_hexpand(false);
        root.set_vexpand(true);
        root.set_width_request(320);

        let panel = Self { root, body };
        panel.rebuild(&[], None, true, sender);
        panel
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Clear and repopulate the patch list + the selected patch's controls.
    pub fn rebuild(
        &self,
        patches: &[AiPatchDefinition],
        selected: Option<usize>,
        fast: bool,
        sender: &ComponentSender<AppModel>,
    ) {
        while let Some(c) = self.body.first_child() {
            self.body.remove(&c);
        }

        let add = gtk::Button::with_label("Add patch");
        add.add_css_class("flat");
        {
            let sender = sender.clone();
            add.connect_clicked(move |_| sender.input(AppMsg::AddPatch));
        }
        self.body.append(&add);

        if patches.is_empty() {
            let hint = gtk::Label::new(Some("No patches. Add one above, draw or\nauto-mask a region, then Generate."));
            hint.add_css_class("dim-label");
            hint.set_margin_top(8);
            self.body.append(&hint);
            return;
        }

        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        list.add_css_class("card");
        list.set_margin_top(4);
        for (i, p) in patches.iter().enumerate() {
            list.append(&patch_row(i, p, selected == Some(i), sender));
        }
        self.body.append(&list);

        if let Some(i) = selected {
            if let Some(p) = patches.get(i) {
                self.body.append(&patch_details(i, p, fast, sender));
            }
        }
    }
}

/// One patch-list row: visibility toggle | name (selects) | delete.
fn patch_row(
    i: usize,
    p: &AiPatchDefinition,
    is_selected: bool,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.set_margin_all(2);

    let eye = gtk::ToggleButton::new();
    eye.set_icon_name(if p.visible {
        "display-brightness-symbolic"
    } else {
        "weather-clear-night-symbolic"
    });
    eye.set_active(p.visible);
    eye.add_css_class("flat");
    eye.set_tooltip_text(Some("Toggle visibility"));
    {
        let sender = sender.clone();
        eye.connect_clicked(move |_| sender.input(AppMsg::TogglePatchVisible(i)));
    }

    let name = gtk::ToggleButton::with_label(&p.name);
    name.set_active(is_selected);
    name.add_css_class("flat");
    name.set_hexpand(true);
    name.set_halign(gtk::Align::Fill);
    {
        let sender = sender.clone();
        name.connect_clicked(move |b| {
            sender.input(AppMsg::SelectPatch(b.is_active().then_some(i)));
        });
    }

    let del = gtk::Button::from_icon_name("user-trash-symbolic");
    del.add_css_class("flat");
    del.set_tooltip_text(Some("Delete patch"));
    {
        let sender = sender.clone();
        del.connect_clicked(move |_| sender.input(AppMsg::DeletePatch(i)));
    }

    row.append(&eye);
    row.append(&name);
    row.append(&del);
    row
}

/// The selected patch's controls: prompt, fast-erase toggle, Generate, plus the
/// sub-mask list (add menu + reused sub-mask editors).
fn patch_details(
    i: usize,
    p: &AiPatchDefinition,
    fast: bool,
    sender: &ComponentSender<AppModel>,
) -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.set_margin_top(6);

    let group = adw::PreferencesGroup::new();
    group.set_title("Generative replace");
    group.set_margin_start(6);
    group.set_margin_end(6);

    // Fast erase (local LaMa, no prompt) vs prompt-driven external connector.
    let fast_row = adw::SwitchRow::new();
    fast_row.set_title("Fast erase (LaMa)");
    fast_row.set_subtitle("Remove content locally; off = prompt via AI connector");
    fast_row.set_active(fast);
    {
        let sender = sender.clone();
        fast_row.connect_active_notify(move |r| sender.input(AppMsg::SetInpaintFast(r.is_active())));
    }
    group.add(&fast_row);

    // Prompt (used when fast erase is off).
    let prompt = adw::EntryRow::new();
    prompt.set_title("Prompt");
    prompt.set_text(&p.prompt);
    prompt.set_sensitive(!fast);
    {
        let sender = sender.clone();
        prompt.connect_changed(move |e| {
            sender.input(AppMsg::SetPatchPrompt(i, e.text().to_string()));
        });
    }
    group.add(&prompt);

    let has_region = !p.sub_masks.is_empty();
    let has_result = p.patch_data.is_some();
    let gen = adw::ActionRow::new();
    gen.set_title("Generate");
    gen.set_subtitle(if has_result {
        "Generated"
    } else if has_region {
        "Ready"
    } else {
        "Add a region below first"
    });
    let gen_btn = gtk::Button::with_label(if has_result { "Regenerate" } else { "Generate" });
    gen_btn.add_css_class("suggested-action");
    gen_btn.set_valign(gtk::Align::Center);
    gen_btn.set_sensitive(has_region);
    {
        let sender = sender.clone();
        gen_btn.connect_clicked(move |_| sender.input(AppMsg::GenerateInpaint { patch: i }));
    }
    gen.add_suffix(&gen_btn);
    group.add(&gen);
    col.append(&group);

    // Sub-mask region: add menu + the reused sub-mask editors.
    col.append(&sub_add_menu(i, sender));
    for (sub_i, sm) in p.sub_masks.iter().enumerate() {
        col.append(&crate::masks::submask_editor(i, sub_i, sm, sender));
    }

    col
}

/// "Add region" menu for a patch (brush / AI auto-mask), emitting `AddSubMask`.
fn sub_add_menu(patch_i: usize, sender: &ComponentSender<AppModel>) -> gtk::MenuButton {
    let btn = gtk::MenuButton::new();
    btn.set_label("Add region");
    btn.add_css_class("flat");
    btn.set_margin_start(6);
    btn.set_margin_end(6);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.set_margin_all(4);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&list));
    for &(label, ty) in PATCH_SUB_TYPES {
        let item = gtk::Button::with_label(label);
        item.add_css_class("flat");
        item.set_halign(gtk::Align::Fill);
        let sender = sender.clone();
        let pop = pop.clone();
        item.connect_clicked(move |_| {
            pop.popdown();
            sender.input(AppMsg::AddSubMask(patch_i, ty));
        });
        list.append(&item);
    }
    btn.set_popover(Some(&pop));
    btn
}
