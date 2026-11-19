use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Cursor, Read};
use std::path::Path;

use image::{
    imageops::FilterType, metadata::Orientation, AnimationDecoder, DynamicImage, ImageDecoder,
    ImageFormat, ImageReader, Limits, RgbaImage,
};

use crate::error::VaultError;

pub const HEADER_SNIFF_BYTES: usize = 4096;
pub const MAX_IMAGE_FILE_BYTES: u64 = 512 * 1024 * 1024;

const THUMB_MAX_EDGE: u32 = 400;
const THUMB_WEBP_QUALITY: f32 = 82.0;
const DISPLAY_MAX_EDGE: u32 = 4096;
const DISPLAY_WEBP_QUALITY: f32 = 90.0;
const WEBP_MAX_DIM: u32 = 16_383;

const MAX_IMAGE_DIMENSION: u32 = 32_768;
const MAX_IMAGE_HEIGHT: u32 = 65_535;
const MAX_DECODE_PIXELS: u64 = 100_000_000;
const MAX_DECODE_ALLOC_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CLIENT_PNG_BYTES: usize = 32 * 1024 * 1024;
const MAX_CLIENT_PNG_DIMENSION: u32 = 8_192;
const MAX_CLIENT_PNG_PIXELS: u64 = 36_000_000;
const MAX_ANIMATION_FRAMES_TO_SCAN: usize = 8;

const AVIF_BRANDS: &[[u8; 4]] = &[*b"avif", *b"avis"];
const QT_BRANDS: &[[u8; 4]] = &[*b"qt  "];
const HEIC_BRANDS: &[[u8; 4]] = &[
    *b"heic", *b"heix", *b"hevc", *b"hevx", *b"heim", *b"heis", *b"mif1", *b"msf1",
];
const AUDIO_BMFF_BRANDS: &[[u8; 4]] = &[*b"M4A ", *b"M4B ", *b"M4P ", *b"f4a ", *b"f4b "];
const MP4_BRANDS: &[[u8; 4]] = &[
    *b"isom", *b"iso2", *b"iso4", *b"iso5", *b"iso6", *b"mp41", *b"mp42", *b"avc1", *b"M4V ",
    *b"m4v ", *b"f4v ", *b"dash", *b"msnv", *b"av01", *b"hev1", *b"hvc1", *b"avio", *b"3gp4",
    *b"3gp5", *b"3gp6", *b"mp71",
];

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

/// Reads up to `max_bytes` from the start of a file for header sniffing without buffering whole files.
pub fn read_file_head(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; max_bytes];
    let mut filled = 0usize;
    while filled < max_bytes {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

/// Inspects magic bytes and container headers so disguised or unsupported files are rejected safely.
pub fn detect_mime(bytes: &[u8]) -> Result<&'static str, VaultError> {
    if bytes.is_empty() {
        return Err(VaultError::CorruptedImage(
            "空のファイル（0 バイト）は取り込めません".to_string(),
        ));
    }
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
    if is_bmp(bytes) {
        return Ok("image/bmp");
    }
    if is_ico(bytes) {
        return Ok("image/x-icon");
    }
    if is_tiff(bytes) {
        return Ok("image/tiff");
    }
    if let Some(bmff_mime) = detect_bmff_mime(bytes)? {
        return Ok(bmff_mime);
    }
    if is_webm(bytes) {
        return Ok("video/webm");
    }

    Err(VaultError::UnsupportedFormat(
        "PNG / JPEG / WebP / GIF / AVIF / BMP / TIFF / ICO / MP4 / WebM / MOV のみ対応しています"
            .to_string(),
    ))
}

fn is_bmp(bytes: &[u8]) -> bool {
    if bytes.len() < 26 || &bytes[0..2] != b"BM" {
        return false;
    }
    let header_size = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
    match header_size {
        12 => {
            let w = u16::from_le_bytes([bytes[18], bytes[19]]);
            let h = u16::from_le_bytes([bytes[20], bytes[21]]);
            w > 0 && h > 0
        }
        40 | 52 | 56 | 64 | 108 | 124 => {
            let w = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
            let h = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
            w > 0 && h != 0 && h != i32::MIN
        }
        _ => false,
    }
}

fn is_ico(bytes: &[u8]) -> bool {
    if bytes.len() < 22 || bytes[0..4] != [0x00, 0x00, 0x01, 0x00] {
        return false;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]);
    if !(1..=256).contains(&count) {
        return false;
    }
    // ICONDIRENTRY: [width, height, color_count, reserved(0), planes(u16), bpp(u16), size(u32), offset(u32)]
    let reserved = bytes[9];
    let bpp = u16::from_le_bytes([bytes[12], bytes[13]]);
    let img_size = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
    let img_offset = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let min_offset = 6u32.saturating_add(u32::from(count).saturating_mul(16));
    reserved == 0 && bpp <= 32 && img_size > 0 && img_offset >= min_offset
}

fn is_tiff(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    let ifd_offset = if bytes.starts_with(&[0x49, 0x49, 0x2A, 0x00]) {
        // Canon CR2 uses II*\0 + CR\x02\x00 at offset 8; exclude RAW variants
        if bytes.len() >= 10 && &bytes[8..10] == b"CR" {
            return false;
        }
        u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]])
    } else if bytes.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]])
    } else {
        return false;
    };
    ifd_offset >= 8
}

#[derive(Debug, Clone, Copy)]
enum BmffBoxParse {
    Valid {
        fourcc: [u8; 4],
        header_len: usize,
        box_len: Option<u64>,
    },
    NeedMoreBytes,
    Invalid,
}

fn parse_bmff_box_at(bytes: &[u8], offset: usize) -> BmffBoxParse {
    if offset.saturating_add(8) > bytes.len() {
        return BmffBoxParse::NeedMoreBytes;
    }
    let size32 = u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]);
    let fourcc = [
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ];
    match size32 {
        0 => BmffBoxParse::Valid {
            fourcc,
            header_len: 8,
            box_len: None,
        },
        1 => {
            if offset.saturating_add(16) > bytes.len() {
                return BmffBoxParse::NeedMoreBytes;
            }
            let size64 = u64::from_be_bytes([
                bytes[offset + 8],
                bytes[offset + 9],
                bytes[offset + 10],
                bytes[offset + 11],
                bytes[offset + 12],
                bytes[offset + 13],
                bytes[offset + 14],
                bytes[offset + 15],
            ]);
            if size64 < 16 {
                return BmffBoxParse::Invalid;
            }
            BmffBoxParse::Valid {
                fourcc,
                header_len: 16,
                box_len: Some(size64),
            }
        }
        2..=7 => BmffBoxParse::Invalid,
        n => BmffBoxParse::Valid {
            fourcc,
            header_len: 8,
            box_len: Some(u64::from(n)),
        },
    }
}

fn is_skippable_leading_bmff_box(fourcc: [u8; 4]) -> bool {
    matches!(&fourcc, b"wide" | b"free" | b"skip" | b"pdin")
}

fn has_valid_moov_child(bytes: &[u8]) -> bool {
    let mut off = 0usize;
    let mut steps = 0usize;
    while off + 8 <= bytes.len() && steps < 16 {
        steps += 1;
        match parse_bmff_box_at(bytes, off) {
            BmffBoxParse::Valid {
                fourcc,
                box_len,
                ..
            } => {
                if matches!(&fourcc, b"mvhd" | b"trak" | b"cmov" | b"udta" | b"iods") {
                    return true;
                }
                let Some(len) = box_len else {
                    break;
                };
                let Ok(step) = usize::try_from(len) else {
                    break;
                };
                let Some(next) = off.checked_add(step) else {
                    break;
                };
                off = next;
            }
            BmffBoxParse::NeedMoreBytes | BmffBoxParse::Invalid => break,
        }
    }
    false
}

