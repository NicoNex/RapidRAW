//! Right-rail "Info" panel: view and edit photo metadata. Mirrors the Tauri
//! `MetadataPanel` layout where sensible: a file-info card, a camera group, an
//! editable author group, an organization group (rating / colour / tags), an
//! optional GPS group with an "Open in Maps" menu, and the full extended-EXIF
//! list. Read-only EXIF comes from [`crate::meta::read_full_exif`]; the editable
//! fields/tags/colour live in the sidecar ([`crate::sidecar::ImageMeta`]).

use std::collections::BTreeMap;

use adw::prelude::*;
use relm4::{ComponentSender, RelmWidgetExt};

use crate::sidecar::ImageMeta;
use crate::{AppModel, AppMsg};

/// Colour labels (name, hex), matching the Tauri `COLOR_LABELS`.
const COLOR_LABELS: &[(&str, &str)] = &[
    ("red", "#ef4444"),
    ("yellow", "#facc15"),
    ("green", "#4ade80"),
    ("blue", "#60a5fa"),
    ("purple", "#a78bfa"),
];

/// Read-only EXIF keys rendered in the dedicated Camera group (so they're not
/// repeated in the extended list).
const CAMERA_KEYS: &[(&str, &str)] = &[
    ("FNumber", "Aperture"),
    ("ExposureTime", "Shutter speed"),
    ("PhotographicSensitivity", "ISO"),
    ("FocalLengthIn35mmFilm", "Focal length"),
    ("LensModel", "Lens"),
];

/// EXIF keys handled elsewhere (camera/author/GPS/dates), excluded from the
/// extended list.
const HANDLED_KEYS: &[&str] = &[
    "FNumber",
    "ExposureTime",
    "PhotographicSensitivity",
    "FocalLengthIn35mmFilm",
    "LensModel",
    "ImageDescription",
    "Artist",
    "Copyright",
    "UserComment",
    "GPSLatitude",
    "GPSLatitudeRef",
    "GPSLongitude",
    "GPSLongitudeRef",
    "GPSAltitude",
];

/// Everything the panel needs to render for the open image.
pub struct InfoData<'a> {
    pub file_name: String,
    pub extension: String,
    pub width: u32,
    pub height: u32,
    pub exif: &'a BTreeMap<String, String>,
    pub meta: &'a ImageMeta,
    pub rating: u8,
}

pub struct InfoPanel {
    root: gtk::ScrolledWindow,
    body: gtk::Box,
}

impl InfoPanel {
    pub fn new() -> Self {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        body.set_margin_all(8);
        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&body));
        root.set_hexpand(false);
        root.set_vexpand(true);
        root.set_width_request(320);
        Self { root, body }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Repopulate from the open image's data. `data` is `None` when no image is
    /// open (shows a hint).
    pub fn rebuild(&self, data: Option<&InfoData>, sender: &ComponentSender<AppModel>) {
        while let Some(c) = self.body.first_child() {
            self.body.remove(&c);
        }
        let Some(d) = data else {
            let hint = gtk::Label::new(Some("No image open."));
            hint.add_css_class("dim-label");
            hint.set_margin_top(8);
            self.body.append(&hint);
            return;
        };

        self.body.append(&file_info_group(d));
        self.body.append(&camera_group(d));
        self.body.append(&author_group(d, sender));
        self.body.append(&organization_group(d, sender));
        if let Some(gps) = parse_gps(d.exif) {
            self.body.append(&gps_group(&gps));
        }
        if let Some(g) = extended_group(d) {
            self.body.append(&g);
        }
    }
}

fn file_info_group(d: &InfoData) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("File");

    let name = adw::ActionRow::new();
    name.set_title("Name");
    name.set_subtitle(if d.file_name.is_empty() { "-" } else { &d.file_name });
    name.add_css_class("property");
    let badge = gtk::Label::new(Some(&d.extension));
    badge.add_css_class("dim-label");
    badge.set_valign(gtk::Align::Center);
    name.add_suffix(&badge);
    group.add(&name);

    let dims = adw::ActionRow::new();
    dims.set_title("Dimensions");
    dims.add_css_class("property");
    if d.width > 0 && d.height > 0 {
        let mp = (d.width as f64 * d.height as f64) / 1_000_000.0;
        dims.set_subtitle(&format!("{} × {} ({:.1} MP)", d.width, d.height, mp));
    } else {
        dims.set_subtitle("-");
    }
    group.add(&dims);

    let date = adw::ActionRow::new();
    date.set_title("Captured");
    date.add_css_class("property");
    date.set_subtitle(d.exif.get("DateTimeOriginal").map(String::as_str).unwrap_or("-"));
    group.add(&date);
    group
}

