pub mod oar;
pub mod tesseract;

use crate::config::RegionExpectation;
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
///   If the best priority result's *validation-adjusted* confidence reaches
///   `fast_threshold`, the `fallback` engines are skipped entirely.
///   Validation uses `expectation` if provided; a result that violates range /
///   decimal / digit constraints has its effective confidence reduced, which
///   may prevent a confident-but-wrong reading from short-circuiting fallback.
/// - Otherwise `fallback` engines also run and the best candidate across **all**
///   engines (scored by confidence × validation) wins.
/// - `prev_value`: the accepted numeric reading from the previous frame for this
///   region, used to score deviation-constrained expectations.
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
    fast_threshold: f64,
    expectation: Option<&RegionExpectation>,
    prev_value: Option<f64>,
) -> (String, f64, String, String, String) {
    // Prefer numeric results when the region is marked as numeric.
    let filter_numeric = expectation.map_or(false, |e| e.numeric);

    let x2 = (x + w).min(frame_width);
    let y2 = (y + h).min(frame_height);
    if x2 <= x || y2 <= y {
        return (
            String::new(),
            0.0,
            String::new(),
            String::new(),
            String::new(),
        );
    }

    let crop = build_crop(frame_bytes, frame_width, frame_height, x, y, x2 - x, y2 - y);
    let crop_dyn = DynamicImage::ImageRgb8(crop);

    // ── Step 1: priority engines (fast path) ─────────────────────────────────
    let mut priority_results: Vec<OcrResult> = priority
        .par_iter()
        .filter_map(|e| e.recognize(&crop_dyn))
        .collect();

    // Fast-path: skip fallback only when the best priority result is confident
    // enough AND satisfies hard constraints.  An out-of-range result must not
    // short-circuit the fallback engines — one of them might produce a valid value.
    if let Some(best) =
        best_result_constrained(&priority_results, filter_numeric, expectation, prev_value)
    {
        let v_score = expectation
            .map(|e| validation_score(&best.text, e, prev_value))
            .unwrap_or(1.0);
        let eff_conf = best.confidence * v_score;
        let numeric_ok = !filter_numeric || clean_number(&best.text).parse::<f64>().is_ok();
        if eff_conf >= fast_threshold && numeric_ok {
            eprintln!(
                "[ocr] fast-path via {} (eff={:.3} ≥ {:.3}), skipping fallback",
                best.engine_name, eff_conf, fast_threshold
            );
            return make_result(best.clone(), filter_numeric);
        }
    }

    // ── Step 2: fallback engines ──────────────────────────────────────────────
    let fallback_results: Vec<OcrResult> = fallback
        .par_iter()
        .filter_map(|e| e.recognize(&crop_dyn))
        .collect();

    // Debug log all candidates
    for r in priority_results.iter().chain(fallback_results.iter()) {
        let vscore = expectation
            .map(|e| validation_score(&r.text, e, prev_value))
            .unwrap_or(1.0);
        eprintln!(
            "[ocr]  {:25}  {:?}  conf={:.3}  valid={:.3}",
            r.engine_name, r.text, r.confidence, vscore
        );
    }

    // ── Step 3: pick best constraint-satisfying candidate ─────────────────────
    // Among all engine outputs, prefer those that pass hard constraints
    // (min/max range, max_deviation).  If no engine produced a valid reading,
    // return an empty value rather than reporting a known-bad result.
    priority_results.extend(fallback_results);
    match best_result_constrained(&priority_results, filter_numeric, expectation, prev_value) {
        Some(w) => make_result(w.clone(), filter_numeric),
        None => {
            // No candidate passed hard constraints — report empty.
            eprintln!("[ocr] hard-filter: no candidate satisfied constraints → empty");
            (
                String::new(),
                0.0,
                String::new(),
                String::new(),
                String::new(),
            )
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_crop(frame_bytes: &[u8], fw: u32, fh: u32, x: u32, y: u32, w: u32, h: u32) -> RgbImage {
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(fw, fh, |px, py| {
        let off = ((py * fw + px) * 3) as usize;
        Rgb([frame_bytes[off], frame_bytes[off + 1], frame_bytes[off + 2]])
    });
    image::imageops::crop_imm(&img, x, y, w, h).to_image()
}

/// Returns `true` when `text` satisfies the hard constraints in `exp`
/// (min/max range, max_deviation from prev_value).
/// Non-numeric results always fail when `exp.numeric` is set.
fn passes_hard_constraints(text: &str, exp: &RegionExpectation, prev_value: Option<f64>) -> bool {
    if !exp.numeric {
        return true; // no numeric constraint → anything passes
    }
    let cleaned = clean_number(text);
    let Ok(v) = cleaned.parse::<f64>() else {
        return false; // can't parse → violates numeric constraint
    };
    let in_range = exp.min.map_or(true, |m| v >= m) && exp.max.map_or(true, |m| v <= m);
    let ok_deviation = match (exp.max_deviation, prev_value) {
        (Some(md), Some(pv)) if md > 0.0 => (v - pv).abs() <= md,
        _ => true,
    };
    in_range && ok_deviation
}

/// Like `best_result` but restricted to candidates that pass hard constraints.
///
/// If any candidate satisfies `passes_hard_constraints`, only those are scored
/// and the best among them is returned.  If NO candidate satisfies the
/// constraints, `None` is returned so the caller can emit an empty reading
/// rather than reporting a known-bad value.
fn best_result_constrained<'a>(
    results: &'a [OcrResult],
    filter_numeric: bool,
    expectation: Option<&RegionExpectation>,
    prev_value: Option<f64>,
) -> Option<&'a OcrResult> {
    let Some(exp) = expectation else {
        return best_result(results, filter_numeric, expectation, prev_value);
    };

    // Collect indices of candidates that pass the hard constraints so we can
    // return a reference into `results` without cloning.
    let valid_indices: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| passes_hard_constraints(&r.text, exp, prev_value))
        .map(|(i, _)| i)
        .collect();

    if valid_indices.is_empty() {
        return None;
    }

    let score = |r: &OcrResult| -> f64 {
        let v = validation_score(&r.text, exp, prev_value);
        r.confidence * v
    };

    let best_idx = if filter_numeric {
        let numeric_indices: Vec<usize> = valid_indices
            .iter()
            .copied()
            .filter(|&i| clean_number(&results[i].text).parse::<f64>().is_ok())
            .collect();
        let pool = if numeric_indices.is_empty() {
            &valid_indices
        } else {
            &numeric_indices
        };
        *pool
            .iter()
            .max_by(|&&a, &&b| {
                score(&results[a])
                    .partial_cmp(&score(&results[b]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(&valid_indices[0])
    } else {
        *valid_indices
            .iter()
            .max_by(|&&a, &&b| {
                score(&results[a])
                    .partial_cmp(&score(&results[b]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(&valid_indices[0])
    };

    Some(&results[best_idx])
}

/// Select the best `OcrResult` from a slice.
///
/// Scoring: `confidence × validation_score`.
/// When `filter_numeric` is set, numeric-parseable results always beat
/// non-numeric ones (within each tier the highest combined score wins).
fn best_result<'a>(
    results: &'a [OcrResult],
    filter_numeric: bool,
    expectation: Option<&RegionExpectation>,
    prev_value: Option<f64>,
) -> Option<&'a OcrResult> {
    if results.is_empty() {
        return None;
    }

    let score = |r: &OcrResult| -> f64 {
        let v = expectation
            .map(|e| validation_score(&r.text, e, prev_value))
            .unwrap_or(1.0);
        r.confidence * v
    };

    if filter_numeric {
        let numeric: Vec<&OcrResult> = results
            .iter()
            .filter(|r| clean_number(&r.text).parse::<f64>().is_ok())
            .collect();
        if !numeric.is_empty() {
            return numeric.into_iter().max_by(|a, b| {
                score(a)
                    .partial_cmp(&score(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
    results.iter().max_by(|a, b| {
        score(a)
            .partial_cmp(&score(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

// ── Validation scoring ────────────────────────────────────────────────────────

/// Returns a multiplier in (0, 1] that scales down `confidence` when the OCR text
/// violates the region's content expectations.  All penalties are multiplicative
/// and stack; a result that fails every check approaches 0 but never reaches it,
/// so we always have a winner even when all engines produce garbage.
fn validation_score(text: &str, exp: &RegionExpectation, prev_value: Option<f64>) -> f64 {
    if !exp.numeric {
        return 1.0; // no numeric expectation → no penalty
    }

    let cleaned = clean_number(text);
    let Ok(value) = cleaned.parse::<f64>() else {
        return 0.1; // can't parse as number at all → heavy penalty
    };

    let mut score = 1.0f64;

    // Out-of-range → strong penalty (0.4×)
    if let Some(min) = exp.min {
        if value < min {
            score *= 0.4;
        }
    }
    if let Some(max) = exp.max {
        if value > max {
            score *= 0.4;
        }
    }

    // Wrong decimal-place count → moderate penalty (0.65×)
    if let Some(expected_dp) = exp.decimal_places {
        if count_decimal_places(&cleaned) != expected_dp {
            score *= 0.65;
        }
    }

    // Wrong total digit count → moderate penalty (0.65×)
    if let Some(expected_td) = exp.total_digits {
        if count_total_digits(&cleaned) != expected_td {
            score *= 0.65;
        }
    }

    // Deviation from previous value too large → medium penalty (0.5×)
    if let (Some(max_dev), Some(prev)) = (exp.max_deviation, prev_value) {
        if max_dev > 0.0 && (value - prev).abs() > max_dev {
            score *= 0.5;
        }
    }

    score
}

/// Number of digits after the decimal point in a numeric string ("3.14" → 2, "42" → 0).
fn count_decimal_places(s: &str) -> u32 {
    s.find('.')
        .map(|pos| (s.len() - pos - 1) as u32)
        .unwrap_or(0)
}

/// Total count of ASCII digit characters in a string ("3.14" → 3, "-007" → 3).
fn count_total_digits(s: &str) -> u32 {
    s.chars().filter(|c| c.is_ascii_digit()).count() as u32
}

fn make_result(r: OcrResult, filter_numeric: bool) -> (String, f64, String, String, String) {
    let value = if filter_numeric {
        clean_number(&r.text)
    } else {
        r.text.trim().to_string()
    };
    (
        value,
        r.confidence,
        r.text.trim().to_string(),
        r.preview_b64,
        r.engine_name,
    )
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
