# relm4 sidebar (folders + albums) and star ratings — design

Date: 2026-06-21
Branch: `feat/relm4-sidebar-ratings`

## Goal

Bring the relm4/GTK frontend closer to Tauri parity by adding:

1. A **left sidebar** with **Folders** and **Albums** sections (folders first, albums
   second), present on both the library (thumbnail grid) screen and the editor screen,
   and **collapsible/restorable** via a toolbar button.
2. **Star ratings** editable from both the library grid and the editor, with ratings
   **visible on the thumbnails**.

Use libadwaita widgets where they fit. Per the parity goal, shared logic moves into
`rapidraw-core` instead of being reimplemented in the relm4 crate.

## Non-goals

- Pinned folders, folder icons, album groups drag-reorder, image-count animations, and
  the framer-motion transitions from the original React UI. (Albums *groups* are still
  supported in the data model because they share the file format, but fancy animation is
  out.)
- Changing the Tauri frontend behavior. The Tauri commands keep working identically.
- Virtual copies / tags surfaced in the sidebar.

## Current state (verified)

- `rapidraw-relm4/src/main.rs`: `adw::ApplicationWindow > adw::ToastOverlay > adw::NavigationView`
  with two `adw::NavigationPage`s tagged `"library"` and `"editor"`. Editor is pushed on
  `OpenInEditor`; back button pops it. Each page has its own `adw::ToolbarView` + `HeaderBar`.
- Ratings already exist: `AppModel.ratings: HashMap<PathBuf,u8>`, persisted to
  `ratings.json` via `load_ratings`/`save_ratings`; settable through keys `0..5`
  (`AppMsg` rating variant at main.rs ~line 296, applied ~2120). `library::arrange` already
  sorts by `RatingDesc` using the ratings map.
- `rapidraw-relm4/src/thumb.rs`: `Thumb` factory shows a **read-only** `★★★☆☆` label and
  already carries `rating: u8` + `ThumbMsg::SetRating(u8)`.
- `rapidraw-relm4/src/library.rs`: `scan_dir` (non-recursive), `arrange`, `texture_from_rgba`.
- Tauri album logic in `src-tauri/src/file_management.rs`: `AlbumItem` enum (`Album`/`Group`,
  serde `tag="type"`, camelCase), `sort_album_tree`, `get_albums`/`save_albums` (read/write
  `albums.json`), `add_to_album`, `get_album_images`. Commands registered in `lib.rs`.

## Architecture

```
adw::ApplicationWindow
└─ adw::ToastOverlay
   └─ adw::OverlaySplitView            (NEW — sidebar lives here, outside the page stack)
      ├─ sidebar:  Sidebar component   (NEW src/sidebar.rs)
      └─ content:  adw::NavigationView (unchanged: library + editor pages)
```

Because the sidebar is a sibling of the `NavigationView`, it stays visible whether the
library page or the editor page is on top. No per-page duplication.

### Module 1 — `rapidraw-core/src/albums.rs` (new) + `folders.rs` helper

Move the **pure** album logic from Tauri into core. New public API:

```rust
// albums.rs
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AlbumItem {
    Album { id: String, name: String, icon: Option<String>, images: Vec<String> },
    Group { id: String, name: String, icon: Option<String>, children: Vec<AlbumItem> },
}

pub fn sort_album_tree(items: &mut [AlbumItem]);
pub fn load_albums(path: &Path) -> Result<Vec<AlbumItem>, String>; // [] if missing, sorted
pub fn save_albums(path: &Path, tree: &mut Vec<AlbumItem>) -> Result<(), String>; // sorts then writes pretty json
pub fn add_to_album(tree: &mut [AlbumItem], album_id: &str, paths: &[String]) -> bool; // dedup append, true if found
pub fn album_images<'a>(tree: &'a [AlbumItem], album_id: &str) -> Option<&'a [String]>;
```

Tauri side: `file_management.rs` deletes its private copies and re-exports / calls
`rapidraw_core::albums::*`. The `#[tauri::command]` functions stay (they still resolve the
`app_data_dir` path via `AppHandle`) but their bodies become thin wrappers. `AlbumItem` is
re-exported from `file_management` so existing `use` paths and serde shape are unchanged.

