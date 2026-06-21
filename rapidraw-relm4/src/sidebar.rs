use std::collections::HashSet;
use std::path::PathBuf;

use adw::prelude::*;
use rapidraw_core::folders::list_subdirs;
use relm4::prelude::*;

#[derive(Debug)]
pub enum SidebarOut {
    SelectFolder(PathBuf),
    AddRootFolder,
}

#[derive(Debug)]
pub enum SidebarIn {
    /// Current root folder changed; rebuild the tree (None = no folder open).
    SetRoot(Option<PathBuf>),
    ToggleFolder(PathBuf),
    SelectFolder(PathBuf),
    Search(String),
}

pub struct Sidebar {
    root: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    search: String,
    /// Container the folder rows are rebuilt into.
    folders_box: gtk::Box,
}

#[relm4::component(pub)]
impl Component for Sidebar {
    type Init = ();
    type Input = SidebarIn;
    type Output = SidebarOut;
    type CommandOutput = ();

    view! {
        gtk::Box {
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
        }
    }

    fn init(
        _init: Self::Init,
        root_widget: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let folders_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let model = Sidebar {
            root: None,
            expanded: HashSet::new(),
            search: String::new(),
            folders_box: folders_box.clone(),
        };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            SidebarIn::SetRoot(root) => {
                self.root = root.clone();
                self.expanded.clear();
                if let Some(r) = &root {
                    self.expanded.insert(r.clone());
                }
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
        let Some(root) = self.root.clone() else { return };
        let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("/").to_string();
        self.add_row(sender, &root, &name, 0, true, 0);
        if self.expanded.contains(&root) {
            self.add_children(sender, &root, 1);
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
            let arrow = gtk::Button::builder()
                .icon_name(if expanded { "pan-down-symbolic" } else { "pan-end-symbolic" })
                .css_classes(["flat", "circular"])
                .build();
            let s = sender.clone();
            let p = path.clone();
            arrow.connect_clicked(move |_| s.input(SidebarIn::ToggleFolder(p.clone())));
            row.append(&arrow);
        } else {
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_width_request(24);
            row.append(&spacer);
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

        self.folders_box.append(&row);
    }
}
