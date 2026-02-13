fn main() {
    // Download oar-ocr models into src-tauri/models/ if they are not already present.
    // Runs before tauri_build so the resource glob in tauri.conf.json resolves correctly.
    download_models();

    tauri_build::build();
}

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