fn classify_ftyp_box(payload: &[u8]) -> Result<Option<&'static str>, VaultError> {
    if payload.len() < 4 {
        return Ok(None);
    }
    let major: [u8; 4] = [payload[0], payload[1], payload[2], payload[3]];
    let compat_bytes = payload.get(8..).unwrap_or(&[]);
    let (compat_chunks, _) = compat_bytes.as_chunks::<4>();
    let has_compat =
        |targets: &[[u8; 4]]| -> bool { compat_chunks.iter().any(|chunk| targets.contains(chunk)) };

    if AVIF_BRANDS.contains(&major) || has_compat(AVIF_BRANDS) {
        return Ok(Some("image/avif"));
    }
    if AUDIO_BMFF_BRANDS.contains(&major) {
        return Err(VaultError::UnsupportedFormat(
            "音声専用ファイル（M4A 等）は未対応です".to_string(),
        ));
    }
    if HEIC_BRANDS.contains(&major) {
        return Err(VaultError::UnsupportedFormat(
            "HEIC / HEIF 画像は現在未対応です（PNG / JPEG / WebP / AVIF 等に変換してください）"
                .to_string(),
        ));
    }
    if QT_BRANDS.contains(&major) || has_compat(QT_BRANDS) {
        return Ok(Some("video/quicktime"));
    }
    if MP4_BRANDS.contains(&major) || has_compat(MP4_BRANDS) {
        return Ok(Some("video/mp4"));
    }
    Ok(None)
}

fn detect_bmff_mime(bytes: &[u8]) -> Result<Option<&'static str>, VaultError> {
    let mut off = 0usize;
    let mut seen_boxes = 0usize;
    while off < bytes.len().min(HEADER_SNIFF_BYTES) && seen_boxes < 8 {
        seen_boxes += 1;
        match parse_bmff_box_at(bytes, off) {
            BmffBoxParse::Valid {
                fourcc,
                header_len,
                box_len,
            } => {
                if &fourcc == b"ftyp" {
                    let payload_start = off + header_len;
                    let end = match box_len {
                        Some(len) => off
                            .saturating_add(usize::try_from(len).unwrap_or(usize::MAX))
                            .min(bytes.len()),
                        None => bytes.len(),
                    };
                    if payload_start >= end {
                        return Ok(None);
                    }
                    return classify_ftyp_box(&bytes[payload_start..end]);
                }
                // Legacy QuickTime MOV without ftyp starts directly with wide/free/moov
                if &fourcc == b"moov" {
                    let payload_start = off + header_len;
                    let end = match box_len {
                        Some(len) => off
                            .saturating_add(usize::try_from(len).unwrap_or(usize::MAX))
                            .min(bytes.len()),
                        None => bytes.len(),
                    };
                    if payload_start < end && has_valid_moov_child(&bytes[payload_start..end]) {
                        return Ok(Some("video/quicktime"));
                    }
                    return Ok(None);
                }
                if !is_skippable_leading_bmff_box(fourcc) {
                    return Ok(None);
                }
                let Some(len) = box_len else {
                    return Ok(None);
                };
                let Ok(step) = usize::try_from(len) else {
                    return Ok(None);
                };
                let Some(next) = off.checked_add(step) else {
                    return Ok(None);
                };
                off = next;
            }
            BmffBoxParse::NeedMoreBytes | BmffBoxParse::Invalid => return Ok(None),
        }
    }
    Ok(None)
}

fn read_ebml_vint_id(slice: &[u8]) -> Option<(u32, usize)> {
    let first = *slice.first()?;
    if first == 0 {
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    if !(1..=4).contains(&len) || slice.len() < len {
        return None;
    }
    let mut id = 0u32;
    for &b in &slice[..len] {
        id = (id << 8) | u32::from(b);
    }
    Some((id, len))
}

fn read_ebml_vint_size(slice: &[u8]) -> Option<(Option<u64>, usize)> {
    let first = *slice.first()?;
    if first == 0 {
        return None;
    }
    let lz = first.leading_zeros() as usize;
    let len = lz + 1;
    if !(1..=8).contains(&len) || slice.len() < len {
        return None;
    }
    let mask = if len == 8 { 0 } else { (1u8 << (8 - len)) - 1 };
    let mut val = u64::from(first & mask);
    let mut all_ones = (first & mask) == mask;
    for &b in &slice[1..len] {
        if b != 0xFF {
            all_ones = false;
        }
        val = (val << 8) | u64::from(b);
    }
    if all_ones {
        Some((None, len))
    } else {
        Some((Some(val), len))
    }
}

fn parse_ebml_header(bytes: &[u8]) -> Option<(Option<u64>, bool)> {
    if !bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return None;
    }
    let (hdr_size_opt, size_len) = read_ebml_vint_size(&bytes[4..])?;
    let payload_start = 4 + size_len;
    let header_end = match hdr_size_opt {
        Some(len) => u64::try_from(payload_start).ok()?.checked_add(len),
        None => None,
    };
    let scan_end = match header_end {
        Some(end) => usize::try_from(end).unwrap_or(usize::MAX).min(bytes.len()),
        None => bytes.len(),
    }
    .min(HEADER_SNIFF_BYTES);

    let mut pos = payload_start;
    let mut found_webm_doctype = false;
    while pos < scan_end {
        let (elem_id, id_len) = match read_ebml_vint_id(&bytes[pos..scan_end]) {
            Some(v) => v,
            None => break,
        };
        pos += id_len;
        let (elem_size_opt, sz_len) = match read_ebml_vint_size(&bytes[pos..scan_end]) {
            Some(v) => v,
            None => break,
        };
        pos += sz_len;
        let Some(elem_size) = elem_size_opt else {
            break;
        };
        let Ok(elem_size_usize) = usize::try_from(elem_size) else {
            break;
        };
        let Some(next) = pos.checked_add(elem_size_usize) else {
            break;
        };
        if next > scan_end {
            break;
        }
        if elem_id == 0x4282 {
            let mut doc_type = &bytes[pos..next];
            while doc_type.last() == Some(&0) {
                doc_type = &doc_type[..doc_type.len() - 1];
            }
            found_webm_doctype = doc_type == b"webm";
            break;
        }
        pos = next;
    }

    Some((header_end, found_webm_doctype))
}

fn is_webm(bytes: &[u8]) -> bool {
    matches!(parse_ebml_header(bytes), Some((_, true)))
}

