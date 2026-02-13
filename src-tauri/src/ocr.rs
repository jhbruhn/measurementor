pub mod oar;
pub mod tesseract;

use image::{DynamicImage, ImageBuffer, Rgb, RgbImage};
use rayon::prelude::*;

// ── Public types ─────────────────────────────────────────────────────────────

/// Result produced by a single `Recognizer` for one crop.
#[derive(Clone, Default)]
pub struct OcrResult {
    pub text: String,
    pub confidence: f64,     // 0.0 – 1.0
    pub preview_b64: String, // base64 PNG of what the engine actually processed
    pub engine_name: String, // e.g. "tesseract/binary", "oar-ocr/rgb"
}

/// Every OCR backend implements this.
/// `recognize` receives the pre-cropped RGB image by reference so the caller
/// does not have to clone the frame buffer for each engine.
/// Engines clone internally only what they actually need for preprocessing.
pub trait Recognizer: Send + Sync {
    fn name(&self) -> &str;
    fn recognize(&self, crop: &DynamicImage) -> Option<OcrResult>;
}

// ── Orchestration ────────────────────────────────────────────────────────────

/// Run OCR on a frame region.
///
/// - `priority` engines run first (in parallel, e.g. oar-ocr).
///   If any result reaches `fast_threshold` *and* passes the numeric filter,
///   the `fallback` engines are skipped entirely.
/// - Otherwise `fallback` engines (e.g. Tesseract variants) also run and the
///   best candidate across **all** engines wins.
///
/// Returns `(value, confidence, raw_text, preview_b64, engine_name)`.
pub fn read_region(
    frame_bytes: &[u8],
    frame_width: u32,
    frame_height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    priority: &[Box<dyn Recognizer>],
    fallback: &[Box<dyn Recognizer>],
    filter_numeric: bool,
    fast_threshold: f64,
) -> (String, f64, String, String, String) {
    let x2 = (x + w).min(frame_width);
    let y2 = (y + h).min(frame_height);
    if x2 <= x || y2 <= y {
        return (String::new(), 0.0, String::new(), String::new(), String::new());
    }

    let crop = build_crop(frame_bytes, frame_width, frame_height, x, y, x2 - x, y2 - y);
    let crop_dyn = DynamicImage::ImageRgb8(crop);

    // ── Step 1: priority engines (fast path) ─────────────────────────────────
    let mut priority_results: Vec<OcrResult> = priority
        .par_iter()
        .filter_map(|e| e.recognize(&crop_dyn))
        .collect();

    // Check whether the best priority result is confident enough to skip fallback.
    if let Some(best) = best_result(&priority_results, filter_numeric) {
        let numeric_ok = !filter_numeric || clean_number(&best.text).parse::<f64>().is_ok();
        if best.confidence >= fast_threshold && numeric_ok {
            eprintln!(
                "[ocr] fast-path via {} ({:.3} ≥ {:.3}), skipping fallback engines",
                best.engine_name, best.confidence, fast_threshold
            );
            return make_result(best.clone());
        }
    }

    // ── Step 2: fallback engines ──────────────────────────────────────────────
    let fallback_results: Vec<OcrResult> = fallback
        .par_iter()
        .filter_map(|e| e.recognize(&crop_dyn))
        .collect();

    // Debug log all candidates
    for r in priority_results.iter().chain(fallback_results.iter()) {
        eprintln!(
            "[ocr]  {:25}  {:?}  conf={:.3}",
            r.engine_name, r.text, r.confidence
        );
    }

    // ── Step 3: pick best from all candidates ─────────────────────────────────
    priority_results.extend(fallback_results);
    let winner = best_result(&priority_results, filter_numeric)
        .cloned()
        .unwrap_or_default();

    make_result(winner)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_crop(frame_bytes: &[u8], fw: u32, fh: u32, x: u32, y: u32, w: u32, h: u32) -> RgbImage {
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(fw, fh, |px, py| {
        let off = ((py * fw + px) * 3) as usize;
        Rgb([frame_bytes[off], frame_bytes[off + 1], frame_bytes[off + 2]])
    });
    image::imageops::crop_imm(&img, x, y, w, h).to_image()
}

/// Select the best `OcrResult` from a slice, honouring `filter_numeric`.
/// A result whose cleaned text parses as `f64` beats one that doesn't,
/// regardless of confidence; ties fall back to higher confidence.
fn best_result<'a>(results: &'a [OcrResult], filter_numeric: bool) -> Option<&'a OcrResult> {
    if results.is_empty() {
        return None;
    }
    if filter_numeric {
        let numeric: Vec<&OcrResult> = results
            .iter()
            .filter(|r| clean_number(&r.text).parse::<f64>().is_ok())
            .collect();
        if !numeric.is_empty() {
            return numeric
                .into_iter()
                .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal));
        }
    }
    results
        .iter()
        .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
}

fn make_result(r: OcrResult) -> (String, f64, String, String, String) {
    let value = clean_number(&r.text);
    (value, r.confidence, r.text.trim().to_string(), r.preview_b64, r.engine_name)
}

/// Normalise raw OCR output to a clean number string.
pub fn clean_number(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();

    // comma → decimal point; strip apostrophe thousands-separators
    let s: String = line
        .chars()
        .map(|c| if c == ',' { '.' } else { c })
        .filter(|c| *c != '\'')
        .collect();

    // Common OCR letter/digit confusions
    let s: String = s
        .chars()
        .map(|c| match c {
            'O' => '0',
            'l' | 'I' => '1',
            'S' => '5',
            _ => c,
        })
        .collect();

    // Strip degree symbols and spaces
    let s: String = s.chars().filter(|c| *c != '°' && *c != ' ').collect();

    // Extract first number-like token (optional minus + digits + optional decimal)
    let mut result = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '-' || c.is_ascii_digit() {
            let start = i;
            if c == '-' {
                i += 1;
            }
            while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                    i += 1;
                }
            }
            result = s[start..i].to_string();
            break;
        }
        i += 1;
    }
    result
}