```rust
// folders.rs (new, pure)
pub struct FolderNode { pub name: String, pub path: PathBuf, pub has_subdirs: bool, pub image_count: u32 }

/// Direct child directories of `dir`, sorted by name. `has_subdirs` lets the UI show an
/// expander without recursing now (lazy expand). `image_count` counts supported images
/// directly inside each child (cheap, non-recursive).
pub fn list_subdirs(dir: &Path) -> Vec<FolderNode>;
```

The supported-extension list currently lives in `rapidraw-relm4/src/library.rs`. Move the
`EXT`/`RAW_EXT` constants and `is_raw` into `rapidraw-core` (e.g. `formats` or a small
`media.rs`) so both `list_subdirs` (core) and the relm4 scanner use one source. relm4
`library.rs` re-exports them to avoid churn at call sites.

Tests (core, `#[test]`):
- album save→load round-trip preserves tree; `sort_album_tree` orders groups-before-albums
  then by lowercased name.
- `add_to_album` dedups and returns false for unknown id.
- `list_subdirs` on a temp dir: finds child dirs, sets `has_subdirs` correctly, counts images.

### Module 2 — `rapidraw-relm4/src/sidebar.rs` (new)

A relm4 `Component` rendering the sidebar contents. Owns no business state beyond view
model; talks to the app via `Output` messages.

```rust
pub enum SidebarOut {
    SelectFolder(PathBuf),         // load grid from a folder
    SelectAlbum { id: String, name: String, images: Vec<String> },
    AddRootFolder,                 // triggers the existing Open Folder dialog
}
pub enum SidebarIn {
    SetRoot(Option<PathBuf>),      // current folder changed → rebuild folders tree
    SetAlbums(Vec<AlbumItem>),
    ToggleFolder(PathBuf),         // lazy-expand / collapse
    Search(String),                // filter folder tree by name
    // album CRUD:
    NewAlbum(String), NewGroup(String), RenameAlbum { id: String, name: String }, DeleteAlbum(String),
}
```

Layout (libadwaita): scrolled `gtk::Box` containing
- `gtk::SearchEntry` at top.
- **Folders** section (built first): a lazy tree. Implementation: `gtk::ListView` +
  `gtk::TreeListModel` + `gtk::TreeExpander` over `FolderNode`s, root = current folder's
  subdirs (and the root folder itself as the top node). Expanding a node calls
  `core::folders::list_subdirs` for its children. Selecting a row → `SelectFolder`.
  Search filters visible names (`gtk::FilterListModel` with a `gtk::StringFilter` /
  custom filter). An "Add folder" row at the bottom emits `AddRootFolder`.
- **Albums** section (built second): an `adw::PreferencesGroup`/`gtk::ListBox` of album
  rows from `AlbumItem`. Groups render as expandable `adw::ExpanderRow`; albums as
  `adw::ActionRow` with the album icon. Row activate → `SelectAlbum`. A "+" button in the
  section header and right-click context menu (`gtk::PopoverMenu`) drive create / rename /
  delete via `adw::AlertDialog` (name entry). Persistence through core `save_albums`.

Folders ship in the first implementation pass; albums in the second. The component is
written so the albums section can be added without reworking folders.

### Module 3 — `rapidraw-relm4/src/stars.rs` (new, reused)

A small reusable clickable star widget.

```rust
pub struct Stars { pub rating: u8 }
pub enum StarsMsg { Set(u8), External(u8) } // External = programmatic sync (keys, load)
pub enum StarsOut { Changed(u8) }
```

- View: `gtk::Box` of 5 star buttons (`gtk::Button` with `★`/`☆` label, flat CSS).
  Clicking star *i* sets rating *i*; clicking the star equal to the current rating clears
  to 0 (toggle-off), matching common photo apps.
- Emits `StarsOut::Changed(u8)`.