/// Validates container framing to reject truncated/header-only stubs early before full buffering.
pub fn validate_media_container_prefix(
    mime_type: &str,
    head_bytes: &[u8],
    total_file_len: u64,
) -> Result<(), VaultError> {
    if total_file_len == 0 {
        return Err(VaultError::CorruptedImage(
            "空のファイル（0 バイト）は取り込めません".to_string(),
        ));
    }

    match mime_type {
        "image/avif" | "video/mp4" | "video/quicktime" => {
            if total_file_len < 24 {
                return Err(VaultError::CorruptedImage(
                    "コンテナヘッダが不完全または破損しています".to_string(),
                ));
            }
            let mut off = 0usize;
            let mut saw_ftyp_or_moov = false;
            let mut saw_meta = false;
            let mut steps = 0usize;
            while off < head_bytes.len() && steps < 64 {
                steps += 1;
                match parse_bmff_box_at(head_bytes, off) {
                    BmffBoxParse::Invalid => {
                        return Err(VaultError::CorruptedImage(
                            "コンテナボックス構造が破損しています".to_string(),
                        ));
                    }
                    BmffBoxParse::NeedMoreBytes => break,
                    BmffBoxParse::Valid {
                        fourcc, box_len, ..
                    } => {
                        if &fourcc == b"meta" {
                            saw_meta = true;
                        }
                        if &fourcc == b"ftyp" || &fourcc == b"moov" {
                            saw_ftyp_or_moov = true;
                        }
                        let Some(len) = box_len else {
                            break;
                        };
                        let box_end = u64::try_from(off)
                            .unwrap_or(u64::MAX)
                            .saturating_add(len);
                        if (&fourcc == b"ftyp" || &fourcc == b"meta") && box_end > total_file_len {
                            return Err(VaultError::CorruptedImage(
                                "コンテナファイルが途中で切れています（破損）".to_string(),
                            ));
                        }
                        if &fourcc == b"ftyp" && box_end >= total_file_len {
                            return Err(VaultError::CorruptedImage(
                                "本体データが含まれていない不完全なコンテナファイルです"
                                    .to_string(),
                            ));
                        }
                        let Ok(step) = usize::try_from(len) else {
                            break;
                        };
                        let Some(next) = off.checked_add(step) else {
                            break;
                        };
                        off = next;
                    }
                }
            }
            if !saw_ftyp_or_moov {
                return Err(VaultError::CorruptedImage(
                    "有効なコンテナヘッダが見つかりません".to_string(),
                ));
            }
            if mime_type == "image/avif"
                && (head_bytes.len() as u64) >= total_file_len
                && !saw_meta
            {
                return Err(VaultError::CorruptedImage(
                    "AVIF ファイルに必須の meta ボックスが存在しません".to_string(),
                ));
            }
        }
        "video/webm" => {
            let Some((ebml_end_opt, true)) = parse_ebml_header(head_bytes) else {
                return Err(VaultError::CorruptedImage(
                    "WebM ヘッダが破損しています".to_string(),
                ));
            };
            if let Some(ebml_end) = ebml_end_opt {
                if total_file_len <= ebml_end.saturating_add(12) {
                    return Err(VaultError::CorruptedImage(
                        "本体データが含まれていない不完全な WebM ファイルです".to_string(),
                    ));
                }
            } else if total_file_len < 32 {
                return Err(VaultError::CorruptedImage(
                    "不完全な WebM ファイルです".to_string(),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum AvifProp {
    Ispe(u32, u32),
    Irot(u8),
    Other,
}

fn find_bmff_child_range<'a>(buf: &'a [u8], target: &[u8; 4]) -> Option<&'a [u8]> {
    let mut off = 0usize;
    let mut steps = 0usize;
    while off.saturating_add(8) <= buf.len() && steps < 128 {
        steps += 1;
        match parse_bmff_box_at(buf, off) {
            BmffBoxParse::Valid {
                fourcc,
                header_len,
                box_len,
            } => {
                let end = match box_len {
                    Some(len) => off
                        .saturating_add(usize::try_from(len).unwrap_or(usize::MAX))
                        .min(buf.len()),
                    None => buf.len(),
                };
                if &fourcc == target {
                    return buf.get(off + header_len..end);
                }
                let Some(len) = box_len else {
                    break;
                };
                let Ok(step) = usize::try_from(len) else {
                    break;
                };
                let Some(next) = off.checked_add(step) else {
                    break;
                };
                off = next;
            }
            BmffBoxParse::NeedMoreBytes | BmffBoxParse::Invalid => break,
        }
    }
    None
}

fn parse_ipco_properties(ipco_payload: &[u8]) -> Vec<AvifProp> {
    let mut props = Vec::new();
    let mut off = 0usize;
    while off.saturating_add(8) <= ipco_payload.len() && props.len() < 256 {
        match parse_bmff_box_at(ipco_payload, off) {
            BmffBoxParse::Valid {
                fourcc,
                header_len,
                box_len,
            } => {
                let end = match box_len {
                    Some(len) => off
                        .saturating_add(usize::try_from(len).unwrap_or(usize::MAX))
                        .min(ipco_payload.len()),
                    None => ipco_payload.len(),
                };
                let body = ipco_payload.get(off + header_len..end).unwrap_or(&[]);
                let prop = if &fourcc == b"ispe" && body.len() >= 12 {
                    let w = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                    let h = u32::from_be_bytes([body[8], body[9], body[10], body[11]]);
                    AvifProp::Ispe(w, h)
                } else if &fourcc == b"irot" && !body.is_empty() {
                    AvifProp::Irot(body[0] & 0x03)
                } else {
                    AvifProp::Other
                };
                props.push(prop);
                let Some(len) = box_len else {
                    break;
                };
                let Ok(step) = usize::try_from(len) else {
                    break;
                };
                let Some(next) = off.checked_add(step) else {
                    break;
                };
                off = next;
            }
            BmffBoxParse::NeedMoreBytes | BmffBoxParse::Invalid => break,
        }
    }
    props
}

fn parse_ipma_associations(ipma_payload: &[u8], target_item_id: u32) -> Option<Vec<usize>> {
    if ipma_payload.len() < 8 {
        return None;
    }
    let version = ipma_payload[0];
    let flags_lsb = ipma_payload[3];
    let long_prop_idx = (flags_lsb & 0x01) != 0;
    let entry_count = u32::from_be_bytes([
        ipma_payload[4],
        ipma_payload[5],
        ipma_payload[6],
        ipma_payload[7],
    ]);
    let mut pos = 8usize;
    for _ in 0..entry_count.min(4096) {
        let item_id = if version < 1 {
            if pos + 2 > ipma_payload.len() {
                return None;
            }
            let v = u16::from_be_bytes([ipma_payload[pos], ipma_payload[pos + 1]]);
            pos += 2;
            u32::from(v)
        } else {
            if pos + 4 > ipma_payload.len() {
                return None;
            }
            let v = u32::from_be_bytes([
                ipma_payload[pos],
                ipma_payload[pos + 1],
                ipma_payload[pos + 2],
                ipma_payload[pos + 3],
            ]);
            pos += 4;
            v
        };
        if pos >= ipma_payload.len() {
            return None;
        }
        let assoc_count = ipma_payload[pos] as usize;
        pos += 1;
        let mut indices = Vec::with_capacity(assoc_count.min(64));
        for _ in 0..assoc_count {
            let prop_idx = if long_prop_idx {
                if pos + 2 > ipma_payload.len() {
                    return None;
                }
                let raw = u16::from_be_bytes([ipma_payload[pos], ipma_payload[pos + 1]]);
                pos += 2;
                (raw & 0x7FFF) as usize
            } else {
                if pos >= ipma_payload.len() {
                    return None;
                }
                let raw = ipma_payload[pos];
                pos += 1;
                (raw & 0x7F) as usize
            };
            if prop_idx > 0 {
                indices.push(prop_idx - 1);
            }
        }
        if item_id == target_item_id {
            return Some(indices);
        }
    }
    None
}

/// Extracts nominal pixel dimensions (accounting for `ispe` + `irot`) from an AVIF container.
pub fn parse_avif_dimensions(bytes: &[u8]) -> Option<(i32, i32)> {
    let scan_slice = &bytes[..bytes.len().min(512 * 1024)];
    let meta_body = find_bmff_child_range(scan_slice, b"meta")?;
    // meta is a FullBox (4 bytes version+flags)
    let meta_children = meta_body.get(4..)?;

    let primary_id = find_bmff_child_range(meta_children, b"pitm").and_then(|pitm| {
        if pitm.len() >= 6 && pitm[0] == 0 {
            Some(u32::from(u16::from_be_bytes([pitm[4], pitm[5]])))
        } else if pitm.len() >= 8 && pitm[0] > 0 {
            Some(u32::from_be_bytes([pitm[4], pitm[5], pitm[6], pitm[7]]))
        } else {
            None
        }
    });

    let iprp_body = find_bmff_child_range(meta_children, b"iprp")?;
    let ipco_body = find_bmff_child_range(iprp_body, b"ipco")?;
    let props = parse_ipco_properties(ipco_body);
    if props.is_empty() {
        return None;
    }

    let mut chosen_ispe: Option<(u32, u32)> = None;
    let mut rotate_90_or_270 = false;

    if let (Some(pid), Some(ipma_body)) = (primary_id, find_bmff_child_range(iprp_body, b"ipma")) {
        if let Some(assoc) = parse_ipma_associations(ipma_body, pid) {
            for idx in assoc {
                if let Some(prop) = props.get(idx) {
                    match *prop {
                        AvifProp::Ispe(w, h) if w > 0 && h > 0 => chosen_ispe = Some((w, h)),
                        AvifProp::Irot(angle) if angle == 1 || angle == 3 => {
                            rotate_90_or_270 = !rotate_90_or_270;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    if chosen_ispe.is_none() {
        let mut best_area = 0u64;
        for prop in &props {
            match *prop {
                AvifProp::Ispe(w, h) if w > 0 && h > 0 => {
                    let area = u64::from(w).saturating_mul(u64::from(h));
                    if area > best_area {
                        best_area = area;
                        chosen_ispe = Some((w, h));
                    }
                }
                AvifProp::Irot(angle) if angle == 1 || angle == 3 => {
                    rotate_90_or_270 = true;
                }
                _ => {}
            }
        }
    }

    let (w, h) = chosen_ispe?;
    let (final_w, final_h) = if rotate_90_or_270 { (h, w) } else { (w, h) };
    if final_w == 0 || final_h == 0 || final_w > 65_535 || final_h > 65_535 {
        return None;
    }
    Some((final_w as i32, final_h as i32))
}

fn collect_palette_buckets(
    raw: &[u8],
    total_pixels: usize,
    step: usize,
    min_alpha: u8,
) -> HashMap<u16, (u64, u64, u64, u64)> {
    let mut buckets: HashMap<u16, (u64, u64, u64, u64)> = HashMap::new();
    for idx in (0..total_pixels).step_by(step) {
        let off = idx * 4;
        let a = raw[off + 3];
        if a < min_alpha {
            continue;
        }
        let r = raw[off] as u64;
        let g = raw[off + 1] as u64;
        let b = raw[off + 2] as u64;
        let key = (((r >> 3) as u16) << 10) | (((g >> 3) as u16) << 5) | ((b >> 3) as u16);
        let entry = buckets.entry(key).or_insert((0, 0, 0, 0));
        entry.0 += 1;
        entry.1 += r;
        entry.2 += g;
        entry.3 += b;
    }
    buckets
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
    let mut buckets = collect_palette_buckets(raw, total_pixels, step, 128);
    if buckets.is_empty() {
        buckets = collect_palette_buckets(raw, total_pixels, step, 16);
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

fn image_format_for_mime(mime_type: &str) -> Option<ImageFormat> {
    match mime_type {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        "image/bmp" | "image/x-ms-bmp" => Some(ImageFormat::Bmp),
        "image/tiff" => Some(ImageFormat::Tiff),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some(ImageFormat::Ico),
        _ => None,
    }
}

fn build_decode_limits(max_width: u32, max_height: u32, max_alloc: u64) -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_width);
    limits.max_image_height = Some(max_height);
    limits.max_alloc = Some(max_alloc);
    limits
}

fn validate_dimensions(
    w: u32,
    h: u32,
    max_width: u32,
    max_height: u32,
    max_pixels: u64,
) -> Result<(), VaultError> {
    if w == 0 || h == 0 {
        return Err(VaultError::CorruptedImage(
            "画像サイズが 0 です".to_string(),
        ));
    }
    let pixels = u64::from(w).saturating_mul(u64::from(h));
    if w > max_width || h > max_height || pixels > max_pixels {
        return Err(VaultError::CorruptedImage(format!(
            "画像解像度が安全上限を超えています（{w}x{h}）"
        )));
    }
    Ok(())
}

fn strip_exif_prefix(raw: &[u8]) -> &[u8] {
    raw.strip_prefix(b"Exif\0\0").unwrap_or(raw)
}

fn read_orientation_safe<D: ImageDecoder>(decoder: &mut D) -> Orientation {
    if let Ok(Some(raw)) = decoder.exif_metadata() {
        if let Some(orient) = Orientation::from_exif_chunk(strip_exif_prefix(&raw)) {
            return orient;
        }
    }
    decoder.orientation().unwrap_or(Orientation::NoTransforms)
}

fn pick_best_animation_frame<'a>(
    frames: image::Frames<'a>,
) -> Result<Option<RgbaImage>, VaultError> {
    let mut best: Option<(RgbaImage, FrameQuality)> = None;
    let mut saw_error: Option<VaultError> = None;

    for frame_res in frames.take(MAX_ANIMATION_FRAMES_TO_SCAN) {
        match frame_res {
            Ok(frame) => {
                let rgba = frame.into_buffer();
                let quality = evaluate_frame_quality(&rgba);
                if quality.kind == FrameQualityKind::Good {
                    return Ok(Some(rgba));
                }
                let replace = match best.as_ref() {
                    None => true,
                    Some((_, cur_q)) => {
                        if cur_q.kind == FrameQualityKind::BlankTransparent
                            && quality.kind != FrameQualityKind::BlankTransparent
                        {
                            true
                        } else if quality.kind == FrameQualityKind::BlankTransparent
                            && cur_q.kind != FrameQualityKind::BlankTransparent
                        {
                            false
                        } else {
                            quality.score > cur_q.score
                        }
                    }
                };
                if replace {
                    best = Some((rgba, quality));
                }
            }
            Err(err) => {
                // Keep any valid earlier frame when a trailing animation frame is truncated.
                if best.is_some() {
                    break;
                }
                saw_error = Some(VaultError::CorruptedImage(err.to_string()));
                break;
            }
        }
    }

    if let Some((rgba, _)) = best {
        Ok(Some(rgba))
    } else if let Some(err) = saw_error {
        Err(err)
    } else {
        Ok(None)
    }
}

fn decode_animated_best_frame(
    mime_type: &str,
    bytes: &[u8],
) -> Result<Option<DynamicImage>, VaultError> {
    let limits = build_decode_limits(
        MAX_IMAGE_DIMENSION,
        MAX_IMAGE_HEIGHT,
        MAX_DECODE_ALLOC_BYTES,
    );

    match mime_type {
        "image/gif" => {
            let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
                .map_err(|e| VaultError::CorruptedImage(e.to_string()))?;
            decoder
                .set_limits(limits)
                .map_err(|e| VaultError::CorruptedImage(e.to_string()))?;
            let (w, h) = decoder.dimensions();
            validate_dimensions(
                w,
                h,
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_HEIGHT,
                MAX_DECODE_PIXELS,
            )?;
            let best = pick_best_animation_frame(decoder.into_frames())?;
            Ok(best.map(DynamicImage::ImageRgba8))
        }
        "image/webp" => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                .map_err(|e| VaultError::CorruptedImage(e.to_string()))?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            decoder
                .set_limits(limits)
                .map_err(|e| VaultError::CorruptedImage(e.to_string()))?;
            let (w, h) = decoder.dimensions();
            validate_dimensions(
                w,
                h,
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_HEIGHT,
                MAX_DECODE_PIXELS,
            )?;
            let orientation = read_orientation_safe(&mut decoder);
            let Some(best) = pick_best_animation_frame(decoder.into_frames())? else {
                return Ok(None);
            };
            let mut img = DynamicImage::ImageRgba8(best);
            img.apply_orientation(orientation);
            Ok(Some(img))
        }
        _ => Ok(None),
    }
}

fn decode_png_lenient(bytes: &[u8]) -> Result<DynamicImage, VaultError> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: MAX_DECODE_ALLOC_BYTES as usize,
        },
    );
    decoder.ignore_checksums(true);
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);

    let mut reader = decoder
        .read_info()
        .map_err(|e| VaultError::CorruptedImage(format!("PNG デコード失敗: {e}")))?;

    let (w, h, orientation) = {
        let info = reader.info();
        validate_dimensions(
            info.width,
            info.height,
            MAX_IMAGE_DIMENSION,
            MAX_IMAGE_HEIGHT,
            MAX_DECODE_PIXELS,
        )?;
        let orient = info
            .exif_metadata
            .as_deref()
            .and_then(|raw| Orientation::from_exif_chunk(strip_exif_prefix(raw)))
            .unwrap_or(Orientation::NoTransforms);
        (info.width, info.height, orient)
    };

    let buf_size = reader
        .output_buffer_size()
        .ok_or_else(|| VaultError::CorruptedImage("PNG 出力サイズ計算がオーバーフローしました".to_string()))?;
    if buf_size as u64 > MAX_DECODE_ALLOC_BYTES {
        return Err(VaultError::CorruptedImage(
            "PNG 展開サイズが安全上限を超えています".to_string(),
        ));
    }

    let mut buf = vec![0u8; buf_size];
    let output_info = reader
        .next_frame(&mut buf)
        .map_err(|e| VaultError::CorruptedImage(format!("PNG フレーム読込失敗: {e}")))?;
    buf.truncate(output_info.buffer_size());

    let mut img = match output_info.color_type {
        png::ColorType::Rgba => RgbaImage::from_raw(output_info.width, output_info.height, buf)
            .map(DynamicImage::ImageRgba8),
        png::ColorType::Rgb => {
            image::RgbImage::from_raw(output_info.width, output_info.height, buf)
                .map(DynamicImage::ImageRgb8)
        }
        png::ColorType::GrayscaleAlpha => {
            image::GrayAlphaImage::from_raw(output_info.width, output_info.height, buf)
                .map(DynamicImage::ImageLumaA8)
        }
        png::ColorType::Grayscale => {
            image::GrayImage::from_raw(output_info.width, output_info.height, buf)
                .map(DynamicImage::ImageLuma8)
        }
        png::ColorType::Indexed => None,
    }
    .ok_or_else(|| VaultError::CorruptedImage(format!("PNG ピクセル変換失敗 ({w}x{h})")))?;

    img.apply_orientation(orientation);
    Ok(img)
}

fn decode_webp_libwebp(bytes: &[u8]) -> Result<DynamicImage, VaultError> {
    let features = webp::BitstreamFeatures::new(bytes).ok_or_else(|| {
        VaultError::CorruptedImage("WebP ヘッダの解析に失敗しました".to_string())
    })?;
    if features.has_animation() {
        return Err(VaultError::CorruptedImage(
            "アニメーション WebP のデコードに失敗しました".to_string(),
        ));
    }
    validate_dimensions(
        features.width(),
        features.height(),
        MAX_IMAGE_DIMENSION,
        MAX_IMAGE_HEIGHT,
        MAX_DECODE_PIXELS,
    )?;

    let decoded = webp::Decoder::new(bytes)
        .decode()
        .ok_or_else(|| VaultError::CorruptedImage("WebP デコードに失敗しました".to_string()))?;
    let w = decoded.width();
    let h = decoded.height();
    validate_dimensions(
        w,
        h,
        MAX_IMAGE_DIMENSION,
        MAX_IMAGE_HEIGHT,
        MAX_DECODE_PIXELS,
    )?;

    let mut img = if decoded.is_alpha() {
        RgbaImage::from_raw(w, h, decoded.to_vec())
            .map(DynamicImage::ImageRgba8)
            .ok_or_else(|| VaultError::CorruptedImage("WebP RGBA 変換失敗".to_string()))?
    } else {
        image::RgbImage::from_raw(w, h, decoded.to_vec())
            .map(DynamicImage::ImageRgb8)
            .ok_or_else(|| VaultError::CorruptedImage("WebP RGB 変換失敗".to_string()))?
    };

    if let Ok(mut std_dec) = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)) {
        let orientation = read_orientation_safe(&mut std_dec);
        img.apply_orientation(orientation);
    }

    Ok(img)
}

fn decode_with_limits(
    format: ImageFormat,
    bytes: &[u8],
    max_width: u32,
    max_height: u32,
    max_pixels: u64,
    max_alloc: u64,
) -> Result<DynamicImage, VaultError> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(build_decode_limits(max_width, max_height, max_alloc));
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| VaultError::CorruptedImage(e.to_string()))?;
    let (w, h) = decoder.dimensions();
    validate_dimensions(w, h, max_width, max_height, max_pixels)?;
    let orientation = read_orientation_safe(&mut decoder);
    let mut img =
        DynamicImage::from_decoder(decoder).map_err(|e| VaultError::CorruptedImage(e.to_string()))?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Decodes a static/animated image cleanly with pixel limits, EXIF orientation, and lenient fallbacks.
pub fn decode_and_orient_image(mime_type: &str, bytes: &[u8]) -> Result<DynamicImage, VaultError> {
    if mime_type == "image/gif" || mime_type == "image/webp" {
        if let Ok(Some(img)) = decode_animated_best_frame(mime_type, bytes) {
            return Ok(img);
        }
    }

    let format = image_format_for_mime(mime_type).ok_or_else(|| {
        VaultError::UnsupportedFormat(format!("未対応の画像形式です: {mime_type}"))
    })?;

    match decode_with_limits(
        format,
        bytes,
        MAX_IMAGE_DIMENSION,
        MAX_IMAGE_HEIGHT,
        MAX_DECODE_PIXELS,
        MAX_DECODE_ALLOC_BYTES,
    ) {
        Ok(img) => Ok(img),
        Err(primary_err) => match mime_type {
            "image/png" => decode_png_lenient(bytes).or(Err(primary_err)),
            "image/webp" => decode_webp_libwebp(bytes).or(Err(primary_err)),
            _ => Err(primary_err),
        },
    }
}

/// Decodes an ffmpeg-extracted PNG pipe frame safely within dimension/memory limits.
pub fn decode_bounded_png_frame(png_bytes: &[u8]) -> Result<DynamicImage, VaultError> {
    decode_with_limits(
        ImageFormat::Png,
        png_bytes,
        MAX_IMAGE_DIMENSION,
        MAX_IMAGE_HEIGHT,
        MAX_DECODE_PIXELS,
        MAX_DECODE_ALLOC_BYTES,
    )
}

/// Smart-crops extreme aspect ratios (> 1:3) for thumbnail generation while keeping normal ratios intact.
pub fn crop_for_thumbnail(img: &DynamicImage) -> Cow<'_, DynamicImage> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Cow::Borrowed(img);
    }
    // Small rounding slack prevents re-cropping an already 1:3 cropped client thumbnail
    let slack = if w <= THUMB_MAX_EDGE && h <= THUMB_MAX_EDGE {
        3u64
    } else {
        0u64
    };
    let wu = u64::from(w);
    let hu = u64::from(h);

    if hu > wu.saturating_mul(3).saturating_add(slack) {
        let target_h = w.saturating_mul(3).max(1).min(h);
        Cow::Owned(img.crop_imm(0, 0, w, target_h))
    } else if wu > hu.saturating_mul(3).saturating_add(slack) {
        let target_w = h.saturating_mul(3).max(1).min(w);
        let x = (w - target_w) / 2;
        Cow::Owned(img.crop_imm(x, 0, target_w, h))
    } else {
        Cow::Borrowed(img)
    }
}

