# relm4 Sidebar (Folders + Albums) and Star Ratings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a collapsible left sidebar (Folders then Albums) shared across the library and editor screens, plus clickable star ratings on thumbnails and in the editor, in the relm4/GTK frontend — reusing `rapidraw-core` for shared logic.

**Architecture:** Wrap the existing `adw::NavigationView` (library + editor pages) inside one `adw::OverlaySplitView` so the sidebar persists across both screens. Move album persistence logic into `rapidraw-core` and add a folder-listing helper there; the Tauri commands become thin wrappers. A reusable `Stars` widget is embedded in both the thumbnail factory and the editor.

**Tech Stack:** Rust, GTK4 + libadwaita (`adw`), relm4 (Component + FactoryComponent), serde_json.

---

## Background facts (verified, for the implementer)

- `rapidraw-core/src/formats.rs` ALREADY provides `is_raw_file<P:AsRef<Path>>(p)->bool` and
  `is_supported_image_file<P:AsRef<Path>>(p)->bool`. Use these; do not invent new extension lists.
- relm4 `src/library.rs` currently has its own `EXT`/`RAW_EXT` constants + `is_raw`. These get
  replaced by the core helpers (Task 4).
- Tauri album logic lives in `src-tauri/src/file_management.rs`:
  - `pub enum AlbumItem` (serde `#[serde(tag="type", rename_all="camelCase")]`, variants
    `Album{id,name,icon,images}` and `Group{id,name,icon,children}`).
  - `pub fn sort_album_tree(items: &mut [AlbumItem])`.
  - `#[tauri::command] pub fn get_albums(app_handle) -> Result<Vec<AlbumItem>,String>`.
  - `#[tauri::command] pub fn save_albums(mut tree, app_handle) -> Result<(),String>`.
  - `#[tauri::command] pub fn add_to_album(album_id, paths, app_handle) -> Result<...>`.
  - `#[tauri::command] pub fn get_album_images(...)`.
  - `fn get_albums_path(app_handle) -> Result<PathBuf,String>` resolves `<app_data>/albums/albums.json`.
  - Commands registered in `src-tauri/src/lib.rs` (~line 2304).
- relm4 ratings already work: `AppModel.ratings: HashMap<PathBuf,u8>`, `AppMsg::RateActive(u8)`
  (main.rs ~line 297, applied ~2115), `save_ratings`/`load_ratings`, `thumb.rs` shows a read-only
  `★★★☆☆` label and has `ThumbMsg::SetRating(u8)`.
- relm4 config dir helper pattern: see `ratings_file()` / `settings_file()` in main.rs (~2863).
- The grid is rebuilt by an EXISTING method `AppModel::apply_library(&mut self, sender: &ComponentSender<AppModel>)`
  (main.rs ~729): it arranges `self.all_images` (scanned) → `self.images` (filtered/sorted) and
  rebuilds the `thumbs` factory. `self.all_images` = scanned set, `self.images` = arranged set,
  `self.images_shared: Rc<RefCell<Vec<PathBuf>>>` = shared copy for the FlowBox activation
  closure. Reuse `apply_library`; do NOT write a new grid-rebuild method.
- adw crate = `libadwaita 0.7` with feature `v1_4` (`rapidraw-relm4/Cargo.toml`): `OverlaySplitView`
  and `AlertDialog` are available. relm4 = `0.9`.
- relm4 0.9 `Component` trait: `fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>, _root: &Self::Root)`.
  No existing relm4 `Component` lives in this repo (panels like `AdjustPanel` are plain structs),
  so `Sidebar` and `Stars` are the first — follow the standard relm4 0.9 `#[relm4::component(pub)]`
  pattern shown in this plan.
- relm4 view tree: `adw::ApplicationWindow > [name=toast_overlay] adw::ToastOverlay >
  [name=nav] adw::NavigationView { page tag "library", page tag "editor" }`.

## File structure

- `rapidraw-core/src/albums.rs` — NEW. `AlbumItem` + pure album tree ops + json load/save.
- `rapidraw-core/src/folders.rs` — NEW. `FolderNode` + `list_subdirs`.
- `rapidraw-core/src/lib.rs` — MODIFY. Register the two new modules.
- `src-tauri/src/file_management.rs` — MODIFY. Re-export `AlbumItem` from core; command bodies
  call core fns.
- `rapidraw-relm4/src/library.rs` — MODIFY. Use core formats; drop local EXT lists.
- `rapidraw-relm4/src/stars.rs` — NEW. Reusable clickable star widget (relm4 Component).
- `rapidraw-relm4/src/sidebar.rs` — NEW. Sidebar Component (folders, then albums).
- `rapidraw-relm4/src/thumb.rs` — MODIFY. Clickable stars overlay instead of read-only label.
- `rapidraw-relm4/src/main.rs` — MODIFY. OverlaySplitView shell, toggle buttons, sidebar wiring,
  album state + persistence, editor stars.

---

## Task 1: Core albums module

**Files:**
- Create: `rapidraw-core/src/albums.rs`
- Modify: `rapidraw-core/src/lib.rs`
- Test: inline `#[cfg(test)]` in `rapidraw-core/src/albums.rs`

- [ ] **Step 1: Register the module**

In `rapidraw-core/src/lib.rs`, add near the other `pub mod` lines (after `pub mod formats;`):

```rust
pub mod albums;
pub mod folders;
```

(`folders` is created in Task 2; adding both now avoids a second edit. The crate will not
compile until Task 2 creates `folders.rs`, so commit Tasks 1 and 2 together — see Task 2 Step 5.)

- [ ] **Step 2: Write `albums.rs` with the moved logic**

Create `rapidraw-core/src/albums.rs`:

