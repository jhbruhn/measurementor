use base64::Engine;
use rayon::prelude::*;
use image::{
    codecs::png::PngEncoder, DynamicImage, GenericImageView, GrayImage, ImageBuffer, ImageEncoder,
    Luma, Rgb, RgbImage,
};
use imageproc::contrast::equalize_histogram;
use imageproc::filter::gaussian_blur_f32;
use crate::oar::OarPipeline;
use tesseract::{PageSegMode, Tesseract};

/// Run OCR on a cropped region (given as raw RGB bytes, row-major).
/// Returns (cleaned_number, confidence 0.0–1.0, raw_text, ocr_preview_png_b64, source).
///
/// `source` is `"tesseract"` or `"oar-ocr"` — whichever candidate won.
///
/// `filter_numeric`: when true, a candidate whose output parses as a float number
/// is preferred over a higher-confidence candidate that doesn't.
///
/// `oar`: optional pre-built oar-ocr pipeline.
///
/// `oar_threshold`: if the best oar-ocr confidence is at or above this value
/// (and passes the numeric filter when enabled), Tesseract is skipped entirely.
/// Otherwise all variants (oar RGB, oar gray, Tesseract binary, Tesseract gray,
/// Tesseract raw-RGB) run and the best result wins.
pub fn read_region(
    frame_bytes: &[u8],
    frame_width: u32,
    frame_height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    languages: &[String],
    preprocess: bool,
    filter_numeric: bool,
    oar: Option<&OarPipeline>,
    oar_threshold: f64,
) -> (String, f64, String, String, String) {
    // Clamp region to frame bounds
    let x2 = (x + w).min(frame_width);
    let y2 = (y + h).min(frame_height);
    if x2 <= x || y2 <= y {
        return (String::new(), 0.0, String::new(), String::new(), String::new());
    }
    let cw = x2 - x;
    let ch = y2 - y;

    // Build image from raw RGB24 bytes
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_fn(frame_width, frame_height, |px, py| {
            let off = ((py * frame_width + px) * 3) as usize;
            Rgb([frame_bytes[off], frame_bytes[off + 1], frame_bytes[off + 2]])
        });

    // Crop to the region
    let crop = image::imageops::crop_imm(&img, x, y, cw, ch).to_image();

    let lang = build_lang(languages);

    // ── Tesseract helper: 4 PSMs on a GrayImage (bpp=1) ─────────────────────
    let ocr_image = |img: GrayImage| -> (String, f64, String) {
        let iw = img.width();
        let ih = img.height();
        let bytes = img.into_raw();

        let (raw, conf) = [
            PageSegMode::PsmSingleLine,
            PageSegMode::PsmSingleBlock,
            PageSegMode::PsmSingleWord,
            PageSegMode::PsmRawLine,
        ]
        .par_iter()
        .filter_map(|&psm| try_ocr(&bytes, iw, ih, &lang, psm, 1).ok())
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or_default();

        let preview = {
            let mut png: Vec<u8> = Vec::new();
            if PngEncoder::new(&mut png)
                .write_image(&bytes, iw, ih, image::ExtendedColorType::L8)
                .is_ok()
            {
                base64::engine::general_purpose::STANDARD.encode(&png)
            } else {
                String::new()
            }
        };
        (raw, conf, preview)
    };

    // ── Tesseract helper: 4 PSMs on a raw RgbImage (bpp=3) ──────────────────
    let ocr_rgb_image = |img: RgbImage| -> (String, f64, String) {
        let iw = img.width();
        let ih = img.height();
        let bytes = img.into_raw();

        let (raw, conf) = [
            PageSegMode::PsmSingleLine,
            PageSegMode::PsmSingleBlock,
            PageSegMode::PsmSingleWord,
            PageSegMode::PsmRawLine,
        ]
        .par_iter()
        .filter_map(|&psm| try_ocr(&bytes, iw, ih, &lang, psm, 3).ok())
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or_default();

        let preview = {
            let mut png: Vec<u8> = Vec::new();
            if PngEncoder::new(&mut png)
                .write_image(&bytes, iw, ih, image::ExtendedColorType::Rgb8)
                .is_ok()
            {
                base64::engine::general_purpose::STANDARD.encode(&png)
            } else {
                String::new()
            }
        };
        (raw, conf, preview)
    };

    // ── Step 1: run oar on RGB + grayscale variants in parallel ──────────────
    let oar_best: Option<(String, f64, String)> = if let Some(pipeline) = oar {
        let oar_rgb_crop = crop.clone();
        let gray_luma    = DynamicImage::ImageRgb8(crop.clone()).to_luma8();
        let oar_gray_as_rgb = DynamicImage::ImageLuma8(gray_luma).to_rgb8();

        let (res_rgb, res_gray) = rayon::join(
            || crate::oar::ocr_oar(oar_rgb_crop, pipeline),
            || crate::oar::ocr_oar(oar_gray_as_rgb, pipeline),
        );

        [res_rgb, res_gray]
            .into_iter()
            .flatten()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    } else {
        None
    };

    // ── Step 2: early exit if oar is confident enough to skip Tesseract ──────
    let skip_tesseract = oar_best.as_ref().map_or(false, |r| {
        let conf_ok    = r.1 >= oar_threshold;
        let numeric_ok = !filter_numeric || clean_number(&r.0).parse::<f64>().is_ok();
        conf_ok && numeric_ok
    });

    if skip_tesseract {
        let (oar_raw, oar_conf, oar_preview) = oar_best.unwrap();
        let value = clean_number(&oar_raw);
        eprintln!(
            "[ocr] oar confident ({:.3} >= {:.3}), skipping Tesseract",
            oar_conf, oar_threshold
        );
        return (value, oar_conf, oar_raw.trim().to_string(), oar_preview, "oar-ocr".to_string());
    }

    // ── Step 3: run Tesseract variants in parallel ────────────────────────────
    // Left arm:  preprocessed binary + gray (or raw gray when preprocess=false)
    // Right arm: raw colour input (bpp=3) — additional variant
    let ((tess_bin, tess_gray), tess_rgb) = rayon::join(
        || {
            if preprocess {
                let binary = preprocess_region(DynamicImage::ImageRgb8(crop.clone()), true);
                let gray   = preprocess_region(DynamicImage::ImageRgb8(crop.clone()), false);
                rayon::join(|| ocr_image(binary), || ocr_image(gray))
            } else {
                let raw_gray = DynamicImage::ImageRgb8(crop.clone()).to_luma8();
                let result   = ocr_image(raw_gray);
                (result.clone(), result)
            }
        },
        || ocr_rgb_image(crop.clone()),
    );

    eprintln!(
        "[ocr] tess_bin: {:?} {:.3}  tess_gray: {:?} {:.3}  tess_rgb: {:?} {:.3}  oar: {:?}",
        tess_bin.0, tess_bin.1,
        tess_gray.0, tess_gray.1,
        tess_rgb.0, tess_rgb.1,
        oar_best.as_ref().map(|(t, c, _)| format!("{t:?} {c:.3}"))
    );

    // ── Step 4: pick best across all candidates ───────────────────────────────
    let (tess_best, _) = pick_winner(tess_bin, tess_gray, filter_numeric);
    let (tess_best, _) = pick_winner(tess_best, tess_rgb, filter_numeric);

    let (raw, conf, preview_b64, source) = match oar_best {
        Some((oar_raw, oar_conf, oar_preview)) => {
            let (winner, tess_won) = pick_winner(
                tess_best,
                (oar_raw, oar_conf, oar_preview),
                filter_numeric,
            );
            let src = if tess_won { "tesseract" } else { "oar-ocr" };
            (winner.0, winner.1, winner.2, src.to_string())
        }
        None => {
            let (r, c, p) = tess_best;
            (r, c, p, "tesseract".to_string())
        }
    };

    let value = clean_number(&raw);
    (value, conf, raw.trim().to_string(), preview_b64, source)
}

