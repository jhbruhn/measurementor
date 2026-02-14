use base64::Engine;
use image::{
    codecs::png::PngEncoder, DynamicImage, GenericImageView, GrayImage, ImageEncoder, Luma,
};
use imageproc::{
    distance_transform::Norm,
    filter::gaussian_blur_f32,
    morphology::open as morph_open,
};
use rayon::prelude::*;
use kreuzberg_tesseract::TesseractAPI;

use super::{OcrResult, Recognizer};

// ── Constants ─────────────────────────────────────────────────────────────

const PSMS: &[u32] = &[7, 6, 8, 13];

/// Upscale factor applied before processing to improve recognition on small crops.
const UPSCALE_FACTOR: u32 = 6;
/// Mean pixel brightness below which the image is assumed dark-bg and auto-inverted.
const DARK_BG_THRESHOLD: u64 = 140;
/// White border added around the processed image to help Tesseract find the text block.
const BORDER_PAD: u32 = 15;

/// CLAHE tile size in pixels (on the upscaled image; ~5 original pixels at 6×).
const CLAHE_TILE_SIZE: u32 = 32;
/// CLAHE clip limit multiplier (ratio to average bin count; 2.0 = clip at 2× average).
const CLAHE_CLIP_LIMIT: f32 = 2.0;

/// Sauvola window diameter in pixels (must be odd; half-radius = (W-1)/2).
const SAUVOLA_WINDOW: u32 = 25;
/// Sauvola k: sensitivity to local standard deviation. Typical range 0.2–0.5.
const SAUVOLA_K: f64 = 0.34;
/// Sauvola R: dynamic range of std deviation (128.0 for 8-bit images).
const SAUVOLA_R: f64 = 128.0;

/// Gamma exponent applied when image is underexposed after inversion (γ < 1 brightens).
const GAMMA_VALUE: f64 = 0.5;
/// Mean brightness threshold; gamma is applied only when image mean falls below this.
const GAMMA_DARK_THRESHOLD: u64 = 80;

// ── Preprocess enum ───────────────────────────────────────────────────────

/// How to prepare the crop before handing it to Tesseract.
pub enum Preprocess {
    /// Full pipeline: upscale → luma → invert → gamma → CLAHE → blur → Sauvola → morph-open.
    Binary,
    /// Same pipeline as `Binary` but stops at enhanced grayscale (no binarization).
    Gray,
    /// Raw luma only — minimal processing, fast baseline.
    RawGray,
    /// Raw RGB bytes (bpp=3) passed directly to Tesseract with no grayscale conversion.
    RawRgb,
    /// Extract the red channel, then apply the full Binary pipeline.
    ChannelR,
    /// Extract the green channel, then apply the full Binary pipeline.
    ChannelG,
    /// Extract the blue channel, then apply the full Binary pipeline.
    ChannelB,
}

// ── TesseractRecognizer ───────────────────────────────────────────────────

pub struct TesseractRecognizer {
    pub languages: Vec<String>,
    pub preprocess: Preprocess,
    /// Directory that directly contains `<lang>.traineddata` files
    /// (passed straight to kreuzberg-tesseract `init()`).
    /// `None` → Tesseract uses TESSDATA_PREFIX or the system default.
    pub tessdata_dir: Option<String>,
}

impl Recognizer for TesseractRecognizer {
    fn name(&self) -> &str {
        match self.preprocess {
            Preprocess::Binary   => "tesseract/binary",
            Preprocess::Gray     => "tesseract/gray",
            Preprocess::RawGray  => "tesseract/raw-gray",
            Preprocess::RawRgb   => "tesseract/rgb",
            Preprocess::ChannelR => "tesseract/channel-r",
            Preprocess::ChannelG => "tesseract/channel-g",
            Preprocess::ChannelB => "tesseract/channel-b",
        }
    }

    fn recognize(&self, crop: &DynamicImage) -> Option<OcrResult> {
        let lang = build_lang(&self.languages);
        let datadir = self.tessdata_dir.as_deref();
        match self.preprocess {
            Preprocess::RawRgb => {
                let img = crop.to_rgb8();
                run_ocr_bytes(img.as_raw(), img.width(), img.height(), 3, &lang, datadir, self.name())
            }
            _ => {
                let img = self.prepare_gray(crop);
                run_ocr_bytes(img.as_raw(), img.width(), img.height(), 1, &lang, datadir, self.name())
            }
        }
    }
}

impl TesseractRecognizer {
    fn prepare_gray(&self, crop: &DynamicImage) -> GrayImage {
        match self.preprocess {
            Preprocess::RawGray => crop.to_luma8(),
            Preprocess::RawRgb  => unreachable!(),
            _                   => preprocess_pipeline(crop, &self.preprocess),
        }
    }
}

