use unicode_normalization::UnicodeNormalization;

pub fn mime_to_extension(mime: &str) -> &'static str {
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        _ => "bin",
    }
}

fn known_extensions_for_mime(mime: &str) -> &'static [&'static str] {
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/png" => &[".png"],
        "image/jpeg" => &[".jpg", ".jpeg", ".jpe"],
        "image/webp" => &[".webp"],
        "image/gif" => &[".gif"],
        "image/avif" => &[".avif"],
        "video/mp4" => &[".mp4", ".m4v"],
        "video/webm" => &[".webm"],
        "video/quicktime" => &[".mov", ".qt"],
        _ => &[],
    }
}

fn strip_matching_extension<'a>(raw: &'a str, mime: &str) -> &'a str {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    for ext in known_extensions_for_mime(mime) {
        if lower.ends_with(ext) && trimmed.len() > ext.len() {
            let cut = trimmed.len() - ext.len();
            if trimmed.is_char_boundary(cut) {
                return &trimmed[..cut];
            }
        }
    }
    trimmed
}

fn is_windows_reserved_stem(stem: &str) -> bool {
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

pub fn sanitize_name_segment(raw: &str, fallback: &str) -> String {
    let nfc: String = raw.nfc().collect();
    let mut cleaned = String::with_capacity(nfc.len());
    for c in nfc.chars() {
        if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            cleaned.push('_');
        } else {
            cleaned.push(c);
        }
    }

    let trimmed = cleaned.trim_matches(|c: char| c.is_whitespace() || c == '.');
    let mut clamped = String::new();
    for (idx, c) in trimmed.chars().enumerate() {
        if idx >= 100 || clamped.len() + c.len_utf8() > 180 {
            break;
        }
        clamped.push(c);
    }
    let final_seg = clamped.trim_matches(|c: char| c.is_whitespace() || c == '.');

    if final_seg.is_empty() || final_seg == "." || final_seg == ".." {
        return fallback.to_string();
    }

    let first_part = final_seg.split('.').next().unwrap_or("").trim_end();
    if is_windows_reserved_stem(first_part) {
        format!("_{final_seg}")
    } else {
        final_seg.to_string()
    }
}

fn short_id_prefix(id: &str) -> String {
    let taken: String = id.chars().take(8).collect();
    if taken.is_empty() {
        "item".to_string()
    } else {
        taken
    }
}

pub fn build_item_filename(title: &str, item_id: &str, mime_type: &str) -> String {
    let ext = mime_to_extension(mime_type);
    let stripped = strip_matching_extension(title, mime_type);
    let fallback = format!("item_{}", short_id_prefix(item_id));
    let stem = sanitize_name_segment(stripped, &fallback);
    format!("{stem}.{ext}")
}

pub fn build_group_dir_name(title: &str, group_id: &str) -> String {
    sanitize_name_segment(title, &format!("manga_{}", short_id_prefix(group_id)))
}

pub fn build_folder_dir_name(name: &str, folder_id: &str) -> String {
    sanitize_name_segment(name, &format!("folder_{}", short_id_prefix(folder_id)))
}

pub fn build_group_page_filename(
    page_index_1based: usize,
    total_pages: usize,
    title: &str,
    mime_type: &str,
) -> String {
    let width = if total_pages >= 1000 { 4 } else { 3 };
    let ext = mime_to_extension(mime_type);
    let stripped = strip_matching_extension(title, mime_type);
    if stripped.trim().is_empty() {
        format!("{page_index_1based:0width$}.{ext}")
    } else {
        let stem = sanitize_name_segment(stripped, "");
        if stem.is_empty() {
            format!("{page_index_1based:0width$}.{ext}")
        } else {
            format!("{page_index_1based:0width$}_{stem}.{ext}")
        }
    }
}

pub fn content_disposition_attachment(filename: &str) -> String {
    let safe_name = match filename.rsplit_once('.') {
        Some((raw_stem, raw_ext)) if !raw_stem.is_empty() && !raw_ext.is_empty() => {
            let ext: String = raw_ext
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(16)
                .collect();
            if ext.is_empty() {
                sanitize_name_segment(filename, "download.bin")
            } else {
                let stem_budget = 180usize.saturating_sub(ext.len() + 1);
                let stem = sanitize_name_segment(raw_stem, "download");
                let mut clamped_stem = String::new();
                for c in stem.chars() {
                    if clamped_stem.len() + c.len_utf8() > stem_budget {
                        break;
                    }
                    clamped_stem.push(c);
                }
                let trimmed_stem =
                    clamped_stem.trim_matches(|c: char| c.is_whitespace() || c == '.');
                let final_stem = if trimmed_stem.is_empty() {
                    "download"
                } else {
                    trimmed_stem
                };
                format!("{final_stem}.{ext}")
            }
        }
        _ => sanitize_name_segment(filename, "download.bin"),
    };
    let ascii_fallback: String = safe_name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && !matches!(c, '"' | '\\' | '/' | ';') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let ascii_clean = if ascii_fallback.trim_matches('.').is_empty() {
        "download.bin".to_string()
    } else {
        ascii_fallback
    };

    let mut utf8_encoded = String::with_capacity(safe_name.len() * 3);
    for b in safe_name.as_bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if keep {
            utf8_encoded.push(*b as char);
        } else {
            utf8_encoded.push_str(&format!("%{b:02X}"));
        }
    }

    format!("attachment; filename=\"{ascii_clean}\"; filename*=UTF-8''{utf8_encoded}")
}