Usage:
- **Thumbnail** (`thumb.rs`): wrap the `gtk::Picture` in a `gtk::Overlay`; place a `Stars`
  instance (or a lightweight inline version) bottom-aligned, shown via a `gtk::Revealer`
  on `motion`/hover and always shown when rating > 0. Clicking forwards through the factory
  `Output` to the app, which updates `ratings`, calls `save_ratings`, and re-sends
  `ThumbMsg::SetRating` (re-uses the existing rating code path at main.rs ~2120). The old
  read-only `stars()` label is removed.
- **Editor**: a `Stars` instance in the editor header bar (or top of the right panel)
  bound to the active image. Existing `0..5` key handler keeps working and pushes
  `StarsMsg::External` to keep the widget in sync.

### Module 4 — `main.rs` wiring

- View tree: insert `adw::OverlaySplitView` between `ToastOverlay` and `NavigationView`.
  `set_sidebar` = the `Sidebar` component's widget; `set_content` = the existing
  `NavigationView`. Set `set_min_sidebar_width`/`set_max_sidebar_width` and
  `set_collapsed`/breakpoint defaults.
- Add a sidebar-toggle `gtk::Button` (icon `sidebar-show-symbolic`,
  tooltip "Toggle sidebar") to **both** header bars (`pack_start`), each bound to flip
  `split_view.show_sidebar()`.
- `AppModel` additions: `sidebar: Controller<Sidebar>`, `albums: Vec<AlbumItem>`,
  `split_view: adw::OverlaySplitView` handle. `current_folder` already on `Session`.
- New `AppMsg` variants to bridge sidebar output: `SidebarSelectFolder(PathBuf)`
  (re-uses `FolderChosen` flow but without replacing the *root* — see below),
  `SidebarSelectAlbum{...}`, plus album CRUD forwarding.
- On `FolderChosen`: also push the folder as the sidebar root (`SidebarIn::SetRoot`).
  Selecting a sub-folder in the tree loads that dir into the grid (`scan_dir` + rebuild
  thumbs) **without** changing the tree root, mirroring the original "folder select".
- Albums path for relm4: reuse the existing config dir helper pattern
  (`ratings_file()` neighbor) → `rapidraw-relm4/albums.json`. Loaded at startup into
  `model.albums`, saved on every CRUD op.

## Data flow

- Open root folder → `FolderChosen` → `scan_dir` + `arrange` builds grid; `SetRoot`
  rebuilds folder tree.
- Click sub-folder in tree → `SidebarSelectFolder` → `scan_dir` that dir → rebuild grid.
- Click album → `SidebarSelectAlbum` → grid is populated from the album's image path list
  (paths filtered to existing files) instead of a dir scan.
- Rate from thumb or editor → update `ratings` map → `save_ratings` → refresh the thumb's
  star widget; if sort is `RatingDesc`, re-arrange.
- Album CRUD → mutate `model.albums` → `core::save_albums` → `SidebarIn::SetAlbums`.

## Error handling

- Missing/corrupt `albums.json`: `load_albums` returns `[]` (corrupt → log + empty), never
  panics. Same defensive behavior as today's Tauri `get_albums`.
- Album images that no longer exist on disk: skipped when populating the grid (logged).
- `list_subdirs` on an unreadable dir: returns empty (mirrors `scan_dir`).
- Save failures (ratings/albums): logged + `adw::Toast`; not fatal.

## Testing

- Core unit tests as listed in Module 1 (albums round-trip/sort/add, folders listing).
- relm4 UI wired and verified manually (build + run): collapse/restore sidebar on both
  screens, folder navigation, album create/select, rating from grid + editor, rating
  visible on thumbs and persisted across restart.

## Implementation order

1. Core: move `AlbumItem` + album fns and `EXT`/`is_raw` into `rapidraw-core`; add
   `folders::list_subdirs`; update Tauri to call core. Tests. (Build both crates.)
2. relm4: `OverlaySplitView` shell + toggle buttons in both header bars (empty sidebar).
3. relm4: Folders section (lazy tree, search, navigate).
4. relm4: `Stars` widget; clickable on thumbnails (overlay) + editor; persist.
5. relm4: Albums section (list, select, CRUD dialogs, persist).