```rust
use std::path::Path;

use serde::{Deserialize, Serialize};

/// An entry in the album tree: either a leaf album (a named list of image paths) or a
/// group containing further entries. Serde shape matches the original Tauri format
/// (`albums.json`), so files are interchangeable between frontends.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AlbumItem {
    Album {
        id: String,
        name: String,
        icon: Option<String>,
        images: Vec<String>,
    },
    Group {
        id: String,
        name: String,
        icon: Option<String>,
        children: Vec<AlbumItem>,
    },
}

/// Sort groups before albums, then by case-insensitive name, recursively.
pub fn sort_album_tree(items: &mut [AlbumItem]) {
    items.sort_by(|a, b| {
        let key = |item: &AlbumItem| match item {
            AlbumItem::Group { name, .. } => (0, name.to_lowercase()),
            AlbumItem::Album { name, .. } => (1, name.to_lowercase()),
        };
        key(a).cmp(&key(b))
    });
    for item in items.iter_mut() {
        if let AlbumItem::Group { children, .. } = item {
            sort_album_tree(children);
        }
    }
}

/// Load + sort the album tree from `path`. Returns an empty tree if the file is missing.
/// A corrupt file is treated as empty (logged) rather than an error, so the UI never breaks.
pub fn load_albums(path: &Path) -> Vec<AlbumItem> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<AlbumItem>>(&content) {
        Ok(mut items) => {
            sort_album_tree(&mut items);
            items
        }
        Err(e) => {
            log::warn!("albums.json parse failed: {e}");
            Vec::new()
        }
    }
}

/// Sort + write the album tree to `path` (pretty JSON). Creates parent dirs.
pub fn save_albums(path: &Path, tree: &mut Vec<AlbumItem>) -> Result<(), String> {
    sort_album_tree(tree);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(tree).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Append `paths` to the album with `album_id` (dedup, preserving order). Returns true if
/// the album was found. Searches recursively into groups.
pub fn add_to_album(tree: &mut [AlbumItem], album_id: &str, paths: &[String]) -> bool {
    for item in tree.iter_mut() {
        match item {
            AlbumItem::Album { id, images, .. } if id == album_id => {
                for p in paths {
                    if !images.contains(p) {
                        images.push(p.clone());
                    }
                }
                return true;
            }
            AlbumItem::Group { children, .. } => {
                if add_to_album(children, album_id, paths) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// The image list of the album with `album_id`, searching recursively. None if not found.
pub fn album_images<'a>(tree: &'a [AlbumItem], album_id: &str) -> Option<&'a [String]> {
    for item in tree {
        match item {
            AlbumItem::Album { id, images, .. } if id == album_id => return Some(images),
            AlbumItem::Group { children, .. } => {
                if let Some(found) = album_images(children, album_id) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, name: &str, imgs: &[&str]) -> AlbumItem {
        AlbumItem::Album {
            id: id.into(),
            name: name.into(),
            icon: None,
            images: imgs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn sort_groups_before_albums_then_name() {
        let mut t = vec![
            album("a2", "Zebra", &[]),
            AlbumItem::Group { id: "g1".into(), name: "beta".into(), icon: None, children: vec![] },
            album("a1", "alpha", &[]),
        ];
        sort_album_tree(&mut t);
        // group first, then albums by lowercased name (alpha, Zebra)
        assert!(matches!(t[0], AlbumItem::Group { .. }));
        assert!(matches!(&t[1], AlbumItem::Album { name, .. } if name == "alpha"));
        assert!(matches!(&t[2], AlbumItem::Album { name, .. } if name == "Zebra"));
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rr_albums_{}", std::process::id()));
        let path = dir.join("albums.json");
        let mut tree = vec![album("a1", "Trip", &["/x/1.jpg"])];
        save_albums(&path, &mut tree).unwrap();
        let loaded = load_albums(&path);
        assert_eq!(loaded, tree);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_is_empty() {
        assert!(load_albums(Path::new("/no/such/albums.json")).is_empty());
    }

    #[test]
    fn add_to_album_dedups_and_reports_missing() {
        let mut tree = vec![AlbumItem::Group {
            id: "g".into(),
            name: "g".into(),
            icon: None,
            children: vec![album("a1", "A", &["/x/1.jpg"])],
        }];
        assert!(add_to_album(&mut tree, "a1", &["/x/1.jpg".into(), "/x/2.jpg".into()]));
        assert_eq!(album_images(&tree, "a1").unwrap(), &["/x/1.jpg", "/x/2.jpg"]);
        assert!(!add_to_album(&mut tree, "nope", &["/x/3.jpg".into()]));
    }
}
```

- [ ] **Step 3: (deferred build)** Do not build yet — the crate references `folders` (Task 1
  Step 1) which is created in Task 2. Build happens at the end of Task 2.

- [ ] **Step 4: (no commit yet)** Commit with Task 2.

---

## Task 2: Core folders helper

**Files:**
- Create: `rapidraw-core/src/folders.rs`
- Test: inline `#[cfg(test)]` in `rapidraw-core/src/folders.rs`

- [ ] **Step 1: Write `folders.rs`**

Create `rapidraw-core/src/folders.rs`:

```rust
use std::path::{Path, PathBuf};

use crate::formats::is_supported_image_file;

/// A directory entry for the sidebar folder tree.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderNode {
    pub name: String,
    pub path: PathBuf,
    /// True if this directory contains at least one sub-directory (lets the UI show an
    /// expander arrow without recursing eagerly).
    pub has_subdirs: bool,
    /// Count of supported image files directly inside this directory (non-recursive).
    pub image_count: u32,
}

/// Direct child directories of `dir`, sorted by case-insensitive name. Returns empty on
/// an unreadable directory (mirrors the relm4 scanner's defensive behavior).
pub fn list_subdirs(dir: &Path) -> Vec<FolderNode> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut nodes: Vec<FolderNode> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .map(|p| {
            let (has_subdirs, image_count) = scan_dir_meta(&p);
            FolderNode {
                name: p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
                path: p,
                has_subdirs,
                image_count,
            }
        })
        .collect();
    nodes.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    nodes
}

/// (has at least one subdir, count of supported images) for a directory, one level deep.
fn scan_dir_meta(dir: &Path) -> (bool, u32) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return (false, 0);
    };
    let mut has_subdirs = false;
    let mut count = 0u32;
    for entry in rd.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir() {
            has_subdirs = true;
        } else if is_supported_image_file(&p) {
            count += 1;
        }
    }
    (has_subdirs, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_child_dirs_with_meta() {
        let base = std::env::temp_dir().join(format!("rr_folders_{}", std::process::id()));
        let child = base.join("child");
        let grand = child.join("grand");
        std::fs::create_dir_all(&grand).unwrap();
        std::fs::write(child.join("a.jpg"), b"x").unwrap();
        std::fs::write(child.join("notes.txt"), b"x").unwrap();

        let nodes = list_subdirs(&base);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "child");
        assert!(nodes[0].has_subdirs); // because of grand/
        assert_eq!(nodes[0].image_count, 1); // a.jpg only, not notes.txt

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unreadable_dir_is_empty() {
        assert!(list_subdirs(Path::new("/no/such/dir")).is_empty());
    }
}
```