// ── Full preprocessing pipeline ───────────────────────────────────────────
//
// Applied for Binary, Gray, and Channel* modes.
//
// Steps:
//   1. Lanczos 6× upscale
//   2. Grayscale (or single RGB-channel extraction for Channel* modes)
//   3. Auto-invert if background is dark (ensures text is always dark on bright bg)
//   4. Gamma correction if image is still underexposed after inversion
//   5. CLAHE — adaptive local contrast enhancement (replaces global histogram EQ)
//   6. σ=1 Gaussian blur — light smoothing to remove CLAHE quantization artifacts
//   7. Sauvola adaptive binarization (Binary / Channel* only)
//   8. Morphological opening — fills small white gaps in dark character strokes
//   9. White border padding for Tesseract

fn preprocess_pipeline(img: &DynamicImage, mode: &Preprocess) -> GrayImage {
    let (w, h) = img.dimensions();
    let scaled = img.resize(
        w * UPSCALE_FACTOR,
        h * UPSCALE_FACTOR,
        image::imageops::FilterType::Lanczos3,
    );

    // Step 1 — Grayscale or single-channel extraction
    let mut gray: GrayImage = match mode {
        Preprocess::ChannelR => extract_channel(&scaled, 0),
        Preprocess::ChannelG => extract_channel(&scaled, 1),
        Preprocess::ChannelB => extract_channel(&scaled, 2),
        _                    => scaled.to_luma8(),
    };

    // Step 2 — Auto-invert: ensure text is dark on bright background (Tesseract & Sauvola expect this)
    let px_count = (gray.width() * gray.height()).max(1) as u64;
    let mean: u64 = gray.pixels().map(|p| p[0] as u64).sum::<u64>() / px_count;
    if mean < DARK_BG_THRESHOLD {
        for p in gray.pixels_mut() {
            p[0] = 255 - p[0];
        }
    }

    // Step 3 — Gamma correction for underexposed images (brightens shadows via power curve)
    let mean2: u64 = gray.pixels().map(|p| p[0] as u64).sum::<u64>() / px_count;
    if mean2 < GAMMA_DARK_THRESHOLD {
        let lut: Vec<u8> = (0u16..=255)
            .map(|i| ((i as f64 / 255.0).powf(GAMMA_VALUE) * 255.0).round() as u8)
            .collect();
        for p in gray.pixels_mut() {
            p[0] = lut[p[0] as usize];
        }
    }

    // Step 4 — CLAHE: adaptive local contrast enhancement, tile-by-tile with clip limit
    gray = clahe(&gray, CLAHE_TILE_SIZE, CLAHE_CLIP_LIMIT);

    // Step 5 — Light Gaussian blur (removes quantization noise introduced by CLAHE)
    gray = gaussian_blur_f32(&gray, 1.0);

    // Step 6 — Binarize (Binary / Channel*) or stop at enhanced grayscale (Gray)
    let binarize = matches!(
        mode,
        Preprocess::Binary | Preprocess::ChannelR | Preprocess::ChannelG | Preprocess::ChannelB
    );
    if binarize {
        // Sauvola adaptive threshold — adapts to local mean and std deviation
        gray = sauvola_threshold(&gray);
        // Morphological opening — erosion then dilation of white; fills white holes inside
        // dark character strokes (e.g. reconnects broken segments of '8', '0', '1')
        gray = morph_open(&gray, Norm::LInf, 1);
    }

    // Step 7 — White border padding
    let (gw, gh) = gray.dimensions();
    let padded_w = gw.saturating_add(BORDER_PAD * 2);
    let padded_h = gh.saturating_add(BORDER_PAD * 2);
    let mut padded = GrayImage::from_pixel(padded_w, padded_h, Luma([255u8]));
    image::imageops::overlay(&mut padded, &gray, BORDER_PAD as i64, BORDER_PAD as i64);
    padded
}

/// Extract a single RGB channel (0=R, 1=G, 2=B) as a grayscale image.
fn extract_channel(img: &DynamicImage, channel: usize) -> GrayImage {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let mut gray = GrayImage::new(w, h);
    for (x, y, pixel) in rgb.enumerate_pixels() {
        gray.put_pixel(x, y, Luma([pixel[channel]]));
    }
    gray
}

