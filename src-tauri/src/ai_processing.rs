//! Thin Tauri adapter over `rapidraw_core::ai`.
//!
//! The ONNX inference and model management now live in `rapidraw-core` (behind
//! its `ai` feature) so both the Tauri app and the relm4/GTK frontend share one
//! implementation. This module supplies the Tauri-specific seams that core
//! deliberately does not depend on:
//!
//! - the models directory, derived from the `AppHandle`'s app-data dir;
//! - progress events, mapped onto Tauri's event channel;
//! - the ort exit handler, registered after a successful init.
//!
//! Call sites elsewhere in `src-tauri` keep using `crate::ai_processing::*`
//! unchanged — the wrappers below preserve the old `AppHandle` signatures.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use image::{DynamicImage, Rgb32FImage};
use ort::session::Session;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex as TokioMutex;

use rapidraw_core::ai::DownloadEvent;

// Types and pure (UI-agnostic) inference fns are re-exported verbatim.
pub use rapidraw_core::ai::{
    AiDepthMaskParameters, AiForegroundMaskParameters, AiModels, AiSkyMaskParameters, AiState,
    AiSubjectMaskParameters, CachedDepthMap, ClipModels, generate_image_embeddings,
    run_depth_anything_model, run_lama_inpainting, run_sam_decoder, run_sky_seg_model,
    run_u2netp_model,
};

/// Models live under the app-data dir; created on first use.
fn models_dir(app: &AppHandle) -> Result<PathBuf> {
    let dir = app.path().app_data_dir()?.join("models");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

/// Map core's download/progress events onto Tauri's event channel (same event
/// names the frontend already listens for).
fn progress_for(app: &AppHandle) -> impl Fn(DownloadEvent) + Send + Sync + '_ {
    move |e| {
        let _ = match e {
            DownloadEvent::Start(n) => app.emit("ai-model-download-start", n),
            DownloadEvent::Finish(n) => app.emit("ai-model-download-finish", n),
            DownloadEvent::Progress(m) => app.emit("denoise-progress", m),
        };
    }
}

pub async fn get_or_init_ai_models(
    app_handle: &AppHandle,
    ai_state_mutex: &Mutex<Option<AiState>>,
    ai_init_lock: &TokioMutex<()>,
) -> Result<Arc<AiModels>> {
    let dir = models_dir(app_handle)?;
    let cb = progress_for(app_handle);
    let r = rapidraw_core::ai::get_or_init_ai_models(&dir, &cb, ai_state_mutex, ai_init_lock).await;
    if r.is_ok() {
        crate::register_exit_handler();
    }
    r
}

pub async fn get_or_init_denoise_model(
    app_handle: &AppHandle,
    ai_state_mutex: &Mutex<Option<AiState>>,
    ai_init_lock: &TokioMutex<()>,
) -> Result<Arc<Mutex<Session>>> {
    let dir = models_dir(app_handle)?;
    let cb = progress_for(app_handle);
    let r =
        rapidraw_core::ai::get_or_init_denoise_model(&dir, &cb, ai_state_mutex, ai_init_lock).await;
    if r.is_ok() {
        crate::register_exit_handler();
    }
    r
}

pub async fn get_or_init_clip_models(
    app_handle: &AppHandle,
    ai_state_mutex: &Mutex<Option<AiState>>,
    ai_init_lock: &TokioMutex<()>,
) -> Result<Arc<ClipModels>> {
    let dir = models_dir(app_handle)?;
    let cb = progress_for(app_handle);
    let r =
        rapidraw_core::ai::get_or_init_clip_models(&dir, &cb, ai_state_mutex, ai_init_lock).await;
    if r.is_ok() {
        crate::register_exit_handler();
    }
    r
}

pub async fn get_or_init_lama_model(
    app_handle: &AppHandle,
    ai_state_mutex: &Mutex<Option<AiState>>,
    ai_init_lock: &TokioMutex<()>,
) -> Result<Arc<Mutex<Session>>> {
    let dir = models_dir(app_handle)?;
    let cb = progress_for(app_handle);
    let r = rapidraw_core::ai::get_or_init_lama_model(&dir, &cb, ai_state_mutex, ai_init_lock).await;
    if r.is_ok() {
        crate::register_exit_handler();
    }
    r
}

pub fn run_ai_denoise(
    rgb_img: &Rgb32FImage,
    intensity: f32,
    session: &Mutex<Session>,
    app_handle: &AppHandle,
) -> Result<DynamicImage> {
    let cb = progress_for(app_handle);
    rapidraw_core::ai::run_ai_denoise(rgb_img, intensity, session, &cb)
}