- [ ] **Step 2: Build the core crate**

Run: `cargo build -p rapidraw-core`
Expected: compiles (warnings OK).

- [ ] **Step 3: Run the new tests**

Run: `cargo test -p rapidraw-core albums:: folders::`
Expected: all tests in `albums` and `folders` modules PASS.

- [ ] **Step 4: Commit Tasks 1 + 2**

```bash
git add rapidraw-core/src/albums.rs rapidraw-core/src/folders.rs rapidraw-core/src/lib.rs
git commit -m "feat(core): album tree + folder listing helpers with tests"
```

---

## Task 3: Rewire Tauri album commands to core

**Files:**
- Modify: `src-tauri/src/file_management.rs`

Goal: delete the duplicated `AlbumItem` enum + `sort_album_tree` from `file_management.rs` and
call the core versions, keeping the `#[tauri::command]` signatures and the `AlbumItem` import
path (`file_management::AlbumItem`) unchanged so `lib.rs` and any callers still compile.

- [ ] **Step 1: Re-export `AlbumItem` and remove the local copy**

In `src-tauri/src/file_management.rs`, delete the local `pub enum AlbumItem { ... }` block and
the local `pub fn sort_album_tree(...)` block. Add near the top imports:

```rust
pub use rapidraw_core::albums::{AlbumItem, sort_album_tree};
```

- [ ] **Step 2: Make `get_albums` / `save_albums` call core**

Replace the bodies (keep the `#[tauri::command]` signatures and `get_albums_path` helper):

```rust
#[tauri::command]
pub fn get_albums(app_handle: AppHandle) -> Result<Vec<AlbumItem>, String> {
    let path = get_albums_path(&app_handle)?;
    Ok(rapidraw_core::albums::load_albums(&path))
}

#[tauri::command]
pub fn save_albums(mut tree: Vec<AlbumItem>, app_handle: AppHandle) -> Result<(), String> {
    let path = get_albums_path(&app_handle)?;
    rapidraw_core::albums::save_albums(&path, &mut tree)
}
```

- [ ] **Step 3: Make `add_to_album` use the core helper**

In `add_to_album`, replace the local recursive `add_recursive` closure and its call with the
core function, keeping the load/save-around-it structure:

```rust
#[tauri::command]
pub fn add_to_album(
    album_id: String,
    paths: Vec<String>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let mut tree = get_albums(app_handle.clone())?;
    if rapidraw_core::albums::add_to_album(&mut tree, &album_id, &paths) {
        save_albums(tree, app_handle)?;
    }
    Ok(())
}
```

(If `get_album_images` has a local recursive search, leave it as-is OR swap to
`rapidraw_core::albums::album_images`; either is fine — do not change its command signature.)

- [ ] **Step 4: Build the Tauri crate**