// ── CLAHE ─────────────────────────────────────────────────────────────────
//
// Contrast-Limited Adaptive Histogram Equalization (Zuiderveld 1994).
//
// Algorithm:
//   1. Divide image into a grid of non-overlapping tiles.
//   2. Build each tile's 256-bin histogram.
//   3. Clip bins exceeding clip_limit_factor × (tile_area/256) and redistribute excess.
//   4. Compute CDF-based tone mapping for each tile.
//   5. Apply per-pixel mapping via bilinear interpolation between the four nearest tile centres.

fn clahe(img: &GrayImage, tile_size: u32, clip_limit_factor: f32) -> GrayImage {
    let width  = img.width();
    let height = img.height();
    let tiles_x = (width  + tile_size - 1) / tile_size;
    let tiles_y = (height + tile_size - 1) / tile_size;

    // Pre-compute tone mapping for every tile
    let maps: Vec<Vec<[u8; 256]>> = (0..tiles_y)
        .map(|ty| {
            (0..tiles_x)
                .map(|tx| tile_mapping(img, tx, ty, tile_size, clip_limit_factor))
                .collect()
        })
        .collect();

    // Apply bilinearly-interpolated mapping to every pixel
    let mut out = GrayImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let pixel = img.get_pixel(x, y)[0];
            let v = bilinear_clahe(&maps, pixel, x, y, tile_size, tiles_x, tiles_y);
            out.put_pixel(x, y, Luma([v]));
        }
    }
    out
}

/// Build the input → output tone mapping [0..255] for one CLAHE tile.
fn tile_mapping(
    img: &GrayImage,
    tx: u32, ty: u32,
    tile_size: u32,
    clip_limit_factor: f32,
) -> [u8; 256] {
    let w = img.width();
    let h = img.height();
    let x0 = tx * tile_size;
    let y0 = ty * tile_size;
    let x1 = ((tx + 1) * tile_size).min(w);
    let y1 = ((ty + 1) * tile_size).min(h);
    let tile_area = ((x1 - x0) * (y1 - y0)) as u64;

    // Histogram
    let mut hist = [0u64; 256];
    for py in y0..y1 {
        for px in x0..x1 {
            hist[img.get_pixel(px, py)[0] as usize] += 1;
        }
    }

    // Clip and redistribute excess uniformly
    let clip = ((tile_area as f32 / 256.0) * clip_limit_factor).max(1.0) as u64;
    let mut excess = 0u64;
    for h in hist.iter_mut() {
        if *h > clip {
            excess += *h - clip;
            *h = clip;
        }
    }
    let per_bin  = excess / 256;
    let leftover = (excess % 256) as usize;
    for (i, h) in hist.iter_mut().enumerate() {
        *h += per_bin + if i < leftover { 1 } else { 0 };
    }

    // CDF → normalised tone mapping
    let mut mapping = [0u8; 256];
    let mut cdf = 0u64;
    for (i, &h) in hist.iter().enumerate() {
        cdf += h;
        mapping[i] = ((cdf * 255) / tile_area).min(255) as u8;
    }
    mapping
}

/// Bilinear interpolation between the four tile mappings nearest to pixel (x, y).
fn bilinear_clahe(
    maps: &[Vec<[u8; 256]>],
    pixel: u8,
    x: u32, y: u32,
    tile_size: u32,
    tiles_x: u32,
    tiles_y: u32,
) -> u8 {
    let ts   = tile_size as f32;
    let half = ts / 2.0;

    // Fractional tile coordinates: 0.0 = centre of tile 0, 1.0 = centre of tile 1, …
    let tx_f = (x as f32 - half) / ts;
    let ty_f = (y as f32 - half) / ts;

    let tx0 = (tx_f.floor() as i32).clamp(0, tiles_x as i32 - 1) as usize;
    let ty0 = (ty_f.floor() as i32).clamp(0, tiles_y as i32 - 1) as usize;
    let tx1 = (tx0 + 1).min(tiles_x as usize - 1);
    let ty1 = (ty0 + 1).min(tiles_y as usize - 1);

    let fx = (tx_f - tx0 as f32).clamp(0.0, 1.0);
    let fy = (ty_f - ty0 as f32).clamp(0.0, 1.0);

    let v00 = maps[ty0][tx0][pixel as usize] as f32;
    let v10 = maps[ty0][tx1][pixel as usize] as f32;
    let v01 = maps[ty1][tx0][pixel as usize] as f32;
    let v11 = maps[ty1][tx1][pixel as usize] as f32;

    let top = v00 + (v10 - v00) * fx;
    let bot = v01 + (v11 - v01) * fx;
    (top + (bot - top) * fy).round() as u8
}