fn camera_group(d: &InfoData) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Camera");
    for &(key, label) in CAMERA_KEYS {
        let row = adw::ActionRow::new();
        row.set_title(label);
        row.add_css_class("property");
        row.set_subtitle(d.exif.get(key).map(String::as_str).unwrap_or("-"));
        group.add(&row);
    }
    group
}

fn author_group(d: &InfoData, sender: &ComponentSender<AppModel>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Author");
    // (sidecar field, EXIF fallback key, label, message field id)
    let fields = [
        ("title", "ImageDescription", "Title"),
        ("artist", "Artist", "Author"),
        ("copyright", "Copyright", "Copyright"),
        ("comment", "UserComment", "Comments"),
    ];
    for (field, exif_key, label) in fields {
        let row = adw::EntryRow::new();
        row.set_title(label);
        row.set_show_apply_button(true);
        // Sidecar value wins; fall back to the file's EXIF.
        let sidecar_val = match field {
            "title" => d.meta.title.clone(),
            "artist" => d.meta.artist.clone(),
            "copyright" => d.meta.copyright.clone(),
            "comment" => d.meta.comment.clone(),
            _ => None,
        };
        let initial = sidecar_val
            .or_else(|| d.exif.get(exif_key).cloned())
            .unwrap_or_default();
        row.set_text(&initial);
        let sender = sender.clone();
        row.connect_apply(move |r| {
            sender.input(AppMsg::SetMetaField(field, r.text().to_string()));
        });
        group.add(&row);
    }
    group
}

fn organization_group(d: &InfoData, sender: &ComponentSender<AppModel>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Organization");

    // Rating: five toggle stars.
    let rating_row = adw::ActionRow::new();
    rating_row.set_title("Rating");
    let stars = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    stars.set_valign(gtk::Align::Center);
    for n in 1..=5u8 {
        let b = gtk::Button::new();
        b.set_icon_name(if n <= d.rating {
            "starred-symbolic"
        } else {
            "non-starred-symbolic"
        });
        b.add_css_class("flat");
        let sender = sender.clone();
        let cur = d.rating;
        // Click a filled star at the current rating to clear it (toggle-off).
        b.connect_clicked(move |_| {
            let v = if n == cur { 0 } else { n };
            sender.input(AppMsg::RateActive(v));
        });
        stars.append(&b);
    }
    rating_row.add_suffix(&stars);
    group.add(&rating_row);

    // Colour label: swatches + a clear button.
    let color_row = adw::ActionRow::new();
    color_row.set_title("Colour label");
    let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    swatches.set_valign(gtk::Align::Center);
    let clear = gtk::Button::from_icon_name("edit-clear-symbolic");
    clear.add_css_class("flat");
    clear.set_tooltip_text(Some("No label"));
    {
        let sender = sender.clone();
        clear.connect_clicked(move |_| sender.input(AppMsg::SetColorLabel(None)));
    }
    swatches.append(&clear);
    for &(cname, hex) in COLOR_LABELS {
        let b = gtk::Button::new();
        b.add_css_class("flat");
        b.set_tooltip_text(Some(cname));
        let selected = d.meta.color.as_deref() == Some(cname);
        let glyph = if selected { "●" } else { "○" };
        let dot = gtk::Label::new(None);
        dot.set_markup(&format!("<span foreground='{hex}' size='x-large'>{glyph}</span>"));
        b.set_child(Some(&dot));
        let sender = sender.clone();
        b.connect_clicked(move |_| sender.input(AppMsg::SetColorLabel(Some(cname.to_string()))));
        swatches.append(&b);
    }
    color_row.add_suffix(&swatches);
    group.add(&color_row);

    // Tags: existing chips, then an add entry.
    if !d.meta.tags.is_empty() {
        let chips = gtk::FlowBox::new();
        chips.set_selection_mode(gtk::SelectionMode::None);
        chips.set_column_spacing(4);
        chips.set_row_spacing(4);
        chips.set_margin_top(2);
        chips.set_margin_bottom(2);
        for tag in &d.meta.tags {
            let chip = gtk::Button::with_label(&format!("{tag}  ✕"));
            chip.add_css_class("pill");
            chip.set_tooltip_text(Some("Remove tag"));
            let sender = sender.clone();
            let tag = tag.clone();
            chip.connect_clicked(move |_| sender.input(AppMsg::RemoveMetaTag(tag.clone())));
            chips.append(&chip);
        }
        let chips_row = adw::ActionRow::new();
        chips_row.set_title("Tags");
        chips_row.set_child(Some(&chips));
        group.add(&chips_row);
    }
    let add = adw::EntryRow::new();
    add.set_title("Add tag");
    add.set_show_apply_button(true);
    let sender_add = sender.clone();
    add.connect_apply(move |r| {
        let t = r.text().to_string();
        if !t.trim().is_empty() {
            sender_add.input(AppMsg::AddMetaTag(t));
            r.set_text("");
        }
    });
    group.add(&add);
    group
}

