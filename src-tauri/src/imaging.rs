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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameQualityKind {
    Good,
    FlatOrDark,
    StaticNoise,
    BlankTransparent,
}

#[derive(Debug, Clone, Copy)]
pub struct FrameQuality {
    pub kind: FrameQualityKind,
    pub score: f64,
}

fn calc_std(sum: f64, sum2: f64, n: f64) -> f64 {
    if n <= 0.0 {
        return 0.0;
    }
    let mean = sum / n;
    ((sum2 / n) - mean * mean).max(0.0).sqrt()
}

fn calc_pearson(n: u64, sx: f64, sy: f64, sx2: f64, sy2: f64, sxy: f64) -> f64 {
    if n == 0 {
        return 1.0;
    }
    let nf = n as f64;
    let mx = sx / nf;
    let my = sy / nf;
    let vx = (sx2 / nf - mx * mx).max(0.0);
    let vy = (sy2 / nf - my * my).max(0.0);
    let denom = (vx * vy).sqrt();
    if denom <= 1e-6 {
        return 1.0;
    }
    ((sxy / nf) - mx * my) / denom
}

/// Evaluates whether an extracted frame looks like real visual content versus blank, flat/black, or random static noise.
pub fn evaluate_frame_quality(rgba: &RgbaImage) -> FrameQuality {
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return FrameQuality {
            kind: FrameQualityKind::BlankTransparent,
            score: -1.0,
        };
    }

    let step = (w.max(h) / 160).max(1) as usize;
    let wu = w as usize;
    let hu = h as usize;
    let raw = rgba.as_raw();

    let mut total_samples: u64 = 0;
    let mut opaque_count: u64 = 0;
    let mut dark_count: u64 = 0;
    let mut bright_count: u64 = 0;

    let (mut sum_r, mut sum_r2) = (0.0f64, 0.0f64);
    let (mut sum_g, mut sum_g2) = (0.0f64, 0.0f64);
    let (mut sum_b, mut sum_b2) = (0.0f64, 0.0f64);
    let (mut sum_y, mut sum_y2) = (0.0f64, 0.0f64);

    let (mut nh, mut sum_h_diff, mut sxh, mut syh, mut sxh2, mut syh2, mut sxyh) =
        (0u64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (mut nv, mut sum_v_diff, mut sxv, mut syv, mut sxv2, mut syv2, mut sxyv) =
        (0u64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);

    for y in (0..hu).step_by(step) {
        for x in (0..wu).step_by(step) {
            total_samples += 1;
            let off = (y * wu + x) * 4;
            let a = raw[off + 3];
            if a < 16 {
                continue;
            }
            opaque_count += 1;

            let r = raw[off] as f64;
            let g = raw[off + 1] as f64;
            let b = raw[off + 2] as f64;
            let lum = 0.299 * r + 0.587 * g + 0.114 * b;

            sum_r += r;
            sum_r2 += r * r;
            sum_g += g;
            sum_g2 += g * g;
            sum_b += b;
            sum_b2 += b * b;
            sum_y += lum;
            sum_y2 += lum * lum;

            if lum < 14.0 {
                dark_count += 1;
            }
            if lum > 242.0 {
                bright_count += 1;
            }

            if x + 1 < wu {
                let off_h = (y * wu + (x + 1)) * 4;
                if raw[off_h + 3] >= 16 {
                    let r2 = raw[off_h] as f64;
                    let g2 = raw[off_h + 1] as f64;
                    let b2 = raw[off_h + 2] as f64;
                    let lum2 = 0.299 * r2 + 0.587 * g2 + 0.114 * b2;
                    sum_h_diff += ((r - r2).abs() + (g - g2).abs() + (b - b2).abs()) / 3.0;
                    sxh += lum;
                    syh += lum2;
                    sxh2 += lum * lum;
                    syh2 += lum2 * lum2;
                    sxyh += lum * lum2;
                    nh += 1;
                }
            }

            if y + 1 < hu {
                let off_v = ((y + 1) * wu + x) * 4;
                if raw[off_v + 3] >= 16 {
                    let r2 = raw[off_v] as f64;
                    let g2 = raw[off_v + 1] as f64;
                    let b2 = raw[off_v + 2] as f64;
                    let lum2 = 0.299 * r2 + 0.587 * g2 + 0.114 * b2;
                    sum_v_diff += ((r - r2).abs() + (g - g2).abs() + (b - b2).abs()) / 3.0;
                    sxv += lum;
                    syv += lum2;
                    sxv2 += lum * lum;
                    syv2 += lum2 * lum2;
                    sxyv += lum * lum2;
                    nv += 1;
                }
            }
        }
    }

    if total_samples == 0 || opaque_count == 0 || (opaque_count as f64 / total_samples as f64) < 0.05
    {
        return FrameQuality {
            kind: FrameQualityKind::BlankTransparent,
            score: -1.0,
        };
    }

    let nf = opaque_count as f64;
    let std_y = calc_std(sum_y, sum_y2, nf);
    let color_spread =
        calc_std(sum_r, sum_r2, nf) + calc_std(sum_g, sum_g2, nf) + calc_std(sum_b, sum_b2, nf);
    let dark_ratio = dark_count as f64 / nf;
    let bright_ratio = bright_count as f64 / nf;

    let corr_h = calc_pearson(nh, sxh, syh, sxh2, syh2, sxyh);
    let corr_v = calc_pearson(nv, sxv, syv, sxv2, syv2, sxyv);
    let mean_h_diff = if nh > 0 { sum_h_diff / nh as f64 } else { 0.0 };
    let mean_v_diff = if nv > 0 { sum_v_diff / nv as f64 } else { 0.0 };

    if (std_y < 7.0 && color_spread < 18.0)
        || (dark_ratio > 0.96 && std_y < 16.0)
        || (bright_ratio > 0.96 && std_y < 16.0)
    {
        return FrameQuality {
            kind: FrameQualityKind::FlatOrDark,
            score: std_y * 0.5 + color_spread * 0.1,
        };
    }

    if std_y >= 10.0
        && corr_h < 0.35
        && corr_v < 0.35
        && (mean_h_diff / std_y) > 0.78
        && (mean_v_diff / std_y) > 0.78
    {
        return FrameQuality {
            kind: FrameQualityKind::StaticNoise,
            score: 5.0 + (corr_h.max(0.0) + corr_v.max(0.0)) * 10.0,
        };
    }

    FrameQuality {
        kind: FrameQualityKind::Good,
        score: 100.0 + std_y.min(80.0) + (color_spread * 0.2).min(40.0) - (dark_ratio * 30.0),
    }
}

