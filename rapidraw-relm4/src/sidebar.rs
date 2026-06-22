use std::collections::HashSet;
use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::IsA;
use rapidraw_core::albums::AlbumItem;
use rapidraw_core::folders::list_subdirs;
use relm4::prelude::*;

#[derive(Debug)]
pub enum SidebarOut {
    SelectFolder(PathBuf),
    AddRootFolder,
    RemoveRootFolder(PathBuf),
    SelectAlbum(Vec<String>),
    NewAlbum(String),
    RenameAlbum { id: String, name: String },
    DeleteAlbum(String),
}

#[derive(Debug)]
pub enum SidebarIn {
    /// Replace the set of root folders shown (stacked). Newly-introduced roots are
    /// expanded by default; collapse state of existing roots is preserved.
    SetRoots(Vec<PathBuf>),
    ToggleFolder(PathBuf),
    SelectFolder(PathBuf),
    Search(String),
    SetAlbums(Vec<AlbumItem>),
    ActivateAlbum(String),
}

pub struct Sidebar {
    roots: Vec<PathBuf>,
    expanded: HashSet<PathBuf>,
    search: String,
    /// Container the folder rows are rebuilt into.
    folders_box: gtk::Box,
    albums: Vec<AlbumItem>,
    albums_box: gtk::Box,
}

#[relm4::component(pub)]
impl Component for Sidebar {
    type Init = ();
    type Input = SidebarIn;
    type Output = SidebarOut;
    type CommandOutput = ();

    view! {
        // ToolbarView + empty HeaderBar so the start-side window controls
        // (macOS traffic lights) get reserved space instead of overlapping
        // the search entry. Matches the header bars on the right panels.
        // Only macOS puts its controls here (left edge); elsewhere the controls
        // live on the right panel, so showing them here too would draw a stray
        // close button mid-window.
        adw::ToolbarView {
            add_top_bar = &adw::HeaderBar {
                set_show_start_title_buttons: cfg!(target_os = "macos"),
                set_show_end_title_buttons: false,
                #[wrap(Some)]
                set_title_widget = &gtk::Box {},
            },
            #[wrap(Some)]
            set_content = &gtk::Box {
            set_orientation: gtk::Orientation::Vertical,
            set_spacing: 6,
            set_margin_all: 6,

            gtk::SearchEntry {
                set_placeholder_text: Some("Search folders"),
                connect_search_changed[sender] => move |e| {
                    sender.input(SidebarIn::Search(e.text().to_string()));
                },
            },

            gtk::Label {
                set_xalign: 0.0,
                set_label: "FOLDERS",
                add_css_class: "caption-heading",
                add_css_class: "dim-label",
            },

            gtk::ScrolledWindow {
                set_vexpand: true,
                set_hscrollbar_policy: gtk::PolicyType::Never,
                // Non-overlay: scrollbar takes its own gutter so it never floats
                // over the row's remove button at the right edge.
                set_overlay_scrolling: false,
                #[local_ref]
                folders_box -> gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_spacing: 2,
                },
            },

            gtk::Button {
                set_halign: gtk::Align::Start,
                add_css_class: "flat",
                set_label: "Add folder",
                connect_clicked[sender] => move |_| {
                    let _ = sender.output(SidebarOut::AddRootFolder);
                },
            },

            gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                gtk::Label {
                    set_xalign: 0.0,
                    set_hexpand: true,
                    set_label: "ALBUMS",
                    add_css_class: "caption-heading",
                    add_css_class: "dim-label",
                },
                gtk::Button {
                    add_css_class: "flat",
                    set_icon_name: "list-add-symbolic",
                    set_tooltip_text: Some("New album"),
                    connect_clicked[sender] => move |b| {
                        let s = sender.clone();
                        ask_name(b, "New album", "", move |name| {
                            let _ = s.output(SidebarOut::NewAlbum(name));
                        });
                    },
                },
            },
            gtk::ScrolledWindow {
                set_vexpand: true,
                set_hscrollbar_policy: gtk::PolicyType::Never,
                // Non-overlay: scrollbar takes its own gutter so it never floats
                // over the row's remove button at the right edge.
                set_overlay_scrolling: false,
                #[local_ref]
                albums_box -> gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_spacing: 2,
                },
            },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root_widget: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let folders_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let albums_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let model = Sidebar {
            roots: Vec::new(),
            expanded: HashSet::new(),
            search: String::new(),
            folders_box: folders_box.clone(),
            albums: Vec::new(),
            albums_box: albums_box.clone(),
        };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            SidebarIn::SetRoots(roots) => {
                for r in &roots {
                    if !self.roots.contains(r) {
                        self.expanded.insert(r.clone());
                    }
                }
                self.roots = roots;
            }
            SidebarIn::ToggleFolder(p) => {
                if !self.expanded.remove(&p) {
                    self.expanded.insert(p);
                }
            }
            SidebarIn::SelectFolder(p) => {
                let _ = sender.output(SidebarOut::SelectFolder(p));
                return;
            }
            SidebarIn::Search(q) => {
                self.search = q.to_lowercase();
            }
            SidebarIn::SetAlbums(items) => {
                self.albums = items;
                self.rebuild_albums(&sender);
                return;
            }
            SidebarIn::ActivateAlbum(id) => {
                let images = rapidraw_core::albums::album_images(&self.albums, &id)
                    .map(|s| s.to_vec())
                    .unwrap_or_default();
                let _ = sender.output(SidebarOut::SelectAlbum(images));
                return;
            }
        }
        self.rebuild(&sender);
    }
}