struct Gps {
    lat: f64,
    lon: f64,
    alt: Option<String>,
}

/// Parse the kamadak GPS display strings ("X deg Y min Z sec" + N/S/E/W ref)
/// into signed decimal degrees. `None` if absent/unparseable.
fn parse_gps(exif: &BTreeMap<String, String>) -> Option<Gps> {
    let dms = |s: &str| -> Option<f64> {
        // e.g. "45 deg 28 min 12.3 sec"
        let nums: Vec<f64> = s
            .split_whitespace()
            .filter_map(|w| w.parse::<f64>().ok())
            .collect();
        match nums.as_slice() {
            [d, m, sec, ..] => Some(d + m / 60.0 + sec / 3600.0),
            [d, m] => Some(d + m / 60.0),
            [d] => Some(*d),
            _ => None,
        }
    };
    let lat = dms(exif.get("GPSLatitude")?)?;
    let lon = dms(exif.get("GPSLongitude")?)?;
    let lat = if exif.get("GPSLatitudeRef").map(|s| s.to_uppercase()).as_deref() == Some("S") {
        -lat
    } else {
        lat
    };
    let lon = if exif.get("GPSLongitudeRef").map(|s| s.to_uppercase()).as_deref() == Some("W") {
        -lon
    } else {
        lon
    };
    Some(Gps {
        lat,
        lon,
        alt: exif.get("GPSAltitude").cloned(),
    })
}

fn gps_group(gps: &Gps) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("GPS");

    let coords = adw::ActionRow::new();
    coords.set_title("Coordinates");
    coords.add_css_class("property");
    coords.set_subtitle(&format!("{:.6}, {:.6}", gps.lat, gps.lon));

    // "Open in Maps" menu: OSM / Google / Apple. Cleaner than a global setting —
    // the user picks per click.
    let menu = gtk::MenuButton::new();
    menu.set_icon_name("mark-location-symbolic");
    menu.set_tooltip_text(Some("Open in maps"));
    menu.add_css_class("flat");
    menu.set_valign(gtk::Align::Center);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.set_margin_all(4);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&list));
    let (lat, lon) = (gps.lat, gps.lon);
    let providers = [
        ("OpenStreetMap", format!("https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=15/{lat}/{lon}")),
        ("Google Maps", format!("https://www.google.com/maps/search/?api=1&query={lat},{lon}")),
        ("Apple Maps", format!("https://maps.apple.com/?ll={lat},{lon}")),
    ];
    for (label, url) in providers {
        let item = gtk::Button::with_label(label);
        item.add_css_class("flat");
        item.set_halign(gtk::Align::Fill);
        let pop = pop.clone();
        item.connect_clicked(move |_| {
            pop.popdown();
            let _ = gtk::gio::AppInfo::launch_default_for_uri(
                &url,
                gtk::gio::AppLaunchContext::NONE,
            );
        });
        list.append(&item);
    }
    menu.set_popover(Some(&pop));
    coords.add_suffix(&menu);
    group.add(&coords);

    if let Some(alt) = &gps.alt {
        let row = adw::ActionRow::new();
        row.set_title("Altitude");
        row.add_css_class("property");
        row.set_subtitle(alt);
        group.add(&row);
    }
    group
}

fn extended_group(d: &InfoData) -> Option<adw::PreferencesGroup> {
    let entries: Vec<(&String, &String)> = d
        .exif
        .iter()
        .filter(|(k, _)| !HANDLED_KEYS.contains(&k.as_str()) && *k != "DateTimeOriginal")
        .collect();
    if entries.is_empty() {
        return None;
    }
    let group = adw::PreferencesGroup::new();
    group.set_title("Metadata");
    for (k, v) in entries {
        let row = adw::ActionRow::new();
        row.set_title(k);
        row.add_css_class("property");
        row.set_subtitle(v);
        // Copy the value to the clipboard.
        let copy = gtk::Button::from_icon_name("edit-copy-symbolic");
        copy.add_css_class("flat");
        copy.set_valign(gtk::Align::Center);
        copy.set_tooltip_text(Some("Copy"));
        let val = v.clone();
        copy.connect_clicked(move |b| {
            b.clipboard().set_text(&val);
        });
        row.add_suffix(&copy);
        group.add(&row);
    }
    Some(group)
}
