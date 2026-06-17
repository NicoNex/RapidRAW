//! Settings dialog for the RapidRAW relm4 frontend.
//!
//! Presents a modal `adw::PreferencesDialog` and emits a fresh [`Settings`]
//! value via `crate::AppMsg::SettingsChanged` whenever any control changes.

use std::rc::Rc;

use adw::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Background {
    Default,
    White,
    Black,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Editor preview max edge in px.
    pub preview_dim: u32,
    /// Thumbnail max edge in px.
    pub thumb_dim: u32,
    pub background: Background,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            preview_dim: 2048,
            thumb_dim: 300,
            background: Background::Default,
        }
    }
}

// Option tables: index <-> value mappings shared between selection and read-back.
const PREVIEW_DIMS: [u32; 3] = [1024, 2048, 4096];
const THUMB_DIMS: [u32; 3] = [200, 300, 400];

fn background_to_index(bg: Background) -> u32 {
    match bg {
        Background::Default => 0,
        Background::White => 1,
        Background::Black => 2,
    }
}

fn index_to_background(idx: u32) -> Background {
    match idx {
        1 => Background::White,
        2 => Background::Black,
        _ => Background::Default,
    }
}

/// Find the index of `value` in `table`, falling back to the index whose value
/// is numerically nearest.
fn nearest_index(table: &[u32], value: u32) -> u32 {
    if let Some(i) = table.iter().position(|&v| v == value) {
        return i as u32;
    }
    let mut best = 0usize;
    let mut best_diff = u32::MAX;
    for (i, &v) in table.iter().enumerate() {
        let diff = v.abs_diff(value);
        if diff < best_diff {
            best_diff = diff;
            best = i;
        }
    }
    best as u32
}

/// Present a modal settings dialog over `parent`. On ANY change, build a fresh
/// [`Settings`] from the current widget states and emit it via
/// `sender.input(crate::AppMsg::SettingsChanged(settings))`.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    current: Settings,
    sender: &relm4::ComponentSender<crate::AppModel>,
) {
    // libadwaita 0.7 with the `v1_4` feature does not expose
    // `adw::PreferencesDialog` (added in 1.5). Use the windowed predecessor
    // `adw::PreferencesWindow`, presented as a modal transient window over the
    // parent's toplevel.
    let dialog = adw::PreferencesWindow::new();
    dialog.set_title(Some("Settings"));
    dialog.set_modal(true);
    if let Some(root) = parent.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        dialog.set_transient_for(Some(&root));
    }

    let page = adw::PreferencesPage::new();

    // --- Editor group ---
    let editor_group = adw::PreferencesGroup::new();
    editor_group.set_title("Editor");

    let background_row = adw::ComboRow::new();
    background_row.set_title("Background");
    let background_model =
        gtk::StringList::new(&["Default (theme)", "White", "Black"]);
    background_row.set_model(Some(&background_model));
    background_row.set_selected(background_to_index(current.background));

    let preview_row = adw::ComboRow::new();
    preview_row.set_title("Preview quality");
    let preview_model = gtk::StringList::new(&["1024 px", "2048 px", "4096 px"]);
    preview_row.set_model(Some(&preview_model));
    preview_row.set_selected(nearest_index(&PREVIEW_DIMS, current.preview_dim));

    editor_group.add(&background_row);
    editor_group.add(&preview_row);

    // --- Library group ---
    let library_group = adw::PreferencesGroup::new();
    library_group.set_title("Library");

    let thumb_row = adw::ComboRow::new();
    thumb_row.set_title("Thumbnail size");
    let thumb_model = gtk::StringList::new(&["200 px", "300 px", "400 px"]);
    thumb_row.set_model(Some(&thumb_model));
    thumb_row.set_selected(nearest_index(&THUMB_DIMS, current.thumb_dim));

    library_group.add(&thumb_row);

    page.add(&editor_group);
    page.add(&library_group);
    dialog.add(&page);

    // Wrap the rows so a single 'static closure can read all three on change.
    let background_row = Rc::new(background_row);
    let preview_row = Rc::new(preview_row);
    let thumb_row = Rc::new(thumb_row);

    let emit = {
        let background_row = Rc::clone(&background_row);
        let preview_row = Rc::clone(&preview_row);
        let thumb_row = Rc::clone(&thumb_row);
        let sender = sender.clone();
        move || {
            let preview_idx = preview_row.selected() as usize;
            let thumb_idx = thumb_row.selected() as usize;
            let settings = Settings {
                preview_dim: PREVIEW_DIMS
                    .get(preview_idx)
                    .copied()
                    .unwrap_or(2048),
                thumb_dim: THUMB_DIMS.get(thumb_idx).copied().unwrap_or(300),
                background: index_to_background(background_row.selected()),
            };
            sender.input(crate::AppMsg::SettingsChanged(settings));
        }
    };

    {
        let emit = emit.clone();
        background_row.connect_selected_notify(move |_| emit());
    }
    {
        let emit = emit.clone();
        preview_row.connect_selected_notify(move |_| emit());
    }
    {
        let emit = emit.clone();
        thumb_row.connect_selected_notify(move |_| emit());
    }

    dialog.present();
}
