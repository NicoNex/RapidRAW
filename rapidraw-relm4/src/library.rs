use std::path::{Path, PathBuf};

const EXT: &[&str] = &[
    "jpg", "jpeg", "png", "tiff", "tif", "webp", "raw", "arw", "cr2", "cr3", "nef", "orf", "raf",
    "dng", "rw2", "pef", "srw", "3fr", "mef",
];

/// Scan a directory (non-recursively) for supported image/RAW files, sorted.
pub fn scan_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| EXT.contains(&x.to_lowercase().as_str()))
                .unwrap_or(false)
        })
        .collect();
    v.sort();
    v
}