impl Sidebar {
    /// Clear + rebuild folder rows from `root`, honoring expansion + the search filter.
    // ponytail: rebuild-the-whole-tree on each toggle is O(visible rows); folder trees are
    // small. Switch to gtk::TreeListModel only if a directory holds thousands of entries.
    fn rebuild(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.folders_box.first_child() {
            self.folders_box.remove(&child);
        }
        for root in &self.roots {
            let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("/").to_string();
            self.add_row(sender, root, &name, 0, true, 0);
            if self.expanded.contains(root) {
                self.add_children(sender, root, 1);
            }
        }
    }

    fn add_children(&self, sender: &ComponentSender<Self>, dir: &PathBuf, depth: i32) {
        for node in list_subdirs(dir) {
            let matches = self.search.is_empty() || node.name.to_lowercase().contains(&self.search);
            if matches {
                self.add_row(sender, &node.path, &node.name, depth, node.has_subdirs, node.image_count);
            }
            if node.has_subdirs && (self.expanded.contains(&node.path) || !self.search.is_empty()) {
                self.add_children(sender, &node.path, depth + 1);
            }
        }
    }

    fn add_row(
        &self,
        sender: &ComponentSender<Self>,
        path: &PathBuf,
        name: &str,
        depth: i32,
        has_subdirs: bool,
        image_count: u32,
    ) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.set_margin_start(depth * 12);

        let expanded = self.expanded.contains(path);
        if has_subdirs {
            // Folder icon doubles as the expand/collapse toggle (open vs closed).
            let toggle = gtk::Button::builder()
                .icon_name(if expanded { "folder-open-symbolic" } else { "folder-symbolic" })
                .css_classes(["flat", "circular"])
                .build();
            let s = sender.clone();
            let p = path.clone();
            toggle.connect_clicked(move |_| s.input(SidebarIn::ToggleFolder(p.clone())));
            row.append(&toggle);
        } else {
            let icon = gtk::Image::from_icon_name("folder-symbolic");
            icon.set_margin_start(6);
            icon.set_margin_end(6);
            row.append(&icon);
        }

        let label = if image_count > 0 {
            format!("{name}  ({image_count})")
        } else {
            name.to_string()
        };
        let btn = gtk::Button::builder()
            .label(&label)
            .css_classes(["flat"])
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .build();
        if let Some(child) = btn.child().and_downcast::<gtk::Label>() {
            child.set_xalign(0.0);
            child.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        }
        let s = sender.clone();
        let p = path.clone();
        btn.connect_clicked(move |_| s.input(SidebarIn::SelectFolder(p.clone())));
        row.append(&btn);

