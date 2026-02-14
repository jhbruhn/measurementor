use base64::Engine;
use ffmpeg_the_third as ffmpeg;
use ffmpeg_the_third::{
    codec::context::Context as CodecCtx,
    format::Pixel,
    frame::Video as VideoFrame,
    media::Type,
    software::scaling::{context::Context as SwsCtx, flag::Flags},
};
use image::{codecs::png::PngEncoder, ImageEncoder};
use serde::Serialize;

#[derive(Serialize)]
pub struct VideoInfo {
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub total_frames: u64,
    pub duration: f64,
}

#[tauri::command]
pub fn get_video_info(path: String) -> Result<VideoInfo, String> {
    ffmpeg::init().map_err(|e| e.to_string())?;

    let ictx = ffmpeg::format::input(&path)
        .map_err(|e| format!("cannot open '{path}': {e}"))?;

    let stream = ictx
        .streams()
        .best(Type::Video)
        .ok_or_else(|| "no video stream found".to_string())?;

    let fps = {
        let r = stream.avg_frame_rate();
        if r.1 != 0 && r.0 > 0 {
            r.0 as f64 / r.1 as f64
        } else {
            eprintln!("warning: could not read FPS from '{path}', defaulting to 30");
            30.0
        }
    };

    let nb_frames = stream.frames();

    let (width, height) = {
        let ctx = CodecCtx::from_parameters(stream.parameters())
            .map_err(|e| format!("codec context: {e}"))?;
        let dec = ctx
            .decoder()
            .video()
            .map_err(|e| format!("video decoder: {e}"))?;
        (dec.width(), dec.height())
    };

    // Format-level duration is in AV_TIME_BASE units (microseconds)
    let duration = ictx.duration().max(0) as f64 / 1_000_000.0;

    let total_frames = if nb_frames > 0 {
        nb_frames as u64
    } else {
        (duration * fps).round() as u64
    };

    Ok(VideoInfo { fps, width, height, total_frames, duration })
}

/// Decode the frame nearest to `timestamp` seconds and return
/// `(rgb24_bytes, width, height)`.  The caller can use this for both
/// interactive preview (→ JPEG) and bulk extraction (→ raw RGB crop).
pub fn decode_frame_at(path: &str, timestamp: f64) -> Result<(Vec<u8>, u32, u32), String> {
    ffmpeg::init().map_err(|e| e.to_string())?;

    let mut ictx = ffmpeg::format::input(&path)
        .map_err(|e| format!("open '{path}': {e}"))?;

    // Collect stream index + build decoder inside a block so that the
    // shared borrow of `ictx` (held by `stream`) is released before we
    // call `ictx.seek()` which needs a mutable borrow.
    let (stream_idx, mut decoder) = {
        let stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| "no video stream".to_string())?;
        let idx = stream.index();
        let ctx = CodecCtx::from_parameters(stream.parameters())
            .map_err(|e| format!("codec context: {e}"))?;
        let dec = ctx
            .decoder()
            .video()
            .map_err(|e| format!("video decoder: {e}"))?;
        (idx, dec) // `stream` dropped here, releasing the shared borrow
    };

    let width = decoder.width();
    let height = decoder.height();

    // Seek to the requested time (AV_TIME_BASE = 1 000 000 µs / s).
    // `..seek_ts` passes max_ts = seek_ts to avformat_seek_file, which
    // lands at the nearest keyframe ≤ timestamp — same as `-ss` before `-i`.
    let seek_ts = (timestamp.max(0.0) * 1_000_000.0) as i64;
    ictx.seek(seek_ts, ..seek_ts)
        .map_err(|e| format!("seek to {timestamp:.3}s: {e}"))?;
    decoder.flush(); // clear decoder buffers after seek

    // Pixel-format converter: native format → RGB24
    let mut scaler = SwsCtx::get(
        decoder.format(),
        width,
        height,
        Pixel::RGB24,
        width,
        height,
        Flags::BILINEAR,
    )
    .map_err(|e| format!("scaler init: {e}"))?;

    let mut rgb_frame = VideoFrame::empty();
    let mut found = false;

    'outer: for (stream, packet) in ictx.packets().filter_map(|r| r.ok()) {
        if stream.index() != stream_idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        let mut decoded = VideoFrame::empty();
        while decoder.receive_frame(&mut decoded).is_ok() {
            scaler
                .run(&decoded, &mut rgb_frame)
                .map_err(|e| format!("pixel convert: {e}"))?;
            found = true;
            break 'outer;
        }
    }

    if !found {
        return Err(format!("no frame decoded at {timestamp:.3}s"));
    }

    // Copy RGB data, stripping per-row padding if the stride > row width.
    let stride    = rgb_frame.stride(0);
    let row_bytes = width as usize * 3;
    let data      = rgb_frame.data(0);

    let rgb = if stride == row_bytes {
        let expected = row_bytes * height as usize;
        if data.len() < expected {
            return Err(format!(
                "frame buffer too small: {} bytes < {} expected ({}×{}×3)",
                data.len(), expected, width, height
            ));
        }
        data[..expected].to_vec()
    } else {
        let mut flat = Vec::with_capacity(row_bytes * height as usize);
        for row in 0..height as usize {
            let start = row * stride;
            let end   = start + row_bytes;
            if end > data.len() {
                return Err(format!(
                    "frame row {row} out of bounds (stride={stride}, data.len()={})",
                    data.len()
                ));
            }
            flat.extend_from_slice(&data[start..end]);
        }
        flat
    };

    Ok((rgb, width, height))
}

/// Extract the frame at `timestamp` and return a base64-encoded lossless PNG.
#[tauri::command]
pub fn get_frame(path: String, timestamp: f64) -> Result<String, String> {
    let (rgb, width, height) = decode_frame_at(&path, timestamp)?;

    let mut png: Vec<u8> = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("PNG encode: {e}"))?;

    Ok(base64::engine::general_purpose::STANDARD.encode(&png))
}
