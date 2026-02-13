use base64::Engine;
use image::{codecs::png::PngEncoder, imageops::FilterType, DynamicImage, ImageEncoder, RgbImage};
use oar_ocr::predictors::TextRecognitionPredictor;

/// Minimum height (px) fed to the PaddleOCR recognition model.
/// The mobile v5 model normalises its input to 48 px tall; feeding something
/// taller is fine, but shorter images produce poor results.
const MIN_HEIGHT: u32 = 48;

/// Thin wrapper that owns the recognition predictor.
pub struct OarPipeline {
    rec: TextRecognitionPredictor,
}

// The underlying ORT session is Send+Sync, so OarPipeline can be shared across rayon threads.
unsafe impl Send for OarPipeline {}
unsafe impl Sync for OarPipeline {}

/// Build a recognition-only pipeline (no text-detection step needed — our
/// regions are already cropped to the text area).
pub fn build_pipeline(rec_model: &str, dict: &str) -> Result<OarPipeline, String> {
    let rec = TextRecognitionPredictor::builder()
        .dict_path(dict)
        // score_threshold(0) — don't filter here; pick_winner handles that
        .score_threshold(0.0)
        .build(rec_model)
        .map_err(|e| e.to_string())?;
    Ok(OarPipeline { rec })
}

/// Run recognition on a pre-cropped RGB image.
/// Returns `(raw_text, confidence_0_to_1, preview_png_b64)` or `None` if nothing was recognised.
/// The preview is the upscaled RGB image that was actually fed to the model.
pub fn ocr_oar(img: RgbImage, pipeline: &OarPipeline) -> Option<(String, f64, String)> {
    let (iw, ih) = (img.width(), img.height());

    // Upscale so the height meets the model's minimum; keep aspect ratio.
    let img = if ih < MIN_HEIGHT {
        let scale = (MIN_HEIGHT + ih - 1) / ih; // ceiling division
        DynamicImage::ImageRgb8(img)
            .resize(iw * scale, ih * scale, FilterType::Lanczos3)
            .to_rgb8()
    } else {
        img
    };

    eprintln!("[oar] input {}×{} (original {}×{})", img.width(), img.height(), iw, ih);

    // Encode the image fed to the model as a preview.
    let preview = {
        let mut png: Vec<u8> = Vec::new();
        if PngEncoder::new(&mut png)
            .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
            .is_ok()
        {
            base64::engine::general_purpose::STANDARD.encode(&png)
        } else {
            String::new()
        }
    };

    let result = match pipeline.rec.predict(vec![img]) {
        Ok(r)  => r,
        Err(e) => { eprintln!("[oar] predict error: {e}"); return None; }
    };

    let text  = result.texts.into_iter().next()?;
    let score = result.scores.into_iter().next().unwrap_or(0.0);

    eprintln!("[oar] {:?} conf={score:.3}", text);

    if text.is_empty() {
        return None;
    }

    Some((text, score as f64, preview))
}
