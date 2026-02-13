use crate::config::RegionConfig;
use crate::oar::build_pipeline;
use crate::ocr::read_region;
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
    pub output_path: String,
    pub use_gpu: bool,
    pub backend: String,
    /// Confidence threshold (0.0–1.0) above which oar-ocr result is accepted
    /// immediately without running Tesseract.  Defaults to 0.9 (90 %).
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
    pub source: String,  // "tesseract" or "oar-ocr"
}

#[derive(Debug, Serialize, Clone)]
pub struct ExtractProgress {
    pub frame: u64,
    pub total: u64,   // total steps between first and last keyframe
    pub timestamp: f64,
    pub region_name: String,
    pub value: String,
    pub confidence: f64,
    pub elapsed_frames: u64,
    pub ocr_preview: String,
    pub source: String,  // "tesseract" or "oar-ocr"
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

    // Require at least two keyframes
    if params.config.keyframes.len() < 2 {
        return Err("At least 2 keyframes are required to run extraction.".to_string());
    }

    // Determine the frame range from the first and last keyframe timestamps
    let mut kf_ts: Vec<f64> = params
        .config
        .keyframes
        .iter()
        .map(|kf| kf.timestamp)
        .collect();
    kf_ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let first_ts = kf_ts[0];
    let last_ts  = *kf_ts.last().unwrap();

    let info = get_video_info(params.video_path.clone())?;
    let fps        = info.fps;
    let fps_sample = params.fps_sample.max(1) as u64;

    let first_frame = (first_ts * fps).round() as u64;
    let last_frame  = (last_ts  * fps).round() as u64;
    let total_steps = (last_frame.saturating_sub(first_frame)) / fps_sample + 1;

    // Try to build the oar-ocr pipeline from bundled model files.
    //
    // Path resolution:
    //   dev build  → src-tauri/models/  (CARGO_MANIFEST_DIR, baked in at compile time)
    //   production → <resource_dir>/models/  (Tauri bundle resource directory)
    //
    // If any model file is absent the pipeline is silently skipped.
    let oar_pipeline = {
        use tauri::Manager as _;

        // Candidate directories in preference order.
        let candidates: &[std::path::PathBuf] = &[
            // Dev: models/ sits next to Cargo.toml in src-tauri/
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"),
            // Production: bundled resource directory
            app.path()
                .resource_dir()
                .map(|d| d.join("models"))
                .unwrap_or_default(),
        ];

        let mut found_dir: Option<&std::path::Path> = None;
        for dir in candidates {
            if dir.join("pp-ocrv5_mobile_rec.onnx").exists()
                && dir.join("ppocrv5_dict.txt").exists()
            {
                found_dir = Some(dir.as_path());
                break;
            }
        }

        if let Some(dir) = found_dir {
            let rec  = dir.join("pp-ocrv5_mobile_rec.onnx");
            let dict = dir.join("ppocrv5_dict.txt");
            match build_pipeline(
                rec.to_str().unwrap_or(""),
                dict.to_str().unwrap_or(""),
            ) {
                Ok(p)  => { eprintln!("oar-ocr pipeline ready (models: {dir:?})"); Some(p) }
                Err(e) => { eprintln!("oar-ocr init failed: {e}"); None }
            }
        } else {
            eprintln!("oar-ocr models not found in any candidate directory — Tesseract only");
            None
        }
    };
    let oar_ref = oar_pipeline.as_ref();

    // Clone the Arc so we own the flag for the duration of the loop
    let flag = cancel.0.clone();
    flag.store(false, Ordering::Relaxed);

    let mut measurements: Vec<Measurement> = Vec::new();
    let mut elapsed: u64 = 0;
    let mut frame_num = first_frame;

    // Frames are processed sequentially.
    // Within each frame, all regions × preprocessing variants × PSM modes
    // run in parallel via rayon (regions.par_iter + rayon::join + PSM par_iter).
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

        let (frame_bytes, fw, fh) = match crate::video::decode_frame_at(
            &params.video_path,
            timestamp,
        ) {
            Ok(t) => t,
            Err(_) => {
                elapsed += 1;
                frame_num += fps_sample;
                continue;
            }
        };

        // Process all regions for this frame in parallel.
        // read_region itself uses rayon::join for the two preprocessing variants
        // and par_iter for the four PSM modes — giving full parallelism within the frame.
        let frame_ms: Vec<Measurement> = regions
            .par_iter()
            .map(|region| {
                let (value, confidence, raw_text, ocr_preview, source) = read_region(
                    &frame_bytes,
                    fw,
                    fh,
                    region.x.max(0) as u32,
                    region.y.max(0) as u32,
                    region.width.max(0) as u32,
                    region.height.max(0) as u32,
                    &params.languages,
                    params.preprocess,
                    params.filter_numeric,
                    oar_ref,
                    params.oar_confidence_threshold,
                );

                let _ = app.emit(
                    "extraction_progress",
                    ExtractProgress {
                        frame: frame_num,
                        total: total_steps,
                        timestamp,
                        region_name: region.name.clone(),
                        value: value.clone(),
                        confidence,
                        elapsed_frames: elapsed,
                        ocr_preview,
                        source: source.clone(),
                    },
                );

                Measurement {
                    timestamp,
                    frame_number: frame_num,
                    region_name: region.name.clone(),
                    value,
                    confidence,
                    raw_text,
                    source,
                }
            })
            .collect();

        measurements.extend(frame_ms);
        elapsed += 1;
        frame_num += fps_sample;
    }

    // Build CSV
    let mut csv = String::from(
        "timestamp,frame_number,region_name,value,confidence,raw_text,source\n",
    );
    for m in &measurements {
        csv.push_str(&format!(
            "{},{},{},{},{:.4},{},{}\n",
            m.timestamp, m.frame_number, m.region_name,
            m.value, m.confidence, m.raw_text, m.source,
        ));
    }

    // Write CSV to disk (best-effort)
    if let Some(parent) = std::path::Path::new(&params.output_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&params.output_path, &csv);

    Ok(ExtractResult { measurements, csv })
}