fn encode_dynamic_image_to_webp(
    img: &DynamicImage,
    max_edge: u32,
    quality: f32,
) -> Result<(Vec<u8>, RgbaImage), VaultError> {
    if img.width() == 0 || img.height() == 0 {
        return Err(VaultError::CorruptedImage(
            "画像サイズが 0 です".to_string(),
        ));
    }

    let safe_max = max_edge.clamp(1, WEBP_MAX_DIM);
    let resized = if img.width() > safe_max || img.height() > safe_max {
        Cow::Owned(img.resize(safe_max, safe_max, FilterType::Lanczos3))
    } else {
        Cow::Borrowed(img)
    };

    let rgba = if resized.width() == 0
        || resized.height() == 0
        || resized.width() > WEBP_MAX_DIM
        || resized.height() > WEBP_MAX_DIM
    {
        let tw = resized.width().clamp(1, WEBP_MAX_DIM);
        let th = resized.height().clamp(1, WEBP_MAX_DIM);
        resized
            .resize_exact(tw, th, FilterType::Triangle)
            .to_rgba8()
    } else {
        resized.to_rgba8()
    };

    let tw = rgba.width();
    let th = rgba.height();
    let encoder = webp::Encoder::from_rgba(rgba.as_raw(), tw, th);
    let webp_mem = encoder
        .encode_simple(false, quality)
        .map_err(|e| VaultError::CorruptedImage(format!("WebP エンコード失敗: {e:?}")))?;

    Ok((webp_mem.to_vec(), rgba))
}