Run: `cargo build -p rapidraw-tauri 2>&1 | tail -20`
(If the package name differs, use the name from `src-tauri/Cargo.toml` `[package] name`.)
Expected: compiles. Fix any leftover references to the removed local items.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/file_management.rs
git commit -m "refactor(tauri): use rapidraw-core album logic instead of local copy"
```

---

## Task 4: relm4 library scanner uses core formats

**Files:**
- Modify: `rapidraw-relm4/src/library.rs`

- [ ] **Step 1: Replace local extension lists with core helpers**

In `rapidraw-relm4/src/library.rs`:

- Delete the `const EXT` and `const RAW_EXT` arrays.
- Replace `pub fn is_raw` with a delegating re-export:

```rust
pub use rapidraw_core::formats::{is_raw_file as is_raw, is_supported_image_file};
```

- In `scan_dir`, change the extension filter to use the core helper:

```rust
.filter(|p| is_supported_image_file(p))
```

(remove the old `.extension()...EXT.contains(...)` closure).

- [ ] **Step 2: Build relm4**

Run: `cargo build -p rapidraw-relm4 2>&1 | tail -20`
Expected: compiles. (`arrange` still calls `is_raw(p)` — now the core fn — and `scan_dir` uses
`is_supported_image_file`.)

- [ ] **Step 3: Commit**

```bash
git add rapidraw-relm4/src/library.rs
git commit -m "refactor(relm4): use rapidraw-core format helpers in library scanner"
```

---

## Task 5: OverlaySplitView shell + sidebar toggle buttons

**Files:**
- Modify: `rapidraw-relm4/src/main.rs`

Goal: wrap the `NavigationView` in an `adw::OverlaySplitView` with a placeholder sidebar, and
add a toggle button to both header bars. No sidebar content yet (proves the shell + toggle).

- [ ] **Step 1: Add the split view to the view tree**

In `main.rs` `view!`, change the `set_content = &adw::ToastOverlay { #[name="nav"] set_child = &adw::NavigationView {...} }`
so the `NavigationView` becomes the *content* of a new split view. Replace:

```rust
                #[wrap(Some)]
                #[name = "nav"]
                set_child = &adw::NavigationView {
```

with:

```rust
                #[wrap(Some)]
                #[name = "split"]
                set_child = &adw::OverlaySplitView {
                    set_min_sidebar_width: 240.0,
                    set_max_sidebar_width: 360.0,
                    set_show_sidebar: true,
                    #[wrap(Some)]
                    #[name = "sidebar_slot"]
                    set_sidebar = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        // placeholder; replaced by the Sidebar component in Task 6
                    },
                    #[wrap(Some)]
                    #[name = "nav"]
                    set_content = &adw::NavigationView {
```

Add the matching extra closing brace for the `OverlaySplitView` after the `NavigationView`'s
closing brace (before the `ToastOverlay` closes). Verify brace balance carefully.

- [ ] **Step 2: Add a toggle button to BOTH header bars**

In the **library** page `HeaderBar` (the one with "Open Folder"), add as the first
`pack_start`:

```rust
                                #[name = "sidebar_toggle_lib"]
                                pack_start = &gtk::ToggleButton {
                                    set_icon_name: "sidebar-show-symbolic",
                                    set_tooltip_text: Some("Toggle sidebar"),
                                    set_active: true,
                                },
```

In the **editor** page `HeaderBar`, add as the first `pack_start` (before the undo/redo box):

```rust
                                #[name = "sidebar_toggle_ed"]
                                pack_start = &gtk::ToggleButton {
                                    set_icon_name: "sidebar-show-symbolic",
                                    set_tooltip_text: Some("Toggle sidebar"),
                                    set_active: true,
                                },
```

- [ ] **Step 3: Wire the toggles to the split view (in `init`, after `widgets` exist)**

In `init`, after the widgets are built (near where other post-build wiring lives, e.g. after
`widgets.nav.replace_with_tags(...)`), add:

```rust
        {
            let split = widgets.split.clone();
            let other = widgets.sidebar_toggle_ed.clone();
            widgets.sidebar_toggle_lib.connect_toggled(move |b| {
                split.set_show_sidebar(b.is_active());
                if other.is_active() != b.is_active() {
                    other.set_active(b.is_active());
                }
            });
        }
        {
            let split = widgets.split.clone();
            let other = widgets.sidebar_toggle_lib.clone();
            widgets.sidebar_toggle_ed.connect_toggled(move |b| {
                split.set_show_sidebar(b.is_active());
                if other.is_active() != b.is_active() {
                    other.set_active(b.is_active());
                }
            });
        }
```

(Keeping both toggles in sync avoids a stale-looking button after switching screens.)

- [ ] **Step 4: Build + run, verify manually**

Run: `cargo run -p rapidraw-relm4 2>&1 | tail -20`
Expected: app launches; an empty sidebar panel shows on the left of the grid; clicking either
toggle hides/shows it; opening an image into the editor keeps the sidebar visible and the
editor's toggle works too.

- [ ] **Step 5: Commit**

```bash
git add rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): OverlaySplitView shell with sidebar toggle on both screens"
```

---

## Task 6: Sidebar component — Folders section

**Files:**
- Create: `rapidraw-relm4/src/sidebar.rs`
- Modify: `rapidraw-relm4/src/main.rs` (declare `mod sidebar;`, instantiate, place in `sidebar_slot`)

The folder tree is built by recursively constructing widgets into a `gtk::Box`, rebuilt on
expand/collapse. Expansion state is a `HashSet<PathBuf>` in the component.
`// ponytail: rebuild-the-tree-on-toggle is O(n) in visible rows; folder trees are small, switch
to gtk::TreeListModel only if a directory has thousands of entries.`

- [ ] **Step 1: Write `sidebar.rs` (folders only; albums added in Task 11)**

Create `rapidraw-relm4/src/sidebar.rs`:

```rust
use std::collections::HashSet;
use std::path::PathBuf;

use adw::prelude::*;
use gtk::glib;
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
    /// The container the folder rows are rebuilt into.
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
                set_icon_name: "list-add-symbolic",
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
                return; // selection doesn't change the tree shape
            }
            SidebarIn::Search(q) => {
                self.search = q.to_lowercase();
            }
        }
        self.rebuild(&sender);
    }
}

impl Sidebar {
    /// Clear and rebuild the folder rows from `root`, honoring expansion + search filter.
    fn rebuild(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.folders_box.first_child() {
            self.folders_box.remove(&child);
        }
        let Some(root) = self.root.clone() else { return };
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("/")
            .to_string();
        // The root itself is the top row (depth 0), then its subtree.
        self.add_row(sender, &root, &name, 0, true, 0);
        if self.expanded.contains(&root) {
            self.add_children(sender, &root, 1);
        }
    }

    fn add_children(&self, sender: &ComponentSender<Self>, dir: &PathBuf, depth: i32) {
        for node in list_subdirs(dir) {
            if !self.search.is_empty() && !node.name.to_lowercase().contains(&self.search) {
                // When searching, still descend so matching descendants show, but skip the
                // non-matching row only if it has no matching descendants. Lazy approach:
                // show all rows whose name matches; otherwise show if expanded ancestor.
                // ponytail: simple substring filter on the row name; good enough for nav.
            }
            let matches = self.search.is_empty() || node.name.to_lowercase().contains(&self.search);
            if matches {
                self.add_row(sender, &node.path, &node.name, depth, node.has_subdirs, node.image_count);
            }
            if (node.has_subdirs) && (self.expanded.contains(&node.path) || !self.search.is_empty()) {
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
            // spacer to keep names aligned
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

// silence unused import if glib ends up unused in this module
#[allow(unused_imports)]
use glib as _glib_keepalive;
```

- [ ] **Step 2: Declare the module and instantiate the component in `main.rs`**

In `main.rs`, add to the `mod` list:

```rust
mod sidebar;
```

and a `use`:

```rust
use sidebar::{Sidebar, SidebarIn, SidebarOut};
```

Add to `AppMsg` (near other variants):

```rust
    /// Sidebar picked a sub-folder to show in the grid (does not change the tree root).
    ShowFolder(PathBuf),
```

Add to `AppModel` struct fields:

```rust
    sidebar: Controller<Sidebar>,
```

In `init`, before constructing `AppModel`, launch the component (it must be created before the
struct literal that moves it in):

```rust
        let sidebar = Sidebar::builder()
            .launch(())
            .forward(sender.input_sender(), |out| match out {
                SidebarOut::SelectFolder(p) => AppMsg::ShowFolder(p),
                SidebarOut::AddRootFolder => AppMsg::OpenFolderDialog,
            });
```

Add `sidebar,` to the `AppModel { ... }` literal.

- [ ] **Step 3: Mount the sidebar widget into the split view slot**

In `init`, after `widgets` exist, replace the placeholder by setting the split view's sidebar to
the component's root widget:

```rust
        widgets.split.set_sidebar(Some(model.sidebar.widget()));
```

(Removes reliance on the `sidebar_slot` placeholder box; that placeholder can stay as the
initial child — `set_sidebar` overrides it.)

- [ ] **Step 4: Feed the root folder to the sidebar on folder open**

In the `AppMsg::FolderChosen(path)` handler (main.rs ~1493), after `self.session.current_folder
= Some(path)` is set, send the root to the sidebar:

```rust
                self.sidebar.emit(SidebarIn::SetRoot(self.session.current_folder.clone()));
```

- [ ] **Step 5: Handle `ShowFolder` (load a sub-folder into the grid)**

Find how `FolderChosen` rebuilds the grid (it calls `scan_dir` + repopulates `self.images`,
`self.images_shared`, and the `thumbs` factory — see main.rs ~735-745 and the `FolderChosen`
handler). Add a handler that does the SAME grid rebuild for an arbitrary directory WITHOUT
calling `save_last_folder` or changing `current_folder`/the sidebar root:

```rust
            AppMsg::ShowFolder(dir) => {
                self.all_images = library::scan_dir(&dir);
                self.apply_library(&sender);
            }
```

This reuses the existing `AppModel::apply_library` (main.rs ~729), which already arranges
`self.all_images` into `self.images` and rebuilds the thumbnail factory. `ShowFolder`
deliberately sets only `self.all_images` (not `self.session.current_folder`, not the sidebar
root) so navigating into a subfolder loads its images without re-rooting the tree.

- [ ] **Step 6: Build + run, verify manually**

Run: `cargo run -p rapidraw-relm4 2>&1 | tail -20`
Expected: opening a folder shows it as the root row in the sidebar; expander arrows reveal
subfolders (with image counts); clicking a subfolder loads its images into the grid; the search
box filters folder names; "Add folder" opens the folder chooser.

- [ ] **Step 7: Commit**

```bash
git add rapidraw-relm4/src/sidebar.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): sidebar folders tree (lazy expand, search, navigate)"
```

---

## Task 7: Reusable clickable Stars widget

**Files:**
- Create: `rapidraw-relm4/src/stars.rs`
- Modify: `rapidraw-relm4/src/main.rs` (declare `mod stars;`)

- [ ] **Step 1: Write `stars.rs`**

Create `rapidraw-relm4/src/stars.rs`:

```rust
use gtk::prelude::*;
use relm4::prelude::*;

/// A row of 5 clickable stars. Clicking star `i` sets the rating to `i`; clicking the star
/// equal to the current rating clears it to 0 (toggle-off).
pub struct Stars {
    rating: u8,
    buttons: Vec<gtk::Button>,
}

#[derive(Debug)]
pub enum StarsMsg {
    /// User clicked star number `n` (1..=5).
    Clicked(u8),
    /// Programmatic sync (e.g. keyboard 0..5 or loading a new image). Does NOT emit output.
    External(u8),
}

#[derive(Debug)]
pub enum StarsOut {
    Changed(u8),
}

#[relm4::component(pub)]
impl Component for Stars {
    type Init = u8;
    type Input = StarsMsg;
    type Output = StarsOut;
    type CommandOutput = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 0,
            add_css_class: "stars",
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let mut buttons = Vec::with_capacity(5);
        for i in 1..=5u8 {
            let b = gtk::Button::builder()
                .css_classes(["flat", "circular"])
                .build();
            let s = sender.clone();
            b.connect_clicked(move |_| s.input(StarsMsg::Clicked(i)));
            root.append(&b);
            buttons.push(b);
        }
        let model = Stars { rating: init, buttons };
        model.render();
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            StarsMsg::Clicked(n) => {
                self.rating = if self.rating == n { 0 } else { n };
                self.render();
                let _ = sender.output(StarsOut::Changed(self.rating));
            }
            StarsMsg::External(n) => {
                self.rating = n.min(5);
                self.render();
            }
        }
    }
}

impl Stars {
    fn render(&self) {
        for (idx, b) in self.buttons.iter().enumerate() {
            let filled = (idx as u8) < self.rating;
            b.set_label(if filled { "★" } else { "☆" });
        }
    }
}
```

- [ ] **Step 2: Declare the module**

In `main.rs`, add `mod stars;` to the module list.

- [ ] **Step 3: Build**

Run: `cargo build -p rapidraw-relm4 2>&1 | tail -20`
Expected: compiles (the widget is unused for now — `#[allow(dead_code)]` is unnecessary because
it is `pub` and a `mod`; if the compiler warns about unused, that is acceptable at this step).

- [ ] **Step 4: Commit**

```bash
git add rapidraw-relm4/src/stars.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): reusable clickable Stars widget"
```

---

## Task 8: Clickable stars on thumbnails

**Files:**
- Modify: `rapidraw-relm4/src/thumb.rs`
- Modify: `rapidraw-relm4/src/main.rs` (route the new factory output)

The factory currently has `type Output = ()`. Change it to emit rating changes, and replace the
read-only star label with 5 clickable star buttons shown over the picture via a `gtk::Overlay`.
`// ponytail: inline 5 buttons rather than embedding the Stars Component — a FactoryComponent
nesting a relm4 Component adds wiring with no payoff for 5 toggle buttons.`

- [ ] **Step 1: Change the factory output type and add a message**

In `thumb.rs`, change:

```rust
    type Output = ();
```

to:

```rust
    type Output = ThumbOut;
```

and add above the impl:

```rust
#[derive(Debug)]
pub enum ThumbOut {
    /// User set this thumbnail's rating via the star strip.
    Rate(PathBuf, u8),
}
```

- [ ] **Step 2: Replace the view with an overlay + clickable stars**

Replace the entire `view!` block in `thumb.rs` with:

```rust
    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Vertical,
            set_spacing: 4,
            set_width_request: 160,

            gtk::Overlay {
                #[name = "picture"]
                gtk::Picture {
                    set_size_request: (150, 150),
                    set_content_fit: gtk::ContentFit::Contain,
                    #[watch]
                    set_paintable: self.texture.as_ref().map(|t| t.upcast_ref::<gdk::Paintable>()),
                },
                add_overlay = &gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::End,
                    set_spacing: 0,
                    add_css_class: "osd",
                    add_css_class: "stars",
                    // five star buttons, built in init_widgets below
                    #[name = "star_box"]
                    gtk::Box {}
                },
            },

            gtk::Label {
                set_ellipsize: gtk::pango::EllipsizeMode::Middle,
                set_max_width_chars: 18,
                #[watch]
                set_label: &self
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string(),
            },
        }
    }
```

- [ ] **Step 3: Build the star buttons in `init_widgets` and keep them in sync**

The relm4 factory macro generates `init_widgets`. To inject manual buttons + a `#[watch]`-style
update, add an `update_view` and build buttons after `view_output!`. Replace the `init_model`
+ `update` section by ALSO implementing the buttons. Concretely, add these to the
`FactoryComponent` impl body (alongside `init_model`/`update`):

```rust
    fn init_widgets(
        &mut self,
        _index: &DynamicIndex,
        root: Self::Root,
        _returned_widget: &gtk::FlowBoxChild,
        sender: FactorySender<Self>,
    ) -> Self::Widgets {
        let widgets = view_output!();
        // Build 5 clickable star buttons into star_box.
        let path = self.path.clone();
        for i in 1..=5u8 {
            let b = gtk::Button::builder().css_classes(["flat"]).build();
            b.set_label("☆");
            let s = sender.clone();
            let p = path.clone();
            b.connect_clicked(move |_| {
                // toggle-off when re-clicking the current rating is handled app-side via
                // current value; here we just request setting to i, and let the app clear
                // if already equal. Simpler: request i; app decides.
                s.output(ThumbOut::Rate(p.clone(), i)).ok();
            });
            widgets.star_box.append(&b);
        }
        render_stars(&widgets.star_box, self.rating);
        widgets
    }

    fn update_view(&self, widgets: &mut Self::Widgets, _sender: FactorySender<Self>) {
        render_stars(&widgets.star_box, self.rating);
    }
```

And add a free helper at the bottom of `thumb.rs`:

```rust
fn render_stars(star_box: &gtk::Box, rating: u8) {
    let mut i = 0u8;
    let mut child = star_box.first_child();
    while let Some(w) = child {
        if let Some(btn) = w.downcast_ref::<gtk::Button>() {
            btn.set_label(if i < rating { "★" } else { "☆" });
        }
        i += 1;
        child = w.next_sibling();
    }
}
```

Remove the now-unused `fn stars(r: u8) -> String` helper.

NOTE: toggle-off (click current rating → 0) is handled in the app handler in Step 4 by
comparing to the stored rating, keeping the thumbnail dumb.

- [ ] **Step 4: Route the factory output in `main.rs`**

The thumbs factory is launched at `init` (main.rs ~1091):

```rust
        let thumbs = FactoryVecDeque::builder()
            .launch(gtk::FlowBox::default())
            .detach();
```

Change `.detach()` to forward output into the app:

```rust
        let thumbs = FactoryVecDeque::builder()
            .launch(gtk::FlowBox::default())
            .forward(sender.input_sender(), |out| match out {
                thumb::ThumbOut::Rate(path, n) => AppMsg::RateThumb(path, n),
            });
```

Add the import: ensure `use thumb::{Thumb, ThumbMsg, ThumbOut};` (extend existing line).

Add `AppMsg::RateThumb`:

```rust
    /// A thumbnail's star strip was clicked: set (or toggle-off) that path's rating.
    RateThumb(PathBuf, u8),
```

Handle it (reusing the existing rating persistence path; mirror the `RateActive` handler logic
at ~2115 but keyed by the clicked path):

```rust
            AppMsg::RateThumb(path, n) => {
                let cur = self.ratings.get(&path).copied().unwrap_or(0);
                let r = if cur == n { 0 } else { n };
                if r == 0 {
                    self.ratings.remove(&path);
                } else {
                    self.ratings.insert(path.clone(), r);
                }
                save_ratings(&self.ratings);
                if let Some(i) = self.images.iter().position(|p| *p == path) {
                    self.thumbs.send(i, ThumbMsg::SetRating(r));
                }
                // keep the editor's star widget in sync if this is the active image
                if self.session.active_path.as_deref() == Some(path.as_path()) {
                    self.editor_stars.emit(stars::StarsMsg::External(r));
                }
            }
```

(`self.editor_stars` is added in Task 9. If implementing Task 8 before Task 9, omit the last
`if` block and add it in Task 9.)

- [ ] **Step 5: Add minimal CSS for the star overlay (optional but recommended)**

If the app loads a CSS provider (search `CssProvider` in main.rs), append rules; otherwise skip.
Acceptable rule:

```css
.stars button { padding: 0 1px; min-width: 0; min-height: 0; }
.osd.stars { border-radius: 6px; padding: 1px 3px; margin: 4px; }
```

`// ponytail: skip a CSS file if none exists; the default flat buttons are legible.`

- [ ] **Step 6: Build + run, verify manually**

Run: `cargo run -p rapidraw-relm4 2>&1 | tail -20`
Expected: each thumbnail shows a star strip over the image; clicking a star sets the rating;
re-clicking the same star clears it; the rating persists after restart; sort-by-rating reflects
changes.

- [ ] **Step 7: Commit**

```bash
git add rapidraw-relm4/src/thumb.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): clickable star ratings on thumbnails"
```

---

## Task 9: Stars in the editor

**Files:**
- Modify: `rapidraw-relm4/src/main.rs`

- [ ] **Step 1: Instantiate an editor Stars component**

In `init`, before the `AppModel { ... }` literal:

```rust
        let editor_stars = Stars::builder()
            .launch(0)
            .forward(sender.input_sender(), |out| match out {
                stars::StarsOut::Changed(n) => AppMsg::RateActive(n),
            });
```

Add `use stars::{Stars, StarsMsg, StarsOut};` (or reference via `stars::`).
Add field to `AppModel`: `editor_stars: Controller<Stars>,` and `editor_stars,` to the literal.

NOTE: `AppMsg::RateActive` already exists and persists the active image's rating (main.rs
~2115). Re-using it means clicking editor stars goes through the proven path. But that handler
toggles only on explicit 0; `Stars` already computes toggle-off internally and emits the final
value, so `RateActive(n)` with the final value is correct — verify the existing `RateActive`
body simply stores `r` (it does: removes on 0, inserts otherwise).

