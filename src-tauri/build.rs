fn main() {
    // Download oar-ocr models into src-tauri/models/ if they are not already present.
    // Runs before tauri_build so the resource glob in tauri.conf.json resolves correctly.
    download_models();

    // Download Tesseract language data into src-tauri/tessdata/ so the bundled app
    // can find tessdata at runtime without relying on a system installation.
    download_tessdata();

    tauri_build::build();
}

// ── Model download ────────────────────────────────────────────────────────────

const MODEL_BASE: &str = "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0";

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

        let mut file = std::fs::File::create(&dest).expect("could not create model file");

        std::io::copy(&mut resp.into_reader(), &mut file)
            .unwrap_or_else(|e| panic!("failed to write {filename}: {e}"));

        println!("cargo:warning=oar-ocr: {filename} ready");
    }
}

// ── Tesseract language data download ──────────────────────────────────────────
//
// tessdata_fast files are small (~3–6 MB each) pre-trained models.  They are
// downloaded into src-tauri/tessdata/ at build time and bundled as Tauri
// resources so the app can find tessdata at runtime without requiring a
// system Tesseract installation.

const TESSDATA_BASE: &str = "https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/main";

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

        let mut file = std::fs::File::create(&dest).expect("could not create tessdata file");

        std::io::copy(&mut resp.into_reader(), &mut file)
            .unwrap_or_else(|e| panic!("failed to write {filename}: {e}"));

        println!("cargo:warning=tessdata: {filename} ready");
    }
}