pub fn encode_webp_thumbnail_with_quality(
    img: &DynamicImage,
) -> Result<(GeneratedThumbnail, FrameQuality), VaultError> {
    let resized = if img.width() > THUMB_MAX_EDGE || img.height() > THUMB_MAX_EDGE {
        img.resize(THUMB_MAX_EDGE, THUMB_MAX_EDGE, FilterType::Lanczos3)
    } else {
        img.clone()
    };

    let rgba = resized.to_rgba8();
    let tw = rgba.width();
    let th = rgba.height();
    let quality = evaluate_frame_quality(&rgba);
    let palette = extract_palette(&rgba, 8);

    let encoder = webp::Encoder::from_rgba(rgba.as_raw(), tw, th);
    let webp_mem = encoder.encode(THUMB_WEBP_QUALITY);

    Ok((
        GeneratedThumbnail {
            data: webp_mem.to_vec(),
            width: tw as i32,
            height: th as i32,
            palette,
        },
        quality,
    ))
}

pub fn encode_webp_thumbnail(img: &DynamicImage) -> Result<GeneratedThumbnail, VaultError> {
    let (thumb, _) = encode_webp_thumbnail_with_quality(img)?;
    Ok(thumb)
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn test_evaluate_frame_quality_cases() {
        // 1. Blank transparent
        let blank = RgbaImage::from_pixel(120, 90, Rgba([0, 0, 0, 0]));
        assert_eq!(
            evaluate_frame_quality(&blank).kind,
            FrameQualityKind::BlankTransparent
        );

        // 2. Flat/dark black frame
        let black = RgbaImage::from_pixel(120, 90, Rgba([5, 5, 5, 255]));
        assert_eq!(
            evaluate_frame_quality(&black).kind,
            FrameQualityKind::FlatOrDark
        );

        // 3. Static random RGB noise
        let mut noise = RgbaImage::new(160, 120);
        let mut seed: u64 = 0x1234_5678_9abc_def0;
        for px in noise.pixels_mut() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let r = ((seed >> 16) & 0xff) as u8;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let g = ((seed >> 16) & 0xff) as u8;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let b = ((seed >> 16) & 0xff) as u8;
            *px = Rgba([r, g, b, 255]);
        }
        assert_eq!(
            evaluate_frame_quality(&noise).kind,
            FrameQualityKind::StaticNoise
        );

        // 4. Smooth gradient normal image
        let mut grad = RgbaImage::new(160, 120);
        for (x, y, px) in grad.enumerate_pixels_mut() {
            let r = ((x * 255) / 160) as u8;
            let g = ((y * 255) / 120) as u8;
            let b = (((x + y) * 128) / 280) as u8;
            *px = Rgba([r, g, b, 255]);
        }
        let gq = evaluate_frame_quality(&grad);
        assert_eq!(gq.kind, FrameQualityKind::Good);
        assert!(gq.score > 100.0);
    }
}