pub fn encode_webp_thumbnail_with_quality(
    img: &DynamicImage,
) -> Result<(GeneratedThumbnail, FrameQuality), VaultError> {
    if img.width() == 0 || img.height() == 0 {
        return Err(VaultError::CorruptedImage(
            "画像サイズが 0 です".to_string(),
        ));
    }
    let cropped = crop_for_thumbnail(img);
    let (data, rgba) =
        encode_dynamic_image_to_webp(cropped.as_ref(), THUMB_MAX_EDGE, THUMB_WEBP_QUALITY)?;
    let tw = rgba.width() as i32;
    let th = rgba.height() as i32;
    let quality = evaluate_frame_quality(&rgba);
    let palette = extract_palette(&rgba, 8);

    Ok((
        GeneratedThumbnail {
            data,
            width: tw,
            height: th,
            palette,
        },
        quality,
    ))
}

pub fn encode_webp_thumbnail(img: &DynamicImage) -> Result<GeneratedThumbnail, VaultError> {
    let (thumb, _) = encode_webp_thumbnail_with_quality(img)?;
    Ok(thumb)
}

pub fn ensure_display_convertible(mime_type: &str) -> Result<(), VaultError> {
    if is_video_mime(mime_type) || mime_type == "image/avif" {
        return Err(VaultError::UnsupportedFormat(
            "この形式はホスト変換表示に対応していません".to_string(),
        ));
    }
    Ok(())
}

