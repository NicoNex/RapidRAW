//! Settings dialog for the RapidRAW relm4 frontend.
//!
//! Presents a modal `adw::PreferencesDialog` and emits a fresh [`Settings`]
//! value via `crate::AppMsg::SettingsChanged` whenever any control changes.

use std::rc::Rc;

use adw::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Background {
    Default,
    White,
    Black,
}

/// GTK GSK renderer for the UI (set via `GSK_RENDERER` before GTK init).
/// `Auto` picks a per-platform default: macOS → GL, everything else → Vulkan.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Renderer {
    #[default]
    Auto,
    Gl,
    Ngl,
    Vulkan,
    Cairo,
}

impl Renderer {
    /// The `GSK_RENDERER` string this resolves to (`Auto` → platform default).
    pub fn gsk_value(self) -> &'static str {
        match self {
            Renderer::Auto => {
                if cfg!(target_os = "macos") {
                    "gl"
                } else {
                    "vulkan"
                }
            }
            Renderer::Gl => "gl",
            Renderer::Ngl => "ngl",
            Renderer::Vulkan => "vulkan",
            Renderer::Cairo => "cairo",
        }
    }
}

// Renderers offered in the dialog, per platform (only values GTK supports
// natively there). GL/NGL/Cairo work everywhere; Vulkan is Linux/Windows only
// (macOS has no native Vulkan — GTK's mac backend uses GL).
#[cfg(target_os = "macos")]
const RENDERER_OPTS: &[(Renderer, &str)] = &[
    (Renderer::Auto, "Auto (OpenGL)"),
    (Renderer::Gl, "OpenGL"),
    (Renderer::Ngl, "OpenGL (NGL)"),
    (Renderer::Cairo, "Cairo (software)"),
];
#[cfg(not(target_os = "macos"))]
const RENDERER_OPTS: &[(Renderer, &str)] = &[
    (Renderer::Auto, "Auto (Vulkan)"),
    (Renderer::Vulkan, "Vulkan"),
    (Renderer::Gl, "OpenGL"),
    (Renderer::Ngl, "OpenGL (NGL)"),
    (Renderer::Cairo, "Cairo (software)"),
];

fn renderer_to_index(r: Renderer) -> u32 {
    RENDERER_OPTS
        .iter()
        .position(|&(v, _)| v == r)
        .unwrap_or(0) as u32
}

fn index_to_renderer(idx: u32) -> Renderer {
    RENDERER_OPTS.get(idx as usize).map(|&(v, _)| v).unwrap_or(Renderer::Auto)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Editor preview max edge in px.
    pub preview_dim: u32,
    /// Thumbnail max edge in px.
    pub thumb_dim: u32,
    pub background: Background,
    /// Reset adjustments to defaults when opening a new image.
    pub reset_on_open: bool,
    /// Library raw-status filter (persisted, like the Tauri app).
    pub raw_filter: crate::library::RawFilter,
    /// Library sort order (persisted, like the Tauri app).
    pub sort_by: crate::library::SortBy,
    /// Last-used export options (format/quality/resize), restored in the dialog.
    pub last_export: crate::ExportOpts,
    /// GTK UI renderer (applied via `GSK_RENDERER` at startup; needs a restart).
    pub renderer: Renderer,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            preview_dim: 1600,
            thumb_dim: 300,
            background: Background::Default,
            // Remember each image's edits (like the original). Toggle on to
            // always start a freshly-opened image from defaults instead.
            reset_on_open: false,
            raw_filter: crate::library::RawFilter::All,
            sort_by: crate::library::SortBy::Name,
            last_export: crate::ExportOpts::default(),
            renderer: Renderer::Auto,
        }
    }
}

// Option tables: index <-> value mappings shared between selection and read-back.
const PREVIEW_DIMS: [u32; 4] = [1024, 1600, 2048, 4096];
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
    let preview_model = gtk::StringList::new(&["1024 px", "1600 px", "2048 px", "4096 px"]);
    preview_row.set_model(Some(&preview_model));
    preview_row.set_selected(nearest_index(&PREVIEW_DIMS, current.preview_dim));

    let reset_row = adw::ActionRow::new();
    reset_row.set_title("Reset adjustments on open");
    reset_row.set_subtitle("Start each image from defaults");
    let reset_switch = gtk::Switch::new();
    reset_switch.set_valign(gtk::Align::Center);
    reset_switch.set_active(current.reset_on_open);
    reset_row.add_suffix(&reset_switch);
    reset_row.set_activatable_widget(Some(&reset_switch));

    let renderer_row = adw::ComboRow::new();
    renderer_row.set_title("UI renderer");
    renderer_row.set_subtitle("Takes effect after restart");
    let renderer_labels: Vec<&str> = RENDERER_OPTS.iter().map(|&(_, l)| l).collect();
    let renderer_model = gtk::StringList::new(&renderer_labels);
    renderer_row.set_model(Some(&renderer_model));
    renderer_row.set_selected(renderer_to_index(current.renderer));

    editor_group.add(&background_row);
    editor_group.add(&preview_row);
    editor_group.add(&reset_row);
    editor_group.add(&renderer_row);

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
    let reset_switch = Rc::new(reset_switch);
    let renderer_row = Rc::new(renderer_row);

    let emit = {
        let background_row = Rc::clone(&background_row);
        let preview_row = Rc::clone(&preview_row);
        let thumb_row = Rc::clone(&thumb_row);
        let reset_switch = Rc::clone(&reset_switch);
        let renderer_row = Rc::clone(&renderer_row);
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
                reset_on_open: reset_switch.is_active(),
                renderer: index_to_renderer(renderer_row.selected()),
                // Not editable here; carry the persisted prefs through.
                raw_filter: current.raw_filter,
                sort_by: current.sort_by,
                last_export: current.last_export,
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
    {
        let emit = emit.clone();
        reset_switch.connect_active_notify(move |_| emit());
    }
    {
        let emit = emit.clone();
        renderer_row.connect_selected_notify(move |_| emit());
    }

    dialog.present();
}
