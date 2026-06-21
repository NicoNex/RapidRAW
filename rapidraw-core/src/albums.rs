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