/// Decodes and converts a Rust-supported image into display-friendly WebP (<= 4096px) for browser-incompatible formats such as TIFF.
pub fn render_display_webp(mime_type: &str, bytes: &[u8]) -> Result<Vec<u8>, VaultError> {
    ensure_display_convertible(mime_type)?;
    let img = decode_and_orient_image(mime_type, bytes)?;
    let (data, _) = encode_dynamic_image_to_webp(&img, DISPLAY_MAX_EDGE, DISPLAY_WEBP_QUALITY)?;
    Ok(data)
}

pub fn process_image_bytes(bytes: &[u8]) -> Result<ProcessedImage, VaultError> {
    let mime_type = detect_mime(bytes)?;
    if mime_type == "image/avif" {
        validate_media_container_prefix(mime_type, bytes, bytes.len() as u64)?;
        let (width, height) = parse_avif_dimensions(bytes).unwrap_or((0, 0));
        return Ok(ProcessedImage {
            mime_type,
            width,
            height,
            thumbnail: None,
            palette: Vec::new(),
        });
    }
    if is_video_mime(mime_type) {
        validate_media_container_prefix(mime_type, bytes, bytes.len() as u64)?;
        return Ok(ProcessedImage {
            mime_type,
            width: 0,
            height: 0,
            thumbnail: None,
            palette: Vec::new(),
        });
    }

    let img = decode_and_orient_image(mime_type, bytes)?;
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
    if png_bytes.is_empty() || png_bytes.len() > MAX_CLIENT_PNG_BYTES {
        return Err(VaultError::CorruptedImage(
            "クライアントサムネイルのサイズが不正または大きすぎます".to_string(),
        ));
    }
    let img = decode_with_limits(
        ImageFormat::Png,
        png_bytes,
        MAX_CLIENT_PNG_DIMENSION,
        MAX_CLIENT_PNG_DIMENSION,
        MAX_CLIENT_PNG_PIXELS,
        160 * 1024 * 1024,
    )?;
    encode_webp_thumbnail(&img)
}

