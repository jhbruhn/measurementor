use base64::Engine;
use image::{codecs::png::PngEncoder, imageops::FilterType, DynamicImage, ImageEncoder};
use oar_ocr::predictors::TextRecognitionPredictor;
use std::sync::Arc;

use super::{OcrResult, Recognizer};

/// Minimum height fed to PaddleOCR v5 mobile (normalises inputs to 48 px internally).
const MIN_HEIGHT: u32 = 48;

// ── Pipeline ──────────────────────────────────────────────────────────────────

pub struct OarPipeline {
    rec: TextRecognitionPredictor,
}

// ONNX Runtime sessions are not `Send`/`Sync` by default, but in practice the
// recognition predictor is stateless between calls and safe to share.
unsafe impl Send for OarPipeline {}
unsafe impl Sync for OarPipeline {}

/// Build the recognition pipeline from on-disk ONNX model and dict files.
pub fn build_pipeline(rec_model: &str, dict: &str) -> Result<OarPipeline, String> {
    let rec = TextRecognitionPredictor::builder()
        .dict_path(dict)
        .score_threshold(0.0)
        .build(rec_model)
        .map_err(|e| e.to_string())?;
    Ok(OarPipeline { rec })
}

// ── Recognizer impl ───────────────────────────────────────────────────────────

/// Which colour space to present to the recognition model.
pub enum ColorMode {
    /// Pass the crop as-is (RGB).
    Rgb,
    /// Convert to grayscale and promote back to RGB (L, L, L channels).
    Grayscale,
}

pub struct OarRecognizer {
    pub pipeline: Arc<OarPipeline>,
    pub color_mode: ColorMode,
}

impl Recognizer for OarRecognizer {
    fn name(&self) -> &str {
        match self.color_mode {
            ColorMode::Rgb       => "oar-ocr/rgb",
            ColorMode::Grayscale => "oar-ocr/gray",
        }
    }

    fn recognize(&self, crop: &DynamicImage) -> Option<OcrResult> {
        // Prepare the image in the requested colour space
        let img = match self.color_mode {
            ColorMode::Rgb       => crop.to_rgb8(),
            ColorMode::Grayscale => DynamicImage::ImageLuma8(crop.to_luma8()).to_rgb8(),
        };

        let (orig_w, orig_h) = (img.width(), img.height());

        // Upscale to at least MIN_HEIGHT (PaddleOCR v5 normalises to 48 px)
        let img = if orig_h < MIN_HEIGHT {
            let scale = (MIN_HEIGHT + orig_h - 1) / orig_h;
            DynamicImage::ImageRgb8(img)
                .resize(orig_w * scale, orig_h * scale, FilterType::Lanczos3)
                .to_rgb8()
        } else {
            img
        };

        eprintln!(
            "[oar] {} {}×{} (orig {}×{})",
            self.name(), img.width(), img.height(), orig_w, orig_h
        );

        // Encode preview of exactly what the model receives
        let preview = {
            let mut png = Vec::new();
            if PngEncoder::new(&mut png)
                .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
                .is_ok()
            {
                base64::engine::general_purpose::STANDARD.encode(&png)
            } else {
                String::new()
            }
        };

        let result = match self.pipeline.rec.predict(vec![img]) {
            Ok(r)  => r,
            Err(e) => { eprintln!("[oar] predict error: {e}"); return None; }
        };

        let text  = result.texts.into_iter().next()?;
        let score = result.scores.into_iter().next().unwrap_or(0.0);

        eprintln!("[oar] {} result: {:?} conf={score:.3}", self.name(), text);

        if text.is_empty() {
            return None;
        }

        Some(OcrResult {
            text,
            confidence: score as f64,
            preview_b64: preview,
            engine_name: self.name().to_string(),
        })
    }
}
