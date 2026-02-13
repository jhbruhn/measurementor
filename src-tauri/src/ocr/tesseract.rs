use base64::Engine;
use image::{
    codecs::png::PngEncoder, DynamicImage, GenericImageView, GrayImage, ImageEncoder, Luma, RgbImage,
};
use imageproc::{contrast::equalize_histogram, filter::gaussian_blur_f32};
use rayon::prelude::*;
use tesseract::{PageSegMode, Tesseract};

use super::{OcrResult, Recognizer};

const PSMS: &[PageSegMode] = &[
    PageSegMode::PsmSingleLine,
    PageSegMode::PsmSingleBlock,
    PageSegMode::PsmSingleWord,
    PageSegMode::PsmRawLine,
];

/// Upscale factor applied before Tesseract to improve recognition on small crops.
const UPSCALE_FACTOR: u32 = 6;
/// Threshold for binary preprocessing: pixels above this become white, below become black.
const BINARY_THRESHOLD: u8 = 100;
/// Mean pixel brightness below which a crop is assumed to have a dark background
/// and needs auto-inversion before OCR.
const DARK_BG_THRESHOLD: u64 = 140;
/// White border added after preprocessing to help Tesseract locate the text block.
const BORDER_PAD: u32 = 15;

/// How to prepare the crop before handing it to Tesseract.
pub enum Preprocess {
    /// 6× upscale → grayscale → auto-invert → hist-eq → blur → binary threshold.
    Binary,
    /// Same as `Binary` but without the final threshold (enhanced grayscale).
    Gray,
    /// Just `to_luma8()` — minimal preprocessing, fast.
    RawGray,
    /// Pass the raw RGB bytes directly to Tesseract (bpp = 3, no grayscale conversion).
    RawRgb,
}

pub struct TesseractRecognizer {
    pub languages: Vec<String>,
    pub preprocess: Preprocess,
}

impl Recognizer for TesseractRecognizer {
    fn name(&self) -> &str {
        match self.preprocess {
            Preprocess::Binary  => "tesseract/binary",
            Preprocess::Gray    => "tesseract/gray",
            Preprocess::RawGray => "tesseract/raw-gray",
            Preprocess::RawRgb  => "tesseract/rgb",
        }
    }

    fn recognize(&self, crop: &DynamicImage) -> Option<OcrResult> {
        let lang = build_lang(&self.languages);
        match self.preprocess {
            Preprocess::RawRgb => {
                let img = crop.to_rgb8();
                run_ocr_bytes(img.as_raw(), img.width(), img.height(), 3, &lang, self.name())
            }
            _ => {
                let img = self.prepare_gray(crop);
                run_ocr_bytes(img.as_raw(), img.width(), img.height(), 1, &lang, self.name())
            }
        }
    }
}

impl TesseractRecognizer {
    fn prepare_gray(&self, crop: &DynamicImage) -> GrayImage {
        match self.preprocess {
            Preprocess::Binary  => preprocess_bw(crop.clone(), true),
            Preprocess::Gray    => preprocess_bw(crop.clone(), false),
            Preprocess::RawGray => crop.to_luma8(),
            Preprocess::RawRgb  => unreachable!(),
        }
    }
}

/// Run all PSMs in parallel on raw bytes, build OcrResult from the best outcome.
/// `bpp` = bytes-per-pixel (1 for grayscale, 3 for RGB).
fn run_ocr_bytes(
    bytes: &[u8],
    w: u32,
    h: u32,
    bpp: i32,
    lang: &str,
    engine_name: &str,
) -> Option<OcrResult> {
    let (text, confidence) = PSMS
        .par_iter()
        .filter_map(|&psm| try_ocr(bytes, w, h, lang, psm, bpp).ok())
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    let color = if bpp == 1 {
        image::ExtendedColorType::L8
    } else {
        image::ExtendedColorType::Rgb8
    };
    let preview_b64 = encode_png(bytes, w, h, color);

    Some(OcrResult {
        text,
        confidence,
        preview_b64,
        engine_name: engine_name.to_string(),
    })
}

/// Call Tesseract for one PSM.
/// `bpp` = bytes-per-pixel: 1 for grayscale, 3 for RGB.
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

/// Full preprocessing pipeline.
/// `binarize = true`  → add binary threshold at the end.
/// `binarize = false` → stop at enhanced grayscale.
///
/// Pipeline: Lanczos `UPSCALE_FACTOR`× → grayscale → auto-invert dark bg →
///           histogram equalisation → σ=1.0 Gaussian blur →
///           (optional binary threshold) → `BORDER_PAD` px white border.
fn preprocess_bw(img: DynamicImage, binarize: bool) -> GrayImage {
    let (w, h) = img.dimensions();
    let scaled = img.resize(w * UPSCALE_FACTOR, h * UPSCALE_FACTOR, image::imageops::FilterType::Lanczos3);
    let mut gray = scaled.to_luma8();

    // Auto-invert if background is dark
    let px_count = (gray.width() * gray.height()).max(1) as u64;
    let mean: u64 = gray.pixels().map(|p| p[0] as u64).sum::<u64>() / px_count;
    if mean < DARK_BG_THRESHOLD {
        for p in gray.pixels_mut() {
            p[0] = 255 - p[0];
        }
    }

    gray = equalize_histogram(&gray);
    gray = gaussian_blur_f32(&gray, 1.0);

    if binarize {
        for p in gray.pixels_mut() {
            p[0] = if p[0] > BINARY_THRESHOLD { 255 } else { 0 };
        }
    }

    // White border — use saturating_add to prevent overflow on large images
    let (gw, gh) = gray.dimensions();
    let padded_w = gw.saturating_add(BORDER_PAD * 2);
    let padded_h = gh.saturating_add(BORDER_PAD * 2);
    let mut padded = GrayImage::from_pixel(padded_w, padded_h, Luma([255u8]));
    image::imageops::overlay(&mut padded, &gray, BORDER_PAD as i64, BORDER_PAD as i64);
    padded
}

fn encode_png(bytes: &[u8], w: u32, h: u32, color: image::ExtendedColorType) -> String {
    let mut png = Vec::new();
    if PngEncoder::new(&mut png).write_image(bytes, w, h, color).is_ok() {
        base64::engine::general_purpose::STANDARD.encode(&png)
    } else {
        String::new()
    }
}

fn build_lang(languages: &[String]) -> String {
    if languages.is_empty() {
        return "eng".to_string();
    }
    languages
        .iter()
        .map(|l| match l.trim() {
            "en" | "eng" => "eng",
            "de" | "deu" => "deu",
            "fr" | "fra" => "fra",
            "es" | "spa" => "spa",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+")
}
