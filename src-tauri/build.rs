fn main() {
    // Download oar-ocr models into src-tauri/models/ if they are not already present.
    // Runs before tauri_build so the resource glob in tauri.conf.json resolves correctly.
    download_models();

    // Download Tesseract language data into src-tauri/tessdata/ so the bundled app
    // can find tessdata at runtime without relying on a system installation.
    download_tessdata();

    // Collect the native shared libraries (FFmpeg, Tesseract, Leptonica) that the
    // binary links against into src-tauri/libs/{platform}/ so tauri.conf.json can
    // bundle them as resources in the installer / AppImage.
    collect_libs();

    tauri_build::build();
}

// ── Model download ────────────────────────────────────────────────────────────

const MODEL_BASE: &str =
    "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0";

const MODELS: &[&str] = &[
    "pp-ocrv5_mobile_det.onnx",
    "pp-ocrv5_mobile_rec.onnx",
    "ppocrv5_dict.txt",
];

fn download_models() {
    // Use CARGO_MANIFEST_DIR (= src-tauri/) so the path is correct regardless
    // of where `cargo build` is invoked from.
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let models_dir = std::path::Path::new(&manifest).join("models");
    std::fs::create_dir_all(&models_dir).expect("could not create models/");

    for filename in MODELS {
        let dest = models_dir.join(filename);

        // Ask Cargo to re-run this script if a file is removed.
        println!("cargo:rerun-if-changed=models/{filename}");

        if dest.exists() {
            continue;
        }

        let url = format!("{MODEL_BASE}/{filename}");
        println!("cargo:warning=oar-ocr: downloading {filename}…");

        let resp = ureq::get(&url)
            .call()
            .unwrap_or_else(|e| panic!("failed to download {filename}: {e}"));

        let mut file =
            std::fs::File::create(&dest).expect("could not create model file");

        std::io::copy(&mut resp.into_reader(), &mut file)
            .unwrap_or_else(|e| panic!("failed to write {filename}: {e}"));

        println!("cargo:warning=oar-ocr: {filename} ready");
    }
}

// ── Tesseract language data download ──────────────────────────────────────────
//
// tessdata_fast files are small (~3–6 MB each) pre-trained models.  They are
// downloaded into src-tauri/tessdata/ at build time and bundled as Tauri
// resources so the app can call Tesseract::new(Some(tessdata_parent), …) at
// runtime without requiring a system Tesseract installation.

const TESSDATA_BASE: &str =
    "https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/main";

/// Languages supported by `build_lang()` in ocr/tesseract.rs.
const TESSDATA_LANGS: &[&str] = &["eng", "deu", "fra", "spa"];

fn download_tessdata() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let dir = std::path::Path::new(&manifest).join("tessdata");
    std::fs::create_dir_all(&dir).expect("could not create tessdata/");

    for lang in TESSDATA_LANGS {
        let filename = format!("{lang}.traineddata");
        let dest = dir.join(&filename);

        println!("cargo:rerun-if-changed=tessdata/{filename}");

        if dest.exists() {
            continue;
        }

        let url = format!("{TESSDATA_BASE}/{filename}");
        println!("cargo:warning=tessdata: downloading {filename}…");

        let resp = ureq::get(&url)
            .call()
            .unwrap_or_else(|e| panic!("failed to download {filename}: {e}"));

        let mut file =
            std::fs::File::create(&dest).expect("could not create tessdata file");

        std::io::copy(&mut resp.into_reader(), &mut file)
            .unwrap_or_else(|e| panic!("failed to write {filename}: {e}"));

        println!("cargo:warning=tessdata: {filename} ready");
    }
}

// ── Windows DLL collection ────────────────────────────────────────────────────
//
// On Windows, DLLs must sit next to the .exe for Windows' loader to find them.
// Tauri resources land in a resources\ subdirectory, so a custom NSIS macro
// (nsis-extra.nsi) copies them to $INSTDIR at install time.
//
// This function collects the DLLs from known locations into
// src-tauri/libs/windows/ which tauri.conf.json then bundles as resources.
//
// On Linux nothing needs collecting: Tauri's AppImage builder runs linuxdeploy
// which automatically detects and bundles all non-system .so dependencies by
// analysing the binary's ldd output.  The RPATH embedded via .cargo/config.toml
// covers running the unwrapped binary directly (dev / non-AppImage use).
fn collect_libs() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-env-changed=SKIP_LIB_COLLECT");

    if std::env::var("SKIP_LIB_COLLECT").is_ok() {
        return;
    }

    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let libs_root = std::path::Path::new(&manifest).join("libs");

    // Always create libs/windows/ with a .gitkeep so the tauri.conf.json
    // resource glob ("libs/windows/*") never errors on an absent directory.
    let win_dir = libs_root.join("windows");
    std::fs::create_dir_all(&win_dir).ok();
    let keep = win_dir.join(".gitkeep");
    if !keep.exists() {
        std::fs::write(&keep, b"").ok();
    }

    #[cfg(target_os = "windows")]
    collect_libs_windows(&win_dir);
}

/// DLL name prefixes we want to bundle (matched against filenames in the search dirs).
#[cfg(target_os = "windows")]
const WINDOWS_DLL_PREFIXES: &[&str] = &[
    "avutil",
    "avformat",
    "avcodec",
    "avfilter",
    "avdevice",
    "swscale",
    "swresample",
    "tesseract",
    "leptonica",
];

#[cfg(target_os = "windows")]
fn collect_libs_windows(dest: &std::path::Path) {
    // Candidate directories to search for DLLs, in priority order:
    //   1. FFMPEG_DIR\bin   (set by ffmpeg-sys-next when FFMPEG_DIR is configured)
    //   2. VCPKG_ROOT\installed\x64-windows\bin   (vcpkg install)
    //   3. Entries on PATH that contain matching DLLs (last resort)
    let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();

    if let Ok(ffmpeg_dir) = std::env::var("FFMPEG_DIR") {
        search_dirs.push(std::path::PathBuf::from(&ffmpeg_dir).join("bin"));
        search_dirs.push(std::path::PathBuf::from(&ffmpeg_dir)); // sometimes DLLs are at root
    }
    if let Ok(vcpkg_root) = std::env::var("VCPKG_ROOT") {
        search_dirs.push(
            std::path::PathBuf::from(&vcpkg_root)
                .join("installed")
                .join("x64-windows")
                .join("bin"),
        );
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            search_dirs.push(dir);
        }
    }

    let mut found: std::collections::HashSet<String> = std::collections::HashSet::new();

    for dir in &search_dirs {
        if !dir.exists() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if !name.to_ascii_lowercase().ends_with(".dll") {
                continue;
            }
            let name_lower = name.to_ascii_lowercase();
            if !WINDOWS_DLL_PREFIXES.iter().any(|p| name_lower.starts_with(p)) {
                continue;
            }
            if found.contains(name) {
                continue; // already found this DLL from a higher-priority dir
            }
            let dst = dest.join(name);
            if !dst.exists() {
                match std::fs::copy(&path, &dst) {
                    Ok(_) => println!("cargo:warning=collect_libs: bundled {}", path.display()),
                    Err(e) => println!(
                        "cargo:warning=collect_libs: could not copy {}: {e}",
                        path.display()
                    ),
                }
            }
            found.insert(name.to_string());
        }
    }

    if found.is_empty() {
        println!(
            "cargo:warning=collect_libs: no FFmpeg/Tesseract DLLs found. \
             Set FFMPEG_DIR or VCPKG_ROOT, or add the DLL directory to PATH."
        );
    }
}