- [ ] **Step 2: Place the stars widget in the editor header bar**

In the editor `HeaderBar`, add a `pack_start` (after the undo/redo linked box) that hosts the
component's widget. Because relm4 `view!` can't directly embed an already-launched controller's
widget by macro, append it in `init` after widgets exist instead. Add a named container in the
editor header bar:

```rust
                                #[name = "editor_stars_slot"]
                                pack_start = &gtk::Box {},
```

Then in `init` after widgets exist:

```rust
        widgets.editor_stars_slot.append(model.editor_stars.widget());
```

- [ ] **Step 3: Sync the stars when opening an image and on keyboard rating**

In the `AppMsg::OpenInEditor(path)` handler (main.rs ~2130), after the active path/rating is
established, push the current rating to the widget:

```rust
                let r = self.ratings.get(&path).copied().unwrap_or(0);
                self.editor_stars.emit(StarsMsg::External(r));
```

In the existing `AppMsg::RateActive(r)` handler (the keyboard 0..5 path), after persisting, also
sync the widget so the stars reflect keyboard input:

```rust
                self.editor_stars.emit(StarsMsg::External(r));
```

(Place this after `save_ratings(&self.ratings);` and the thumb send.)

- [ ] **Step 4: Add the editor-sync line in `RateThumb`** (deferred from Task 8 Step 4)