pub fn rgba_to_png_bytes(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, VaultError> {
    validate_dimensions(
        width,
        height,
        MAX_IMAGE_DIMENSION,
        MAX_IMAGE_HEIGHT,
        MAX_DECODE_PIXELS,
    )?;
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4));
    if expected_len != Some(rgba.len()) {
        return Err(VaultError::CorruptedImage(
            "不正なクリップボード画像サイズです".to_string(),
        ));
    }
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
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, Rgba};

    fn make_bmff_box(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + payload.len());
        let len = (8 + payload.len()) as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(fourcc);
        out.extend_from_slice(payload);
        out
    }

    fn make_bmff_fullbox(fourcc: &[u8; 4], version: u8, flags: [u8; 3], payload: &[u8]) -> Vec<u8> {
        let mut inner = Vec::with_capacity(4 + payload.len());
        inner.push(version);
        inner.extend_from_slice(&flags);
        inner.extend_from_slice(payload);
        make_bmff_box(fourcc, &inner)
    }

    fn encode_ebml_element(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(id);
        // Encode length up to 16383 using 2-byte VINT
        let len = payload.len();
        if len < 127 {
            out.push(0x80 | (len as u8));
        } else {
            let v = 0x4000u16 | (len as u16);
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn test_evaluate_frame_quality_cases() {
        let blank = RgbaImage::from_pixel(120, 90, Rgba([0, 0, 0, 0]));
        assert_eq!(
            evaluate_frame_quality(&blank).kind,
            FrameQualityKind::BlankTransparent
        );

        let black = RgbaImage::from_pixel(120, 90, Rgba([5, 5, 5, 255]));
        assert_eq!(
            evaluate_frame_quality(&black).kind,
            FrameQualityKind::FlatOrDark
        );

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

    #[test]
    fn test_detect_mime_new_formats_and_rejections() {
        // 0-byte file
        assert!(matches!(detect_mime(b""), Err(VaultError::CorruptedImage(_))));

        // Fake "BM" text must be rejected
        assert!(matches!(
            detect_mime(b"BM_this_is_plain_text_not_a_bmp_file"),
            Err(VaultError::UnsupportedFormat(_))
        ));

        // Valid BITMAPINFOHEADER BMP header
        let mut bmp = vec![0u8; 30];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&64i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&64i32.to_le_bytes());
        assert_eq!(detect_mime(&bmp).unwrap(), "image/bmp");

        // Valid ICO vs CUR rejection
        let mut ico = vec![0u8; 30];
        ico[0..4].copy_from_slice(&[0x00, 0x00, 0x01, 0x00]);
        ico[4..6].copy_from_slice(&1u16.to_le_bytes());
        ico[12..14].copy_from_slice(&32u16.to_le_bytes());
        ico[14..18].copy_from_slice(&128u32.to_le_bytes());
        ico[18..22].copy_from_slice(&22u32.to_le_bytes());
        assert_eq!(detect_mime(&ico).unwrap(), "image/x-icon");
        ico[2] = 0x02; // .cur cursor
        assert!(detect_mime(&ico).is_err());

        // TIFF LE, BE, and Canon CR2 rejection
        let tiff_le = [0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00];
        let tiff_be = [0x4D, 0x4D, 0x00, 0x2A, 0x00, 0x00, 0x00, 0x08, 0x01, 0x00];
        let cr2 = [0x49, 0x49, 0x2A, 0x00, 0x10, 0x00, 0x00, 0x00, b'C', b'R', 0x02, 0x00];
        assert_eq!(detect_mime(&tiff_le).unwrap(), "image/tiff");
        assert_eq!(detect_mime(&tiff_be).unwrap(), "image/tiff");
        assert!(detect_mime(&cr2).is_err());

        // HEIC and M4A audio rejection (even with iso/hev compatible brands)
        let heic = make_bmff_box(b"ftyp", b"heic\0\0\0\0mif1heiciso2");
        let m4a = make_bmff_box(b"ftyp", b"M4A \0\0\0\0isommp42");
        assert!(matches!(detect_mime(&heic), Err(VaultError::UnsupportedFormat(_))));
        assert!(matches!(detect_mime(&m4a), Err(VaultError::UnsupportedFormat(_))));

        // Leading wide box + ftyp mov, 64-bit ftyp mp4, legacy moov-only mov
        let mut wide_mov = make_bmff_box(b"wide", &[]);
        wide_mov.extend_from_slice(&make_bmff_box(b"ftyp", b"qt  \0\0\0\0qt  "));
        assert_eq!(detect_mime(&wide_mov).unwrap(), "video/quicktime");

        let mut largesize_mp4 = Vec::new();
        largesize_mp4.extend_from_slice(&1u32.to_be_bytes());
        largesize_mp4.extend_from_slice(b"ftyp");
        largesize_mp4.extend_from_slice(&32u64.to_be_bytes());
        largesize_mp4.extend_from_slice(b"isom\0\0\0\0isommp41");
        assert_eq!(detect_mime(&largesize_mp4).unwrap(), "video/mp4");

        let mvhd = make_bmff_fullbox(b"mvhd", 0, [0; 3], &[0; 16]);
        let legacy_mov = make_bmff_box(b"moov", &mvhd);
        assert_eq!(detect_mime(&legacy_mov).unwrap(), "video/quicktime");

        // EBML: Matroska with "webm" string in Void element MUST NOT match video/webm; real WebM beyond 64 bytes MUST match
        let mut mkv_hdr_body = Vec::new();
        mkv_hdr_body.extend_from_slice(&encode_ebml_element(&[0xEC], b"webm_fake_metadata"));
        mkv_hdr_body.extend_from_slice(&encode_ebml_element(&[0x42, 0x82], b"matroska"));
        let mkv = encode_ebml_element(&[0x1A, 0x45, 0xDF, 0xA3], &mkv_hdr_body);
        assert!(matches!(detect_mime(&mkv), Err(VaultError::UnsupportedFormat(_))));

        let mut webm_hdr_body = Vec::new();
        webm_hdr_body.extend_from_slice(&encode_ebml_element(&[0xEC], &[0u8; 80]));
        webm_hdr_body.extend_from_slice(&encode_ebml_element(&[0x42, 0x82], b"webm"));
        let webm = encode_ebml_element(&[0x1A, 0x45, 0xDF, 0xA3], &webm_hdr_body);
        assert_eq!(detect_mime(&webm).unwrap(), "video/webm");
    }

    #[test]
    fn test_container_prefix_validation_and_avif_ispe() {
        // ftyp-only truncated stub must fail validation
        let mp4_stub = make_bmff_box(b"ftyp", b"isom\0\0\0\0isommp41");
        assert!(matches!(
            validate_media_container_prefix("video/mp4", &mp4_stub, mp4_stub.len() as u64),
            Err(VaultError::CorruptedImage(_))
        ));

        // Synthetic AVIF with pitm=1, ipco=[thumb ispe 160x120, main ispe 1920x1080, irot 90deg], ipma associating item 1 -> props 2,3
        let ftyp = make_bmff_box(b"ftyp", b"avif\0\0\0\0avifmif1");
        let pitm = make_bmff_fullbox(b"pitm", 0, [0; 3], &1u16.to_be_bytes());
        let mut thumb_ispe_data = Vec::new();
        thumb_ispe_data.extend_from_slice(&160u32.to_be_bytes());
        thumb_ispe_data.extend_from_slice(&120u32.to_be_bytes());
        let prop1 = make_bmff_fullbox(b"ispe", 0, [0; 3], &thumb_ispe_data);

        let mut main_ispe_data = Vec::new();
        main_ispe_data.extend_from_slice(&1920u32.to_be_bytes());
        main_ispe_data.extend_from_slice(&1080u32.to_be_bytes());
        let prop2 = make_bmff_fullbox(b"ispe", 0, [0; 3], &main_ispe_data);
        let prop3 = make_bmff_box(b"irot", &[1u8]); // 90 deg CCW -> swaps width and height

        let mut ipco_inner = Vec::new();
        ipco_inner.extend_from_slice(&prop1);
        ipco_inner.extend_from_slice(&prop2);
        ipco_inner.extend_from_slice(&prop3);
        let ipco = make_bmff_box(b"ipco", &ipco_inner);

        let mut ipma_data = Vec::new();
        ipma_data.extend_from_slice(&1u32.to_be_bytes()); // entry_count = 1
        ipma_data.extend_from_slice(&1u16.to_be_bytes()); // item_id = 1
        ipma_data.push(2); // assoc_count = 2
        ipma_data.push(2); // prop_idx = 2 (1920x1080)
        ipma_data.push(3); // prop_idx = 3 (irot = 1)
        let ipma = make_bmff_fullbox(b"ipma", 0, [0; 3], &ipma_data);

        let mut iprp_inner = Vec::new();
        iprp_inner.extend_from_slice(&ipco);
        iprp_inner.extend_from_slice(&ipma);
        let iprp = make_bmff_box(b"iprp", &iprp_inner);

        let mut meta_inner = Vec::new();
        meta_inner.extend_from_slice(&pitm);
        meta_inner.extend_from_slice(&iprp);
        let meta = make_bmff_fullbox(b"meta", 0, [0; 3], &meta_inner);

        let mut avif = Vec::new();
        avif.extend_from_slice(&ftyp);
        avif.extend_from_slice(&meta);

        let processed = process_image_bytes(&avif).unwrap();
        assert_eq!(processed.mime_type, "image/avif");
        assert_eq!((processed.width, processed.height), (1080, 1920));
    }

    #[test]
    fn test_bmp_tiff_ico_roundtrip_and_display_webp() {
        let mut src = RgbaImage::new(64, 48);
        for (x, y, px) in src.enumerate_pixels_mut() {
            *px = Rgba([(x * 3) as u8, (y * 5) as u8, 200, 255]);
        }
        let dyn_img = DynamicImage::ImageRgba8(src);

        for (fmt, expected_mime) in [
            (ImageFormat::Bmp, "image/bmp"),
            (ImageFormat::Tiff, "image/tiff"),
            (ImageFormat::Ico, "image/x-icon"),
        ] {
            let mut bytes = Vec::new();
            dyn_img
                .write_to(&mut Cursor::new(&mut bytes), fmt)
                .unwrap();
            let processed = process_image_bytes(&bytes).unwrap();
            assert_eq!(processed.mime_type, expected_mime);
            assert_eq!((processed.width, processed.height), (64, 48));
            let thumb = processed.thumbnail.unwrap();
            assert!(!thumb.data.is_empty());
            assert!(!processed.palette.is_empty());

            let display_webp = render_display_webp(expected_mime, &bytes).unwrap();
            let disp_img = image::load_from_memory_with_format(&display_webp, ImageFormat::WebP)
                .unwrap();
            assert_eq!((disp_img.width(), disp_img.height()), (64, 48));
        }
    }

    #[test]
    fn test_exif_orientation_auto_rotate_with_prefix() {
        // Minimal valid TIFF Orientation=6 tag payload prefixed with "Exif\0\0"
        let mut chunk = b"Exif\0\0".to_vec();
        chunk.extend_from_slice(&[
            0x49, 0x49, 0x2A, 0x00, // II*\0 LE
            0x08, 0x00, 0x00, 0x00, // IFD0 offset = 8
            0x01, 0x00, // 1 tag
            0x12, 0x01, // Tag 0x0112 (Orientation)
            0x03, 0x00, // SHORT
            0x01, 0x00, 0x00, 0x00, // count = 1
            0x06, 0x00, 0x00, 0x00, // value = 6 (Rotate90 CW)
            0x00, 0x00, 0x00, 0x00, // next IFD
        ]);
        let orient = Orientation::from_exif_chunk(strip_exif_prefix(&chunk)).unwrap();
        assert_eq!(orient, Orientation::Rotate90);

        // Encode an 80x40 PNG with eXIf chunk containing raw TIFF orientation=6
        let mut img = RgbaImage::new(80, 40);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = Rgba([(x * 3) as u8, (y * 6) as u8, 120, 255]);
        }
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 80, 40);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().unwrap();
            writer.write_chunk(png::chunk::ChunkType(*b"eXIf"), &chunk[6..]).unwrap();
            writer.write_image_data(img.as_raw()).unwrap();
        }

        let processed = process_image_bytes(&png_bytes).unwrap();
        assert_eq!((processed.width, processed.height), (40, 80));
        let thumb = processed.thumbnail.unwrap();
        assert_eq!((thumb.width, thumb.height), (40, 80));
    }

    #[test]
    fn test_extreme_aspect_ratio_smart_crop() {
        // Vertical 1:10 webtoon: top red, bottom blue
        let mut webtoon = RgbaImage::new(200, 2000);
        for (_, y, px) in webtoon.enumerate_pixels_mut() {
            *px = if y < 600 {
                Rgba([220, 40, 40, 255])
            } else {
                Rgba([40, 40, 220, 255])
            };
        }
        let mut webtoon_png = Vec::new();
        DynamicImage::ImageRgba8(webtoon)
            .write_to(&mut Cursor::new(&mut webtoon_png), ImageFormat::Png)
            .unwrap();
        let processed = process_image_bytes(&webtoon_png).unwrap();
        assert_eq!((processed.width, processed.height), (200, 2000));
        let thumb = processed.thumbnail.unwrap();
        // Top 200x600 cropped -> scaled to 400 max edge => ~133x400
        assert_eq!(thumb.height, 400);
        assert!((132..=134).contains(&thumb.width));

        // Horizontal 10:1 panorama: green center, red edges
        let mut pano = RgbaImage::new(2000, 200);
        for (x, _, px) in pano.enumerate_pixels_mut() {
            *px = if (700..1300).contains(&x) {
                Rgba([40, 210, 40, 255])
            } else {
                Rgba([210, 40, 40, 255])
            };
        }
        let dyn_pano = DynamicImage::ImageRgba8(pano);
        let pano_thumb = encode_webp_thumbnail(&dyn_pano).unwrap();
        assert_eq!(pano_thumb.width, 400);
        assert!((132..=134).contains(&pano_thumb.height));
        let decoded = image::load_from_memory(&pano_thumb.data).unwrap().to_rgba8();
        let mid_px = decoded.get_pixel(200, 66);
        assert!(mid_px[1] > mid_px[0]); // green center preserved!
    }

    #[test]
    fn test_animated_gif_skips_blank_first_frame() {
        let mut gif_bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut gif_bytes);
            enc.set_repeat(Repeat::Infinite).unwrap();

            let f0 = Frame::from_parts(
                RgbaImage::from_pixel(120, 90, Rgba([0, 0, 0, 0])),
                0,
                0,
                Delay::from_numer_denom_ms(100, 1),
            );
            let mut grad = RgbaImage::new(120, 90);
            for (x, y, px) in grad.enumerate_pixels_mut() {
                *px = Rgba([((x * 2) % 255) as u8, ((y * 3) % 255) as u8, 180, 255]);
            }
            let f1 = Frame::from_parts(grad, 0, 0, Delay::from_numer_denom_ms(100, 1));
            enc.encode_frames(vec![f0, f1]).unwrap();
        }

        let processed = process_image_bytes(&gif_bytes).unwrap();
        assert_eq!(processed.mime_type, "image/gif");
        let thumb = processed.thumbnail.unwrap();
        let thumb_rgba = image::load_from_memory(&thumb.data).unwrap().to_rgba8();
        assert_eq!(evaluate_frame_quality(&thumb_rgba).kind, FrameQualityKind::Good);
    }

    #[test]
    fn test_lenient_png_bad_crc_recovery() {
        let mut img = RgbaImage::new(64, 48);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = Rgba([(x * 3) as u8, (y * 4) as u8, 128, 255]);
        }
        let mut png_bytes = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
            .unwrap();

        // Corrupt the CRC of the first chunk after signature (IHDR starts at offset 8: len=13 -> CRC at 8+4+4+13..+4 = 29..33)
        png_bytes[29] ^= 0xFF;
        assert!(image::load_from_memory(&png_bytes).is_err());

        let processed = process_image_bytes(&png_bytes).unwrap();
        assert_eq!((processed.width, processed.height), (64, 48));
        assert!(processed.thumbnail.is_some());
    }

    #[test]
    fn test_decompression_bomb_header_rejected_without_oom() {
        // Minimal PNG signature + IHDR claiming 50000 x 50000 pixels
        let mut fake_png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        fake_png.extend_from_slice(&13u32.to_be_bytes());
        fake_png.extend_from_slice(b"IHDR");
        fake_png.extend_from_slice(&50_000u32.to_be_bytes());
        fake_png.extend_from_slice(&50_000u32.to_be_bytes());
        fake_png.extend_from_slice(&[8, 6, 0, 0, 0]);
        fake_png.extend_from_slice(&[0u8; 4]);

        let err = process_image_bytes(&fake_png).unwrap_err();
        assert!(matches!(err, VaultError::CorruptedImage(_)));
    }

    #[test]
    fn test_translucent_palette_fallback() {
        let img = RgbaImage::from_pixel(32, 32, Rgba([60, 140, 220, 64]));
        let palette = extract_palette(&img, 4);
        assert_eq!(palette, vec!["#3c8cdc"]);
    }
}