        // Root rows (depth 0) get a remove button: un-lists the folder (does
        // not touch files on disk), so no confirmation needed.
        if depth == 0 {
            let remove = gtk::Button::builder()
                .icon_name("list-remove-symbolic")
                .css_classes(["flat", "circular"])
                .tooltip_text("Remove folder from sidebar (does not delete files)")
                .valign(gtk::Align::Center)
                .build();
            let s = sender.clone();
            let p = path.clone();
            remove.connect_clicked(move |_| {
                let _ = s.output(SidebarOut::RemoveRootFolder(p.clone()));
            });
            row.append(&remove);
        }

        self.folders_box.append(&row);
    }

    fn rebuild_albums(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.albums_box.first_child() {
            self.albums_box.remove(&child);
        }
        for item in &self.albums {
            self.add_album_row(sender, item);
        }
    }

    fn add_album_row(&self, sender: &ComponentSender<Self>, item: &AlbumItem) {
        match item {
            AlbumItem::Album { id, name, .. } => {
                let row = adw::ActionRow::builder().title(name).activatable(true).build();
                row.add_prefix(&gtk::Image::from_icon_name("emblem-photos-symbolic"));
                {
                    let s = sender.clone();
                    let id = id.clone();
                    row.connect_activated(move |_| s.input(SidebarIn::ActivateAlbum(id.clone())));
                }
                // overflow menu: rename / delete
                let menu_btn = gtk::MenuButton::builder()
                    .icon_name("view-more-symbolic")
                    .css_classes(["flat"])
                    .valign(gtk::Align::Center)
                    .build();
                let pop = gtk::Popover::new();
                let vb = gtk::Box::new(gtk::Orientation::Vertical, 2);
                let rename = gtk::Button::builder().label("Rename").css_classes(["flat"]).build();
                let delete = gtk::Button::builder().label("Delete").css_classes(["flat"]).build();
                vb.append(&rename);
                vb.append(&delete);
                pop.set_child(Some(&vb));
                menu_btn.set_popover(Some(&pop));
                {
                    let s = sender.clone();
                    let id = id.clone();
                    let cur = name.clone();
                    let pop = pop.clone();
                    rename.connect_clicked(move |btn| {
                        pop.popdown();
                        let s2 = s.clone();
                        let id2 = id.clone();
                        ask_name(btn, "Rename album", &cur, move |name| {
                            let _ = s2.output(SidebarOut::RenameAlbum { id: id2.clone(), name });
                        });
                    });
                }
                {
                    let s = sender.clone();
                    let id = id.clone();
                    let pop = pop.clone();
                    delete.connect_clicked(move |_| {
                        pop.popdown();
                        let _ = s.output(SidebarOut::DeleteAlbum(id.clone()));
                    });
                }
                row.add_suffix(&menu_btn);
                self.albums_box.append(&row);
            }
            AlbumItem::Group { name, children, .. } => {
                let exp = adw::ExpanderRow::builder().title(name).build();
                for child in children {
                    if let AlbumItem::Album { id, name, .. } = child {
                        let crow = adw::ActionRow::builder().title(name).activatable(true).build();
                        let s = sender.clone();
                        let id = id.clone();
                        crow.connect_activated(move |_| s.input(SidebarIn::ActivateAlbum(id.clone())));
                        exp.add_row(&crow);
                    }
                }
                self.albums_box.append(&exp);
            }
        }
    }
}

/// Show a libadwaita text-entry dialog; calls `on_ok` with the trimmed-nonempty name.
/// Uses MessageDialog (adw v1_2+) because AlertDialog needs v1_5 which isn't enabled.
fn ask_name(anchor: &impl IsA<gtk::Widget>, title: &str, initial: &str, on_ok: impl Fn(String) + 'static) {
    let window = anchor.root().and_downcast::<gtk::Window>();
    let dialog = adw::MessageDialog::new(window.as_ref(), Some(title), None);
    let entry = gtk::Entry::builder().text(initial).build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("ok", "OK");
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, resp| {
        if resp == "ok" {
            let name = entry.text().to_string();
            if !name.trim().is_empty() {
                on_ok(name);
            }
        }
    });
    dialog.present();
}
