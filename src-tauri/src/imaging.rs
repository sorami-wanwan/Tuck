use std::collections::HashMap;
use std::io::Cursor;

use image::{imageops::FilterType, DynamicImage, ImageFormat, RgbaImage};

use crate::error::VaultError;

const THUMB_MAX_EDGE: u32 = 400;
const THUMB_WEBP_QUALITY: f32 = 82.0;

#[derive(Debug, Clone)]
pub struct GeneratedThumbnail {
    pub data: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub palette: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ProcessedImage {
    pub mime_type: &'static str,
    pub width: i32,
    pub height: i32,
    pub thumbnail: Option<GeneratedThumbnail>,
    pub palette: Vec<String>,
}

pub fn is_video_mime(mime: &str) -> bool {
    mime.starts_with("video/")
}

/// Inspects magic bytes so disguised or unsupported files are rejected safely.
pub fn detect_mime(bytes: &[u8]) -> Result<&'static str, VaultError> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Ok("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok("image/webp");
    }
    if is_avif(bytes) {
        return Ok("image/avif");
    }
    if is_quicktime(bytes) {
        return Ok("video/quicktime");
    }
    if is_mp4(bytes) {
        return Ok("video/mp4");
    }
    if is_webm(bytes) {
        return Ok("video/webm");
    }

    Err(VaultError::UnsupportedFormat(
        "PNG / JPEG / WebP / GIF / AVIF / MP4 / WebM / MOV のみ対応しています".to_string(),
    ))
}

fn ftyp_has_brand(bytes: &[u8], targets: &[&[u8; 4]]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let box_size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let end = if box_size >= 12 {
        box_size.min(bytes.len())
    } else {
        bytes.len().min(64)
    };

    let major = &bytes[8..12];
    if targets.iter().any(|t| major == *t) {
        return true;
    }
    if end <= 16 {
        return false;
    }
    bytes[16..end]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|brand| targets.contains(&brand))
}

fn is_avif(bytes: &[u8]) -> bool {
    ftyp_has_brand(bytes, &[b"avif", b"avis"])
}

fn is_quicktime(bytes: &[u8]) -> bool {
    ftyp_has_brand(bytes, &[b"qt  "])
}

fn is_mp4(bytes: &[u8]) -> bool {
    ftyp_has_brand(
        bytes,
        &[
            b"isom", b"iso2", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"avc1", b"M4V ",
            b"m4v ", b"f4v ", b"dash", b"msnv", b"av01", b"hev1", b"hvc1",
        ],
    )
}

fn is_webm(bytes: &[u8]) -> bool {
    if !bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return false;
    }
    let header_len = bytes.len().min(4096);
    bytes[..header_len].windows(4).any(|w| w == b"webm")
}

/// Extracts up to `max_colors` representative hex swatches in <1ms using quantized color buckets.
pub fn extract_palette(img: &RgbaImage, max_colors: usize) -> Vec<String> {
    if max_colors == 0 {
        return Vec::new();
    }

    let (w, h) = img.dimensions();
    let total_pixels = (w as usize).saturating_mul(h as usize);
    if total_pixels == 0 {
        return Vec::new();
    }

    let step = (total_pixels / 12_000).max(1);
    let raw = img.as_raw();
    let mut buckets: HashMap<u16, (u64, u64, u64, u64)> = HashMap::new();

    for idx in (0..total_pixels).step_by(step) {
        let off = idx * 4;
        let r = raw[off] as u64;
        let g = raw[off + 1] as u64;
        let b = raw[off + 2] as u64;
        let a = raw[off + 3];
        if a < 128 {
            continue;
        }
        let key = (((r >> 3) as u16) << 10) | (((g >> 3) as u16) << 5) | ((b >> 3) as u16);
        let entry = buckets.entry(key).or_insert((0, 0, 0, 0));
        entry.0 += 1;
        entry.1 += r;
        entry.2 += g;
        entry.3 += b;
    }

    if buckets.is_empty() {
        return Vec::new();
    }

    let mut ranked: Vec<(u64, u8, u8, u8)> = buckets
        .into_values()
        .map(|(count, sr, sg, sb)| {
            (
                count,
                (sr / count) as u8,
                (sg / count) as u8,
                (sb / count) as u8,
            )
        })
        .collect();
    ranked.sort_unstable_by_key(|b| std::cmp::Reverse(b.0));

    let mut picked: Vec<(u8, u8, u8)> = Vec::with_capacity(max_colors);
    for min_dist_sq in [42i32 * 42, 20 * 20, 0] {
        for &(_, r, g, b) in &ranked {
            if picked.len() >= max_colors {
                break;
            }
            let distinct = picked.iter().all(|&(pr, pg, pb)| {
                let dr = r as i32 - pr as i32;
                let dg = g as i32 - pg as i32;
                let db = b as i32 - pb as i32;
                dr * dr + dg * dg + db * db >= min_dist_sq
            });
            if distinct {
                picked.push((r, g, b));
            }
        }
        if picked.len() >= max_colors.min(ranked.len()) {
            break;
        }
    }

    picked
        .into_iter()
        .map(|(r, g, b)| format!("#{:02x}{:02x}{:02x}", r, g, b))
        .collect()
}

pub fn encode_webp_thumbnail(img: &DynamicImage) -> Result<GeneratedThumbnail, VaultError> {
    let resized = if img.width() > THUMB_MAX_EDGE || img.height() > THUMB_MAX_EDGE {
        img.resize(THUMB_MAX_EDGE, THUMB_MAX_EDGE, FilterType::Lanczos3)
    } else {
        img.clone()
    };

    let rgba = resized.to_rgba8();
    let tw = rgba.width();
    let th = rgba.height();
    let palette = extract_palette(&rgba, 8);

    let encoder = webp::Encoder::from_rgba(rgba.as_raw(), tw, th);
    let webp_mem = encoder.encode(THUMB_WEBP_QUALITY);

    Ok(GeneratedThumbnail {
        data: webp_mem.to_vec(),
        width: tw as i32,
        height: th as i32,
        palette,
    })
}

pub fn process_image_bytes(bytes: &[u8]) -> Result<ProcessedImage, VaultError> {
    let mime_type = detect_mime(bytes)?;
    if mime_type == "image/avif" || is_video_mime(mime_type) {
        return Ok(ProcessedImage {
            mime_type,
            width: 0,
            height: 0,
            thumbnail: None,
            palette: Vec::new(),
        });
    }

    let img =
        image::load_from_memory(bytes).map_err(|e| VaultError::CorruptedImage(e.to_string()))?;
    let width = img.width() as i32;
    let height = img.height() as i32;
    let thumb = encode_webp_thumbnail(&img)?;
    let palette = thumb.palette.clone();

    Ok(ProcessedImage {
        mime_type,
        width,
        height,
        thumbnail: Some(thumb),
        palette,
    })
}

pub fn process_client_png_thumbnail(png_bytes: &[u8]) -> Result<GeneratedThumbnail, VaultError> {
    let img = image::load_from_memory_with_format(png_bytes, ImageFormat::Png)
        .map_err(|e| VaultError::CorruptedImage(e.to_string()))?;
    encode_webp_thumbnail(&img)
}

pub fn rgba_to_png_bytes(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, VaultError> {
    let img = RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(|| {
        VaultError::CorruptedImage("不正なクリップボード画像サイズです".to_string())
    })?;
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| VaultError::CorruptedImage(e.to_string()))?;
    Ok(out)
}