If omitted earlier, add to the `RateThumb` handler:

```rust
                if self.session.active_path.as_deref() == Some(path.as_path()) {
                    self.editor_stars.emit(StarsMsg::External(r));
                }
```

- [ ] **Step 5: Build + run, verify manually**

Run: `cargo run -p rapidraw-relm4 2>&1 | tail -20`
Expected: opening an image shows its rating in the editor header stars; clicking them updates
the rating and the matching thumbnail; pressing keys 0..5 updates both the stored rating and
the editor stars.

- [ ] **Step 6: Commit**

```bash
git add rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): star rating control in the editor header"
```

---

## Task 10: Albums section in the sidebar

**Files:**
- Modify: `rapidraw-relm4/src/sidebar.rs`
- Modify: `rapidraw-relm4/src/main.rs`

Albums are listed below the folders section. Groups render as `adw::ExpanderRow`, albums as
`adw::ActionRow` with an icon. Selecting an album loads its images into the grid. Create / rename
/ delete via a "+" button and a right-click menu, persisted through core.

- [ ] **Step 1: Extend sidebar messages and state**

In `sidebar.rs`, extend the enums and struct:

```rust
use rapidraw_core::albums::AlbumItem;
```

Add to `SidebarOut`:

```rust
    SelectAlbum { id: String, name: String, images: Vec<String> },
    NewAlbum(String),
    RenameAlbum { id: String, name: String },
    DeleteAlbum(String),
```

