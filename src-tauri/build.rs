fn main() {
    // Download oar-ocr models into src-tauri/models/ if they are not already present.
    // Runs before tauri_build so the resource glob in tauri.conf.json resolves correctly.
    download_models();

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

// ── Native library collection ─────────────────────────────────────────────────
//
// Copies the shared libraries that the binary dynamically links against
// (FFmpeg libav*/libsw*, Tesseract, Leptonica) into
//   src-tauri/libs/linux/   — .so files for Linux builds
//   src-tauri/libs/windows/ — .dll files for Windows builds
//
// tauri.conf.json then bundles these as resources so:
//   • AppImage / deb:  libraries land in the app resource dir; the binary's
//                      RPATH ($ORIGIN/../resources) finds them at runtime.
//   • Windows NSIS:    a custom macro (nsis-extra.nsi) copies them from
//                      resources\ to the install root where Windows DLL search
//                      finds them (must be next to the .exe).
//
// Trigger a rebuild when the environment hints change.
fn collect_libs() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-env-changed=SKIP_LIB_COLLECT");

    if std::env::var("SKIP_LIB_COLLECT").is_ok() {
        return;
    }

    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let libs_root = std::path::Path::new(&manifest).join("libs");

    // Ensure both platform dirs exist with at least a .gitkeep so the
    // tauri.conf.json resource globs ("libs/linux/*", "libs/windows/*") never
    // fail on an absent or empty directory during a cross-platform build.
    for sub in &["linux", "windows"] {
        let dir = libs_root.join(sub);
        std::fs::create_dir_all(&dir).ok();
        let keep = dir.join(".gitkeep");
        if !keep.exists() {
            std::fs::write(&keep, b"").ok();
        }
    }

    // Collect the actual libraries for the current platform.
    #[cfg(target_os = "linux")]
    collect_libs_linux(&libs_root.join("linux"));

    #[cfg(target_os = "windows")]
    collect_libs_windows(&libs_root.join("windows"));
}

/// Library name prefixes we want to bundle (matched against ldconfig output).
const LINUX_LIB_PREFIXES: &[&str] = &[
    "libavutil",
    "libavformat",
    "libavcodec",
    "libavfilter",
    "libavdevice",
    "libswscale",
    "libswresample",
    "libtesseract",
    "libleptonica",
];

/// DLL name prefixes we want to bundle on Windows.
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

#[cfg(target_os = "linux")]
fn collect_libs_linux(dest: &std::path::Path) {
    // `ldconfig -p` lists every cached shared library with its resolved path, e.g.:
    //   libavutil.so.59 (libc6,x86-64) => /lib/x86_64-linux-gnu/libavutil.so.59
    let output = match std::process::Command::new("ldconfig").arg("-p").output() {
        Ok(o) => o,
        Err(e) => {
            println!("cargo:warning=collect_libs: ldconfig not available ({e}) — skipping");
            return;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        // Only handle lines that start with one of our target prefixes.
        if !LINUX_LIB_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            continue;
        }
        // Extract the resolved path after "=>".
        let Some(arrow) = trimmed.find("=>") else { continue };
        let src_str = trimmed[arrow + 2..].trim();
        let src = std::path::Path::new(src_str);
        if !src.exists() {
            continue;
        }
        let filename = match src.file_name() {
            Some(n) => n,
            None => continue,
        };
        let dst = dest.join(filename);
        if dst.exists() {
            continue; // already collected this run or a previous run
        }
        match std::fs::copy(src, &dst) {
            Ok(_) => println!("cargo:warning=collect_libs: bundled {src_str}"),
            Err(e) => println!("cargo:warning=collect_libs: could not copy {src_str}: {e}"),
        }
    }
}

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
