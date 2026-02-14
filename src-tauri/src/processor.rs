use crate::config::RegionConfig;
use std::collections::HashMap;
use crate::ocr::{
    oar::{build_pipeline, ColorMode, OarRecognizer},
    read_region,
    tesseract::{Preprocess, TesseractRecognizer},
    Recognizer,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{AppHandle, Emitter};

/// Shared cancellation flag — stored in Tauri managed state.
pub struct CancelFlag(pub Arc<AtomicBool>);

impl Default for CancelFlag {
    fn default() -> Self {
        CancelFlag(Arc::new(AtomicBool::new(false)))
    }
}

#[tauri::command]
pub fn cancel_extract(cancel: tauri::State<'_, CancelFlag>) {
    cancel.0.store(true, Ordering::Relaxed);
}

fn default_oar_threshold() -> f64 { 0.9 }

#[derive(Debug, Deserialize)]
pub struct ExtractParams {
    pub video_path: String,
    pub config: RegionConfig,
    pub fps_sample: u32,
    pub preprocess: bool,
    pub filter_numeric: bool,
    pub languages: Vec<String>,
    pub use_gpu: bool,
    pub backend: String,
    /// Confidence threshold (0.0–1.0): if the best oar-ocr result is at or
    /// above this value the Tesseract engines are skipped.  Defaults to 0.9.
    #[serde(default = "default_oar_threshold")]
    pub oar_confidence_threshold: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct Measurement {
    pub timestamp: f64,
    pub frame_number: u64,
    pub region_name: String,
    pub value: String,
    pub confidence: f64,
    pub raw_text: String,
    pub source: String,
}

/// Per-region result emitted inside each frame progress event.
#[derive(Debug, Serialize, Clone)]
pub struct RegionProgress {
    pub region_name: String,
    pub value: String,
    pub confidence: f64,
    pub ocr_preview: String,
    pub source: String,
}

/// One event emitted per frame (contains all regions, not one per region).
#[derive(Debug, Serialize, Clone)]
pub struct ExtractProgress {
    pub frame: u64,
    pub total: u64,
    pub timestamp: f64,
    pub elapsed_frames: u64,
    pub regions: Vec<RegionProgress>,
}

#[derive(Debug, Serialize)]
pub struct ExtractResult {
    pub measurements: Vec<Measurement>,
    pub csv: String,
}

#[tauri::command]
pub async fn extract(
    app: AppHandle,
    params: ExtractParams,
    cancel: tauri::State<'_, CancelFlag>,
) -> Result<ExtractResult, String> {
    use crate::video::get_video_info;

    if params.config.keyframes.len() < 2 {
        return Err("At least 2 keyframes are required to run extraction.".to_string());
    }

    let mut kf_ts: Vec<f64> = params.config.keyframes.iter().map(|kf| kf.timestamp).collect();
    kf_ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let first_ts = kf_ts[0];
    let last_ts  = *kf_ts.last().unwrap();

    let info = get_video_info(params.video_path.clone())?;
    let fps        = info.fps;
    let fps_sample = params.fps_sample.max(1) as u64;

    let first_frame = (first_ts * fps).round() as u64;
    let last_frame  = (last_ts  * fps).round() as u64;
    let total_steps = (last_frame.saturating_sub(first_frame)) / fps_sample + 1;

    // ── Build OCR engine lists ────────────────────────────────────────────────
    //
    // `priority` engines (oar-ocr variants) run first on every region.
    //   → If the best result exceeds `oar_confidence_threshold` the
    //     `fallback` engines (Tesseract variants) are skipped entirely.
    //
    // `fallback` engines only run when oar-ocr is not confident enough.
    //
    // To add a new OCR backend: implement `Recognizer` in `src/ocr/<backend>.rs`
    // and push a `Box<dyn Recognizer>` into one of the two lists here.

    // Priority: oar-ocr (RGB input + grayscale input)
    let priority_engines: Vec<Box<dyn Recognizer>> = {
        // Locate bundled model files.
        // Dev: src-tauri/models/ (baked in via CARGO_MANIFEST_DIR).
        // Prod: Tauri resource directory.
        use tauri::Manager as _;
        let candidates = [
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"),
            app.path()
                .resource_dir()
                .map(|d| d.join("models"))
                .unwrap_or_default(),
        ];

        let found = candidates.iter().find(|d| {
            d.join("pp-ocrv5_mobile_rec.onnx").exists() && d.join("ppocrv5_dict.txt").exists()
        });

        if let Some(dir) = found {
            let rec  = dir.join("pp-ocrv5_mobile_rec.onnx");
            let dict = dir.join("ppocrv5_dict.txt");
            match build_pipeline(rec.to_str().unwrap_or(""), dict.to_str().unwrap_or("")) {
                Ok(pipeline) => {
                    eprintln!("oar-ocr pipeline ready (models: {dir:?})");
                    let pipeline = Arc::new(pipeline);
                    vec![
                        Box::new(OarRecognizer { pipeline: pipeline.clone(), color_mode: ColorMode::Rgb })
                            as Box<dyn Recognizer>,
                        Box::new(OarRecognizer { pipeline, color_mode: ColorMode::Grayscale }),
                    ]
                }
                Err(e) => {
                    eprintln!("oar-ocr init failed: {e}");
                    vec![]
                }
            }
        } else {
            eprintln!("oar-ocr models not found — Tesseract only");
            vec![]
        }
    };

    // Locate bundled Tesseract tessdata.
    // Tesseract::new(datadir, lang) looks for <datadir>/tessdata/<lang>.traineddata.
    // Dev:  src-tauri/ (CARGO_MANIFEST_DIR) contains tessdata/ downloaded by build.rs.
    // Prod: Tauri resource_dir() contains tessdata/ bundled via tauri.conf.json.
    let tessdata_dir: Option<String> = {
        use tauri::Manager as _;
        let candidates = [
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            app.path()
                .resource_dir()
                .unwrap_or_default(),
        ];
        candidates
            .iter()
            .find(|d| d.join("tessdata").join("eng.traineddata").exists())
            .map(|d| d.to_string_lossy().to_string())
    };
    if let Some(ref td) = tessdata_dir {
        eprintln!("tesseract tessdata found at {td:?}");
    } else {
        eprintln!("tesseract tessdata not found — falling back to system default");
    }

    // Fallback: Tesseract variants (run when oar-ocr confidence is below threshold).
    //
    // When preprocessing is enabled:
    //   • Binary  — full pipeline with Sauvola binarization + morph opening
    //   • Gray    — same pipeline, stops at enhanced grayscale (no binarization)
    //   • ChannelR/G/B — extract each colour channel independently, then Binary pipeline
    //                    (helps with coloured digit displays: red LEDs, green LCDs, etc.)
    // When preprocessing is disabled: RawGray (no upscaling, minimal cost).
    // RawRgb is always included as a final Tesseract fallback.
    let fallback_engines: Vec<Box<dyn Recognizer>> = {
        let mut v: Vec<Box<dyn Recognizer>> = Vec::new();
        if params.preprocess {
            v.push(Box::new(TesseractRecognizer {
                languages: params.languages.clone(),
                preprocess: Preprocess::Binary,
                tessdata_dir: tessdata_dir.clone(),
            }));
            v.push(Box::new(TesseractRecognizer {
                languages: params.languages.clone(),
                preprocess: Preprocess::Gray,
                tessdata_dir: tessdata_dir.clone(),
            }));
            v.push(Box::new(TesseractRecognizer {
                languages: params.languages.clone(),
                preprocess: Preprocess::ChannelR,
                tessdata_dir: tessdata_dir.clone(),
            }));
            v.push(Box::new(TesseractRecognizer {
                languages: params.languages.clone(),
                preprocess: Preprocess::ChannelG,
                tessdata_dir: tessdata_dir.clone(),
            }));
            v.push(Box::new(TesseractRecognizer {
                languages: params.languages.clone(),
                preprocess: Preprocess::ChannelB,
                tessdata_dir: tessdata_dir.clone(),
            }));
        } else {
            v.push(Box::new(TesseractRecognizer {
                languages: params.languages.clone(),
                preprocess: Preprocess::RawGray,
                tessdata_dir: tessdata_dir.clone(),
            }));
        }
        // Raw RGB is always included as an additional Tesseract variant
        v.push(Box::new(TesseractRecognizer {
            languages: params.languages.clone(),
            preprocess: Preprocess::RawRgb,
            tessdata_dir,
        }));
        v
    };

    // ── Frame loop ────────────────────────────────────────────────────────────

    let flag = cancel.0.clone();
    flag.store(false, Ordering::Relaxed);

    // Clamp threshold to [0, 1] — invalid values from the frontend become safe defaults.
    let oar_threshold = params.oar_confidence_threshold.clamp(0.0, 1.0);

    // Track the last accepted numeric reading per region for deviation scoring.
    let mut prev_values: HashMap<String, f64> = HashMap::new();

    let mut measurements: Vec<Measurement> = Vec::new();
    let mut elapsed: u64 = 0;
    let mut frame_num = first_frame;

    while frame_num <= last_frame {
        if flag.load(Ordering::Relaxed) {
            break;
        }

        let timestamp = frame_num as f64 / fps;
        let regions   = params.config.get_regions_at(timestamp);

        if regions.is_empty() {
            elapsed += 1;
            frame_num += fps_sample;
            continue;
        }

        let (frame_bytes, fw, fh) = match crate::video::decode_frame_at(&params.video_path, timestamp) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("frame decode failed at {timestamp:.3}s: {e}");
                elapsed += 1;
                frame_num += fps_sample;
                continue;
            }
        };

        // Snapshot previous values before parallel processing so all regions in this
        // frame read the *previous* frame's accepted values (not each other's).
        let prev_snap = &prev_values;

        // Run OCR for all regions in parallel, producing (Measurement, RegionProgress) pairs.
        let outcomes: Vec<(Measurement, RegionProgress)> = regions
            .par_iter()
            .map(|region| {
                let expectation = params.config.expectations.get(&region.name);
                let prev_value  = prev_snap.get(&region.name).copied();
                let (value, confidence, raw_text, ocr_preview, source) = read_region(
                    &frame_bytes,
                    fw,
                    fh,
                    region.x.max(0) as u32,
                    region.y.max(0) as u32,
                    region.width.max(0) as u32,
                    region.height.max(0) as u32,
                    &priority_engines,
                    &fallback_engines,
                    params.filter_numeric,
                    oar_threshold,
                    expectation,
                    prev_value,
                );
                (
                    Measurement {
                        timestamp,
                        frame_number: frame_num,
                        region_name: region.name.clone(),
                        value: value.clone(),
                        confidence,
                        raw_text,
                        source: source.clone(),
                    },
                    RegionProgress {
                        region_name: region.name.clone(),
                        value,
                        confidence,
                        ocr_preview,
                        source,
                    },
                )
            })
            .collect();

        // Emit one batched event for the entire frame (reduces IPC calls by N_regions).
        let _ = app.emit(
            "extraction_progress",
            ExtractProgress {
                frame: frame_num,
                total: total_steps,
                timestamp,
                elapsed_frames: elapsed,
                regions: outcomes.iter().map(|(_, rp)| rp.clone()).collect(),
            },
        );

        // Update prev_values with successfully parsed readings from this frame.
        for (m, _) in &outcomes {
            if let Ok(v) = m.value.parse::<f64>() {
                prev_values.insert(m.region_name.clone(), v);
            }
        }

        measurements.extend(outcomes.into_iter().map(|(m, _)| m));
        elapsed += 1;
        frame_num += fps_sample;
    }

    // ── Build CSV string (not written to disk — user exports explicitly) ──────

    let mut csv = String::from("timestamp,frame_number,region_name,value,confidence,raw_text,source\n");
    for m in &measurements {
        csv.push_str(&format!(
            "{},{},{},{},{:.4},{},{}\n",
            m.timestamp, m.frame_number, m.region_name,
            m.value, m.confidence, m.raw_text, m.source,
        ));
    }

    Ok(ExtractResult { measurements, csv })
}

/// Write CSV content to the given path, creating parent directories if needed.
#[tauri::command]
pub fn save_csv(path: String, csv: String) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create directories: {e}"))?;
    }
    std::fs::write(&path, csv).map_err(|e| format!("Cannot write {path}: {e}"))
}
