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

    fn recognize(&self, crop: DynamicImage) -> Option<OcrResult> {
        let lang = build_lang(&self.languages);
        match self.preprocess {
            Preprocess::RawRgb => run_rgb(crop.to_rgb8(), &lang, self.name()),
            _                  => run_gray(self.prepare_gray(crop), &lang, self.name()),
        }
    }
}

impl TesseractRecognizer {
    fn prepare_gray(&self, crop: DynamicImage) -> GrayImage {
        match self.preprocess {
            Preprocess::Binary  => preprocess_bw(crop, true),
            Preprocess::Gray    => preprocess_bw(crop, false),
            Preprocess::RawGray => crop.to_luma8(),
            Preprocess::RawRgb  => unreachable!(),
        }
    }
}

fn run_gray(img: GrayImage, lang: &str, name: &str) -> Option<OcrResult> {
    let (iw, ih) = (img.width(), img.height());
    let bytes = img.into_raw();
    let (text, confidence) = run_psms(&bytes, iw, ih, lang, 1)?;
    Some(OcrResult {
        text,
        confidence,
        preview_b64: encode_png(&bytes, iw, ih, image::ExtendedColorType::L8),
        engine_name: name.to_string(),
    })
}

fn run_rgb(img: RgbImage, lang: &str, name: &str) -> Option<OcrResult> {
    let (iw, ih) = (img.width(), img.height());
    let bytes = img.into_raw();
    let (text, confidence) = run_psms(&bytes, iw, ih, lang, 3)?;
    Some(OcrResult {
        text,
        confidence,
        preview_b64: encode_png(&bytes, iw, ih, image::ExtendedColorType::Rgb8),
        engine_name: name.to_string(),
    })
}

/// Run all `PSMS` in parallel and return the highest-confidence result.
fn run_psms(bytes: &[u8], w: u32, h: u32, lang: &str, bpp: i32) -> Option<(String, f64)> {
    PSMS.par_iter()
        .filter_map(|&psm| try_ocr(bytes, w, h, lang, psm, bpp).ok())
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
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
/// `binarize = true`  → add binary threshold at the end (sharper, fewer shades).
/// `binarize = false` → stop at enhanced grayscale (softer, more detail).
///
/// Pipeline: 6× Lanczos upscale → grayscale → auto-invert dark bg →
///           histogram equalisation → σ=1.0 Gaussian blur →
///           (optional binary threshold) → 15 px white border.
fn preprocess_bw(img: DynamicImage, binarize: bool) -> GrayImage {
    let (w, h) = img.dimensions();
    let scaled = img.resize(w * 6, h * 6, image::imageops::FilterType::Lanczos3);
    let mut gray = scaled.to_luma8();

    // Auto-invert if background is dark
    let mean: u64 = gray.pixels().map(|p| p[0] as u64).sum::<u64>()
        / (gray.width() * gray.height()).max(1) as u64;
    if mean < 140 {
        for p in gray.pixels_mut() {
            p[0] = 255 - p[0];
        }
    }

    gray = equalize_histogram(&gray);
    gray = gaussian_blur_f32(&gray, 1.0);

    if binarize {
        for p in gray.pixels_mut() {
            p[0] = if p[0] > 100 { 255 } else { 0 };
        }
    }

    // Add white border (improves Tesseract's text-block detection)
    let pad = 15u32;
    let (gw, gh) = gray.dimensions();
    let mut padded = GrayImage::from_pixel(gw + pad * 2, gh + pad * 2, Luma([255u8]));
    image::imageops::overlay(&mut padded, &gray, pad as i64, pad as i64);
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
