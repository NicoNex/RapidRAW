//! Fetch the ONNX Runtime dynamic library for the host target so `ort`
//! (load-dynamic) can `dlopen` it at runtime. The Tauri app does the same in its
//! own `build.rs` + bundles it; here we drop it in `resources/` and bake its
//! absolute path into the binary (`RAPIDRAW_ORT_DYLIB`), which `main` promotes to
//! `ORT_DYLIB_PATH` at startup. Pinned to the same v1.22.0 build the Tauri app uses.
//!
//! A failed download (e.g. offline) is a warning, not an error: the app still
//! builds and runs; only AI features are unavailable until the lib is present.

use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

fn sha256_ok(path: &Path, expected: &str) -> io::Result<bool> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()) == expected)
}

fn download(url: &str, dest: &Path, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut resp = reqwest::blocking::get(url)?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status()).into());
    }
    let tmp = dest.with_extension("part");
    let mut f = fs::File::create(&tmp)?;
    resp.copy_to(&mut f)?;
    drop(f);
    if sha256_ok(&tmp, expected)? {
        fs::rename(&tmp, dest)?;
        Ok(())
    } else {
        let _ = fs::remove_file(&tmp);
        Err("sha256 mismatch (corrupt download)".into())
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Bundle the icons used by the UI (relm4-icons 0.11 lists them here, not in a
    // TOML). Generates icon_names.rs in OUT_DIR, included by `mod icon_names`.
    relm4_icons_build::bundle_icons(
        "icon_names.rs",
        Some("com.rapidraw.relm4"),
        None::<&str>,
        None::<&str>,
        [
            // Right-rail tabs
            "options-regular",
            "crop-regular",
            "layer-diagonal-regular",
            "paint-brush-regular",
            "info-regular",
            // Inpaint create-grid cards
            "eraser",
            "sparkle-regular",
            "person-regular",
            "line-horizontal-4-regular",
            "circle-regular",
            // Masks create-grid cards
            "cloud-regular",
            "more-horizontal-regular",
            // General UI
            "add-regular",
            "delete-regular",
            "arrow-counterclockwise-regular",
            "eye-regular",
            "eye-off-regular",
        ],
    );

    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    let (file, lib, hash) = match (os.as_str(), arch.as_str()) {
        ("macos", "aarch64") => (
            "libonnxruntime-macos-aarch64.dylib",
            "libonnxruntime.dylib",
            "2b885992d3d6fa4130d39ec84a80d7504ff52750027c547bb22c86165f19406a",
        ),
        ("macos", "x86_64") => (
            "libonnxruntime-macos-x86_64.dylib",
            "libonnxruntime.dylib",
            "283e595e61cf65df7a6b1d59a1616cbd35c8b6399dd90d799d99b71a3ff83160",
        ),
        ("linux", "x86_64") => (
            "libonnxruntime-linux-x86_64.so",
            "libonnxruntime.so",
            "3da6146e14e7b8aaec625dde11d6114c7457c87a5f93d744897da8781e35c673",
        ),
        ("linux", "aarch64") => (
            "libonnxruntime-linux-aarch64.so",
            "libonnxruntime.so",
            "0afd69a0ae38c5099fd0e8604dda398ac43dee67cd9c6394b5142b19e82528de",
        ),
        ("windows", "x86_64") => (
            "onnxruntime-windows-x86_64.dll",
            "onnxruntime.dll",
            "579b636403983254346a5c1d80bd28f1519cd1e284cd204f8d4ff41f8d711559",
        ),
        ("windows", "aarch64") => (
            "onnxruntime-windows-aarch64.dll",
            "onnxruntime.dll",
            "79281671a386ed1baab9dbdbb09fe55f99577011472e9526cf9d0b468bb6bcc7",
        ),
        _ => {
            println!("cargo:warning=No ONNX Runtime mapping for {os}-{arch}; AI features disabled.");
            return;
        }
    };

    let dir = manifest.join("resources");
    let _ = fs::create_dir_all(&dir);
    let dest = dir.join(lib);

    let have_valid = dest.exists() && sha256_ok(&dest, hash).unwrap_or(false);
    if !have_valid {
        let url = format!(
            "https://huggingface.co/CyberTimon/RapidRAW-Models/resolve/main/onnxruntimes-v1.22.0/{file}?download=true"
        );
        println!("cargo:warning=Downloading ONNX Runtime ({os}-{arch})…");
        if let Err(e) = download(&url, &dest, hash) {
            println!("cargo:warning=ONNX Runtime download failed: {e}. AI features will be unavailable until it is present at {}", dest.display());
            return;
        }
    }

    // Bake the absolute path in; `main` sets ORT_DYLIB_PATH from it at startup.
    println!("cargo:rustc-env=RAPIDRAW_ORT_DYLIB={}", dest.display());
}
