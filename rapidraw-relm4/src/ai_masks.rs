//! AI mask generation glue: runs the ONNX models in `rapidraw_core::ai` on a
//! worker thread and returns a base64 PNG mask to store in the sub-mask's
//! parameters. The render path's `ai_sub_mask_resolver` decodes it.
//!
//! Inference runs on the geometry-applied (render-space) image, so the stored
//! mask aligns 1:1 with the render and the transform fields are identity — the
//! resolver only needs to scale, not re-orient.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use image::{DynamicImage, GenericImageView, GrayImage, Rgba, RgbaImage};
use rapidraw_core::ai::{self, AiState};
use rapidraw_core::mask_generation::{PatchData, encode_patch_data};
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

/// Where the inpaint result comes from.
pub enum InpaintBackend {
    /// Local LaMa erase — removes content under the mask, no prompt. Downloads
    /// the LaMa model on first use.
    FastErase,
    /// External generative server (the local AI-connector middleware).
    /// `source_path` keys its upload cache; `prompt` guides the fill.
    Connector {
        base_url: String,
        source_path: String,
        prompt: String,
    },
}

/// Generate an inpaint patch: run `backend` over `source_image` (other patches
/// already composited, geometry-applied) within `mask_bitmap` (render-space),
/// returning the encoded [`PatchData`]. Blocks; call on a worker thread.
///
/// Mirrors the tail of the Tauri `invoke_generative_replace_with_mask_def`
/// command, minus the warp/unwarp dance (relm4 masks are already render-space).
pub fn run_inpaint(
    source_image: &DynamicImage,
    mask_bitmap: &GrayImage,
    backend: InpaintBackend,
) -> Result<PatchData, String> {
    let (st, lock) = state();
    let progress = |e: ai::DownloadEvent| match e {
        ai::DownloadEvent::Start(name) => log::info!("AI model download: {name}…"),
        ai::DownloadEvent::Finish(name) => log::info!("AI model ready: {name}"),
        ai::DownloadEvent::Progress(msg) => log::info!("AI: {msg}"),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let patch_rgba = match backend {
        InpaintBackend::FastErase => {
            let lama = rt
                .block_on(ai::get_or_init_lama_model(&models_dir(), &progress, st, lock))
                .map_err(|e| e.to_string())?;
            ai::run_lama_inpainting(source_image, mask_bitmap, &lama).map_err(|e| e.to_string())?
        }
        InpaintBackend::Connector {
            base_url,
            source_path,
            prompt,
        } => {
            // Server expects an RGBA mask the size of the source (white = fill).
            let (w, h) = source_image.dimensions();
            let mut rgba = RgbaImage::new(w, h);
            for (x, y, p) in mask_bitmap.enumerate_pixels() {
                let i = p[0];
                rgba.put_pixel(x, y, Rgba([i, i, i, 255]));
            }
            let mask_dyn = DynamicImage::ImageRgba8(rgba);
            rt.block_on(rapidraw_core::ai_connector::process_inpainting(
                &base_url,
                &source_path,
                source_image,
                &mask_dyn,
                prompt,
                None,
            ))
            .map_err(|e| e.to_string())?
        }
    };

    encode_patch_data(mask_bitmap, &patch_rgba).map_err(|e| e.to_string())
}