/// Feed bytes directly to Tesseract.
/// `bpp` = bytes per pixel: 1 for grayscale, 3 for RGB.
/// `mean_text_conf()` returns 0–100; we normalise to 0.0–1.0.
fn try_ocr(
    bytes: &[u8],
    w: u32,
    h: u32,
    lang: &str,
    psm: PageSegMode,
    bpp: i32,
) -> Result<(String, f64), ()> {
    let mut tess = Tesseract::new(None, Some(lang))
        .map_err(|_| ())?
        .set_frame(bytes, w as i32, h as i32, bpp, w as i32 * bpp)
        .map_err(|_| ())?;
    tess.set_page_seg_mode(psm);
    let mut tess = tess.recognize().map_err(|_| ())?;

    let raw = tess.get_text().map_err(|_| ())?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        return Err(());
    }

    let conf = tess.mean_text_conf().max(0) as f64 / 100.0;
    Ok((trimmed, conf))
}

fn build_lang(languages: &[String]) -> String {
    if languages.is_empty() {
        "eng".to_string()
    } else {
        languages
            .iter()
            .map(|l| lang_to_tesseract(l))
            .collect::<Vec<_>>()
            .join("+")
    }
}

/// Choose between two OCR candidates (raw_text, confidence, preview).
/// Returns `(winner, first_won)` where `first_won = true` means `a` was selected.
/// When `filter_numeric` is true:
///   - A candidate whose cleaned output parses as f64 beats one that doesn't,
///     regardless of confidence.
///   - If both (or neither) parse as a number, fall back to higher confidence.
fn pick_winner(
    a: (String, f64, String),
    b: (String, f64, String),
    filter_numeric: bool,
) -> ((String, f64, String), bool) {
    if filter_numeric {
        let a_num = clean_number(&a.0).parse::<f64>().is_ok();
        let b_num = clean_number(&b.0).parse::<f64>().is_ok();
        match (a_num, b_num) {
            (true, false) => (a, true),
            (false, true) => (b, false),
            _ => if b.1 > a.1 { (b, false) } else { (a, true) },
        }
    } else {
        if b.1 > a.1 { (b, false) } else { (a, true) }
    }
}