Add to `SidebarIn`:

```rust
    SetAlbums(Vec<AlbumItem>),
    ActivateAlbum(String),
```

Add fields to `Sidebar`:

```rust
    albums: Vec<AlbumItem>,
    albums_box: gtk::Box,
```

Initialize them in `init` (an empty `Vec` and a new `gtk::Box`), and add a second
`ScrolledWindow`/`Box` + an "ALBUMS" header + a "+" button to the `view!` (mirroring the folders
section). The "+" button opens a name dialog (Step 3) and emits `SidebarOut::NewAlbum(name)`.

- [ ] **Step 2: Render albums on `SetAlbums`**

Handle `SidebarIn::SetAlbums(items)` by storing `self.albums = items;` then calling a
`rebuild_albums()` that clears `albums_box` and appends a row per item:

```rust
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
            let s = sender.clone();
            let id = id.clone();
            row.connect_activated(move |_| s.input(SidebarIn::ActivateAlbum(id.clone())));
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
```

Handle `SidebarIn::ActivateAlbum(id)` by looking up the album via
`rapidraw_core::albums::album_images(&self.albums, &id)` and the name by a small local search,
then emitting `SidebarOut::SelectAlbum { id, name, images }`.

- [ ] **Step 3: New / rename / delete dialogs (libadwaita)**

Add a helper that shows an `adw::AlertDialog` with an entry and emits the given output on OK.
Trigger "New album" from the "+" button. For rename/delete, attach a right-click
`gtk::GestureClick` to each album row that pops a `gtk::PopoverMenu` with "Rename" / "Delete";
"Rename" reuses the name dialog. Example name dialog:

```rust
fn ask_name(parent: &gtk::Widget, title: &str, initial: &str, on_ok: impl Fn(String) + 'static) {
    let dialog = adw::AlertDialog::new(Some(title), None);
    let entry = gtk::Entry::builder().text(initial).build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("ok", "OK");
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    let entry2 = entry.clone();
    dialog.connect_response(None, move |_, resp| {
        if resp == "ok" {
            let name = entry2.text().to_string();
            if !name.trim().is_empty() {
                on_ok(name);
            }
        }
    });
    dialog.present(Some(parent));
}
```