// ── Sauvola adaptive threshold ────────────────────────────────────────────
//
// T(x,y) = mean(x,y) × (1 + k × (σ(x,y)/R − 1))
//
// Uses summed-area tables (integral images) for O(1) per-pixel local statistics.
// With a 25×25 window and k=0.34, thresholds adapt to local background variations
// that a global threshold would miss (gradients, glare, uneven display backlighting).

fn sauvola_threshold(gray: &GrayImage) -> GrayImage {
    let w = gray.width()  as usize;
    let h = gray.height() as usize;
    let half = (SAUVOLA_WINDOW / 2) as usize;
    let stride = w + 1;

    // Integral images: row-major, (h+1) × (w+1), zero-padded top and left border
    let mut isum   = vec![0i64; stride * (h + 1)];
    let mut isumsq = vec![0i64; stride * (h + 1)];

    for y in 0..h {
        for x in 0..w {
            let v = gray.get_pixel(x as u32, y as u32)[0] as i64;
            let idx     = (y + 1) * stride + (x + 1);
            let above   = y * stride + (x + 1);
            let left    = (y + 1) * stride + x;
            let diag    = y * stride + x;
            isum  [idx] = v     + isum  [above] + isum  [left] - isum  [diag];
            isumsq[idx] = v * v + isumsq[above] + isumsq[left] - isumsq[diag];
        }
    }

    let mut out = GrayImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let x0 = x.saturating_sub(half);
            let y0 = y.saturating_sub(half);
            let x1 = (x + half + 1).min(w);
            let y1 = (y + half + 1).min(h);
            let count = ((x1 - x0) * (y1 - y0)) as i64;

            let br    = y1 * stride + x1;
            let bl    = y1 * stride + x0;
            let tr    = y0 * stride + x1;
            let tl    = y0 * stride + x0;
            let sum   = isum  [br] - isum  [bl] - isum  [tr] + isum  [tl];
            let sumsq = isumsq[br] - isumsq[bl] - isumsq[tr] + isumsq[tl];

            let mean = sum as f64 / count as f64;
            let var  = (sumsq as f64 / count as f64) - mean * mean;
            let std  = var.max(0.0).sqrt();

            let threshold = mean * (1.0 + SAUVOLA_K * (std / SAUVOLA_R - 1.0));
            let pv        = gray.get_pixel(x as u32, y as u32)[0] as f64;
            out.put_pixel(x as u32, y as u32, Luma([if pv >= threshold { 255 } else { 0 }]));
        }
    }
    out
}

// ── Tesseract OCR ─────────────────────────────────────────────────────────

/// Run all PSMs in parallel on raw bytes, return the best-confidence OcrResult.
/// `bpp` = bytes-per-pixel (1 for grayscale, 3 for RGB).
/// `datadir` is the parent directory of `tessdata/`; `None` → system default.
fn run_ocr_bytes(
    bytes: &[u8],
    w: u32,
    h: u32,
    bpp: i32,
    lang: &str,
    datadir: Option<&str>,
    engine_name: &str,
) -> Option<OcrResult> {
    let (text, confidence) = PSMS
        .par_iter()
        .filter_map(|&psm| try_ocr(bytes, w, h, lang, datadir, psm, bpp).ok())
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    let color = if bpp == 1 {
        image::ExtendedColorType::L8
    } else {
        image::ExtendedColorType::Rgb8
    };
    let preview_b64 = encode_png(bytes, w, h, color);

    Some(OcrResult { text, confidence, preview_b64, engine_name: engine_name.to_string() })
}

/// Call Tesseract for one PSM. `bpp` = 1 for grayscale, 3 for RGB.
/// `datadir` is the parent directory of the `tessdata/` folder; `None` → system default.
fn try_ocr(
    bytes: &[u8],
    w: u32,
    h: u32,
    lang: &str,
    datadir: Option<&str>,
    psm: u32,
    bpp: i32,
) -> Result<(String, f64), ()> {
    let mut api = TesseractAPI::new();
    api.init(datadir.unwrap_or(""), lang).map_err(|_| ())?;
    api.set_variable("tessedit_pageseg_mode", &psm.to_string()).map_err(|_| ())?;
    api.set_image(bytes, w as i32, h as i32, bpp, w as i32 * bpp).map_err(|_| ())?;

    let raw     = api.get_utf8_text().map_err(|_| ())?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        return Err(());
    }

    let conf = api.mean_text_conf().map_err(|_| ())?.max(0) as f64 / 100.0;
    Ok((trimmed, conf))
}

// ── Utilities ─────────────────────────────────────────────────────────────

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
            other        => other,
        })
        .collect::<Vec<_>>()
        .join("+")
}
