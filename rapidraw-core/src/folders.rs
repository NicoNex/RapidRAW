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
        assert!(nodes[0].has_subdirs);
        assert_eq!(nodes[0].image_count, 1);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unreadable_dir_is_empty() {
        assert!(list_subdirs(Path::new("/no/such/dir")).is_empty());
    }
}