Wire the "+" button to `ask_name(self_root_widget, "New album", "", move |n| sender.output(SidebarOut::NewAlbum(n)))`.
For the row context menu, emit `SidebarOut::RenameAlbum{id,name}` / `SidebarOut::DeleteAlbum(id)`.

`// ponytail: only flat (top-level) album create from the UI; nested-group creation reuses the
same albums.json format but is not exposed yet — add a "New group" action when needed.`

- [ ] **Step 4: App-side album state + persistence in `main.rs`**

Add a config-path helper next to `ratings_file()`:

```rust
fn albums_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rapidraw-relm4").join("albums.json"))
}
```

Add to `AppModel`: `albums: Vec<rapidraw_core::albums::AlbumItem>,`.
In `init`, load them: `let albums = albums_file().map(|p| rapidraw_core::albums::load_albums(&p)).unwrap_or_default();`
and add `albums,` to the literal. After building widgets, push them to the sidebar:
`model.sidebar.emit(SidebarIn::SetAlbums(model.albums.clone()));`

Extend the sidebar `.forward(...)` mapping in `init` to cover the new outputs:

```rust
                SidebarOut::SelectAlbum { id, name, images } => AppMsg::ShowAlbum { id, name, images },
                SidebarOut::NewAlbum(name) => AppMsg::AlbumNew(name),
                SidebarOut::RenameAlbum { id, name } => AppMsg::AlbumRename { id, name },
                SidebarOut::DeleteAlbum(id) => AppMsg::AlbumDelete(id),
```

Add the `AppMsg` variants:

```rust
    ShowAlbum { id: String, name: String, images: Vec<String> },
    AlbumNew(String),
    AlbumRename { id: String, name: String },
    AlbumDelete(String),
```

- [ ] **Step 5: Handle album messages**

```rust
            AppMsg::ShowAlbum { images, .. } => {
                // Populate the grid from the album's existing image paths.
                let paths: Vec<PathBuf> = images
                    .into_iter()
                    .map(PathBuf::from)
                    .filter(|p| p.exists())
                    .collect();
                self.all_images = paths;
                self.apply_library(&sender); // existing method; arranges + rebuilds thumbs
            }
            AppMsg::AlbumNew(name) => {
                let id = format!("album-{}", std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
                self.albums.push(rapidraw_core::albums::AlbumItem::Album {
                    id, name, icon: None, images: vec![],
                });
                self.persist_albums();
            }
            AppMsg::AlbumRename { id, name } => {
                rename_album(&mut self.albums, &id, &name);
                self.persist_albums();
            }
            AppMsg::AlbumDelete(id) => {
                delete_album(&mut self.albums, &id);
                self.persist_albums();
            }
```

Add `AppModel` methods + free fns:

```rust
impl AppModel {
    fn persist_albums(&mut self) {
        if let Some(p) = albums_file() {
            let _ = rapidraw_core::albums::save_albums(&p, &mut self.albums);
        }
        self.sidebar.emit(SidebarIn::SetAlbums(self.albums.clone()));
    }
}

fn rename_album(tree: &mut [rapidraw_core::albums::AlbumItem], target: &str, new_name: &str) {
    use rapidraw_core::albums::AlbumItem::*;
    for item in tree.iter_mut() {
        match item {
            Album { id, name, .. } if id == target => { *name = new_name.to_string(); return; }
            Group { children, .. } => rename_album(children, target, new_name),
            _ => {}
        }
    }
}

fn delete_album(tree: &mut Vec<rapidraw_core::albums::AlbumItem>, target: &str) {
    use rapidraw_core::albums::AlbumItem::*;
    tree.retain(|i| !matches!(i, Album { id, .. } if id == target));
    for item in tree.iter_mut() {
        if let Group { children, .. } = item {
            delete_album(children, target);
        }
    }
}
```

- [ ] **Step 6: Build + run, verify manually**

Run: `cargo run -p rapidraw-relm4 2>&1 | tail -20`
Expected: the ALBUMS section lists existing albums (compatible with an `albums.json` produced by
Tauri if copied in); "+" creates a named album; right-click renames/deletes; clicking an album
loads its images into the grid; changes persist across restart.

- [ ] **Step 7: Commit**

```bash
git add rapidraw-relm4/src/sidebar.rs rapidraw-relm4/src/main.rs
git commit -m "feat(relm4): albums section in sidebar (list, select, CRUD, persist)"
```

---

## Task 11: Final verification

- [ ] **Step 1: Full workspace build + core tests**

Run: `cargo build --workspace 2>&1 | tail -20 && cargo test -p rapidraw-core 2>&1 | tail -20`
Expected: workspace builds; core tests pass.

- [ ] **Step 2: Manual acceptance pass**

Verify against the spec acceptance list:
- Sidebar visible on BOTH library and editor screens; toggle hides/restores on both.
- Folders: recursive lazy expand, image counts, search filter, navigate into subfolders.
- Albums: list, select-to-grid, create/rename/delete, persisted.
- Ratings: clickable on thumbnails AND editor; keys 0..5 work; ratings visible on thumbs and
  persisted across restart; sort-by-rating reflects edits.

- [ ] **Step 3: Final commit (if any tidy-ups)**

```bash
git add -A
git commit -m "chore(relm4): tidy-ups after sidebar + ratings feature"
```

---

## Notes for the implementer

- Field names are confirmed: `self.all_images` = scanned set, `self.images` = arranged set,
  `self.images_shared` = `Rc<RefCell<Vec<PathBuf>>>` shared copy. The single grid-rebuild method
  is the existing `AppModel::apply_library(&mut self, &ComponentSender<AppModel>)` — both folder
  and album selection call it; never duplicate the thumbs-rebuild loop.
- relm4 0.9 `Component::update(&mut self, msg, sender, _root)`. adw = libadwaita 0.7 (`v1_4`), so
  `OverlaySplitView` + `AlertDialog` are available — no `Flap` fallback needed.
- `apply_library` runs the active filter/sort/search over `all_images`. For albums this means an
  album view still honors the current sort/search, which is acceptable; if an album should ignore
  filters, that is a future enhancement, not part of this plan.
```
