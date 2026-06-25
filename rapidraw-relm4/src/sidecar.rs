//! Per-image edit persistence ("sidecar"), so reopening a photo restores its
//! edits — matching the original UI's non-destructive flow.
//!
//! Stored as JSON in the config dir, keyed by a hash of the image's absolute
//! path (so the image folder isn't polluted). The adjustment struct is `Pod`,
//! so its raw bytes are persisted directly; slider UI values are stored too, so
//! the panel can be restored without per-field getters.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use rapidraw_core::mask_generation::{AiPatchDefinition, MaskDefinition};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
pub struct Edits {
    /// Raw bytes of `GlobalAdjustments` (Pod).
    pub global: Vec<u8>,
    /// Slider UI values (panel snapshot), for restoring the controls.
    pub vals: Vec<f64>,
    pub orientation_steps: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    pub straighten: f32,
    pub crop: Option<[f32; 4]>,
    /// Path of the active .cube LUT, if any.
    pub lut: Option<String>,
    /// Masks (containers + sub-masks), camelCase JSON (same contract as the
    /// Tauri sidecar). Defaulted so old sidecars without it still load.
    #[serde(default)]
    pub masks: Vec<MaskDefinition>,
    /// AI inpaint patches, camelCase JSON (same contract as the Tauri sidecar).
    /// Defaulted so old sidecars without it still load.
    #[serde(default)]
    pub ai_patches: Vec<AiPatchDefinition>,
}

fn edits_path(image: &Path) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let abs = std::fs::canonicalize(image).unwrap_or_else(|_| image.to_path_buf());
    let mut h = DefaultHasher::new();
    abs.hash(&mut h);
    Some(
        base.join("rapidraw-relm4")
            .join("edits")
            .join(format!("{:016x}.json", h.finish())),
    )
}

pub fn save(image: &Path, e: &Edits) {
    let Some(p) = edits_path(image) else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(e) {
        let _ = std::fs::write(p, json);
    }
}

pub fn load(image: &Path) -> Option<Edits> {
    let p = edits_path(image)?;
    let bytes = std::fs::read(p).ok()?;
    serde_json::from_slice(&bytes).ok()
}
