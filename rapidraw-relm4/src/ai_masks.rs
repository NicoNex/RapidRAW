//! AI mask generation glue: runs the ONNX models in `rapidraw_core::ai` on a
//! worker thread and returns a base64 PNG mask to store in the sub-mask's
//! parameters. The render path's `ai_sub_mask_resolver` decodes it.
//!
//! Inference runs on the geometry-applied (render-space) image, so the stored
//! mask aligns 1:1 with the render and the transform fields are identity — the
//! resolver only needs to scale, not re-orient.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use image::DynamicImage;
use rapidraw_core::ai::{self, AiState};
use serde_json::Value;
use tokio::sync::Mutex as TokioMutex;

/// AI mask flavours. Foreground/Sky are one-click; Subject uses a box prompt
/// (full image if unset); Depth produces a depth map the resolver thresholds.
pub enum Kind {
    Foreground,
    Sky,
    Subject { start: (f64, f64), end: (f64, f64) },
    Depth,
}

impl Kind {
    /// Map a sub-mask type + its parameters to a [`Kind`], or `None` for
    /// non-AI types.
    pub fn from_sub(mask_type: &str, params: &Value, w: f64, h: f64) -> Option<Self> {
        match mask_type {
            "ai-foreground" => Some(Kind::Foreground),
            "ai-sky" => Some(Kind::Sky),
            "ai-subject" | "quick-eraser" => {
                let g = |k: &str| params.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                let (start, end) = ((g("startX"), g("startY")), (g("endX"), g("endY")));
                // No box drawn yet -> prompt with the whole frame (segments the
                // dominant subject). Canvas box-drag refinement is a follow-up.
                let (start, end) = if start == end { ((0.0, 0.0), (w, h)) } else { (start, end) };
                Some(Kind::Subject { start, end })
            }
            "ai-depth" => Some(Kind::Depth),
            _ => None,
        }
    }
}

fn state() -> &'static (Mutex<Option<AiState>>, TokioMutex<()>) {
    static S: OnceLock<(Mutex<Option<AiState>>, TokioMutex<()>)> = OnceLock::new();
    S.get_or_init(|| (Mutex::new(None), TokioMutex::new(())))
}

fn models_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("rapidraw-relm4").join("models");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Run the model for `kind` on `image` (geometry-applied, render-space) and
/// return a base64 PNG of the full-res mask. Blocks; call on a worker thread.
pub fn generate(kind: Kind, image: &DynamicImage) -> Result<String, String> {
    let (st, lock) = state();
    // First run downloads the models (hundreds of MB). Log each step so the
    // terminal shows progress; the UI shows a persistent "Generating…" status.
    let progress = |e: ai::DownloadEvent| match e {
        ai::DownloadEvent::Start(name) => log::info!("AI model download: {name}…"),
        ai::DownloadEvent::Finish(name) => log::info!("AI model ready: {name}"),
        ai::DownloadEvent::Progress(msg) => log::info!("AI: {msg}"),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let models = rt
        .block_on(ai::get_or_init_ai_models(&models_dir(), &progress, st, lock))
        .map_err(|e| e.to_string())?;

    let gray = match kind {
        Kind::Foreground => ai::run_u2netp_model(image, &models.u2netp),
        Kind::Sky => ai::run_sky_seg_model(image, &models.sky_seg),
        Kind::Subject { start, end } => ai::generate_image_embeddings(image, &models.sam_encoder)
            .and_then(|emb| ai::run_sam_decoder(&models.sam_decoder, &emb, start, end)),
        Kind::Depth => ai::run_depth_anything_model(image, &models.depth_anything),
    }
    .map_err(|e| e.to_string())?;
    ai::encode_mask_png_base64(&gray).map_err(|e| e.to_string())
}