/// Preprocess a region image before OCR.
/// `binarize`: when true applies the binary threshold (black/white);
///             when false stops at enhanced grayscale.
/// Pipeline: 6× upscale → grayscale → auto-invert → hist-eq → Gaussian blur
///           → (optional binary threshold) → 15 px white border
fn preprocess_region(img: DynamicImage, binarize: bool) -> GrayImage {
    let (w, h) = img.dimensions();

    // 1. 6× upscale with Lanczos3
    let scaled = img.resize(w * 6, h * 6, image::imageops::FilterType::Lanczos3);

    // 2. Grayscale
    let mut gray = scaled.to_luma8();

    // 3. Auto-invert if the background is dark (mean pixel < 140)
    let mean: u64 = gray.pixels().map(|p| p[0] as u64).sum::<u64>()
        / (gray.width() * gray.height()).max(1) as u64;
    if mean < 140 {
        for p in gray.pixels_mut() {
            p[0] = 255 - p[0];
        }
    }

    // 3.5 Histogram equalisation
    gray = equalize_histogram(&gray);

    // 3.6 Mild Gaussian blur (σ=1.0) to suppress compression artefacts
    gray = gaussian_blur_f32(&gray, 1.0);

    // 4. Binary threshold (optional)
    if binarize {
        for p in gray.pixels_mut() {
            p[0] = if p[0] > 100 { 255 } else { 0 };
        }
    }

    // 5. Add 15 px white border (helps Tesseract find the text block)
    let pad = 15u32;
    let (gw, gh) = gray.dimensions();
    let mut padded = GrayImage::from_pixel(gw + pad * 2, gh + pad * 2, Luma([255u8]));
    image::imageops::overlay(&mut padded, &gray, pad as i64, pad as i64);

    padded
}

/// Map user-facing language codes to Tesseract language identifiers.
fn lang_to_tesseract(lang: &str) -> &str {
    match lang.trim() {
        "en" | "eng" => "eng",
        "de" | "deu" => "deu",
        "fr" | "fra" => "fra",
        "es" | "spa" => "spa",
        other => other,
    }
}

/// Normalise the raw OCR output to a clean number string.
pub fn clean_number(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();

    // comma → dot, strip apostrophe thousands-separators
    let s: String = line
        .chars()
        .map(|c| if c == ',' { '.' } else { c })
        .filter(|c| *c != '\'')
        .collect();

    // Fix common OCR letter/digit confusion
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

    // Extract first occurrence of: optional minus, digits, optional decimal part
    let mut result = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '-' || c.is_ascii_digit() {
            let start = i;
            if c == '-' { i += 1; }
            while i < bytes.len() && (bytes[i] as char).is_ascii_digit() { i += 1; }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                while i < bytes.len() && (bytes[i] as char).is_ascii_digit() { i += 1; }
            }
            result = s[start..i].to_string();
            break;
        }
        i += 1;
    }
    result
}
