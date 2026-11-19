use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use rusqlite::{params, OptionalExtension};
use tokio::sync::mpsc;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::db::{now_ms, ExportItemMeta, VaultDb};
use crate::error::VaultError;
use crate::models::{
    EntryKind, EntryRef, PlainExportBatchRequest, PlainExportItemRequest, PlainExportReport,
};

const STREAM_CHUNK_BYTES: usize = 4 * 1024 * 1024;
const ZIP_GPBF_DATA_DESC_UTF8: u16 = 0x0808;
const ZIP_METHOD_STORE: u16 = 0;
const ZIP_VERSION_20: u16 = 20;
const ZIP_VERSION_45: u16 = 45;
const ZIP_UNIX_VERSION_MADE_BY: u16 = (3 << 8) | 45;
const ZIP_UNIX_REGULAR_FILE_ATTRS: u32 = 0o100644 << 16;

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            if (c & 1) != 0 {
                c = 0xEDB8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = make_crc32_table();

pub(crate) fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = CRC32_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

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
    let clamped: String = trimmed.chars().take(100).collect();
    let final_seg = clamped.trim_matches(|c: char| c.is_whitespace() || c == '.');

    if final_seg.is_empty() || final_seg == "." || final_seg == ".." {
        return fallback.to_string();
    }

    let first_part = final_seg.split('.').next().unwrap_or("");
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
    let safe_name = sanitize_name_segment(filename, "download.bin");
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

#[derive(Debug, Clone)]
pub struct PlannedExportFile {
    pub rel_segments: Vec<String>,
    pub item_id: String,
    pub byte_size: u64,
    pub chunk_size: usize,
}

impl PlannedExportFile {
    pub fn zip_entry_name(&self) -> String {
        self.rel_segments.join("/")
    }
}

#[derive(Debug, Clone)]
pub struct PlainExportPlan {
    pub files: Vec<PlannedExportFile>,
    pub group_count: usize,
    pub folder_count: usize,
    pub total_bytes: u64,
    pub suggested_archive_name: String,
}

#[derive(Default)]
struct PathAllocator {
    used_lower: HashSet<String>,
}

impl PathAllocator {
    fn reserve_dir(&mut self, parent: &[String], base_name: &str) -> String {
        let prefix = if parent.is_empty() {
            String::new()
        } else {
            format!("{}/", parent.join("/"))
        };
        let mut candidate = base_name.to_string();
        let mut idx = 2usize;
        loop {
            let key = format!("{prefix}{candidate}").to_ascii_lowercase();
            if self.used_lower.insert(key) {
                return candidate;
            }
            candidate = format!("{base_name} ({idx})");
            idx += 1;
        }
    }

    fn reserve_file(&mut self, parent: &[String], filename: &str) -> String {
        let prefix = if parent.is_empty() {
            String::new()
        } else {
            format!("{}/", parent.join("/"))
        };
        let (stem, ext_with_dot) = match filename.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
            _ => (filename, String::new()),
        };
        let mut candidate = filename.to_string();
        let mut idx = 2usize;
        loop {
            let key = format!("{prefix}{candidate}").to_ascii_lowercase();
            if self.used_lower.insert(key) {
                return candidate;
            }
            candidate = format!("{stem} ({idx}){ext_with_dot}");
            idx += 1;
        }
    }
}

struct RawFolderMeta {
    id: String,
    parent_id: Option<String>,
    name: String,
}

struct PlanBuilder<'a> {
    db: &'a VaultDb,
    allocator: PathAllocator,
    folders_by_id: HashMap<String, RawFolderMeta>,
    children_by_parent: HashMap<Option<String>, Vec<String>>,
    files: Vec<PlannedExportFile>,
    group_count: usize,
    folder_count: usize,
    total_bytes: u64,
}

impl<'a> PlanBuilder<'a> {
    fn new(db: &'a VaultDb) -> Result<Self, VaultError> {
        let mut folders_by_id = HashMap::new();
        let mut children_by_parent: HashMap<Option<String>, Vec<String>> = HashMap::new();
        {
            let mut stmt = db.conn().prepare(
                "SELECT id, parent_id, name
                 FROM folders
                 ORDER BY sort_order ASC, name COLLATE NOCASE ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(RawFolderMeta {
                    id: r.get(0)?,
                    parent_id: r.get(1)?,
                    name: r.get(2)?,
                })
            })?;
            for row in rows {
                let f = row?;
                children_by_parent
                    .entry(f.parent_id.clone())
                    .or_default()
                    .push(f.id.clone());
                folders_by_id.insert(f.id.clone(), f);
            }
        }
        Ok(Self {
            db,
            allocator: PathAllocator::default(),
            folders_by_id,
            children_by_parent,
            files: Vec::new(),
            group_count: 0,
            folder_count: 0,
            total_bytes: 0,
        })
    }

    fn push_item_in_parent(&mut self, parent: &[String], meta: ExportItemMeta) {
        let filename = build_item_filename(&meta.title, &meta.id, &meta.mime_type);
        let final_name = self.allocator.reserve_file(parent, &filename);
        let mut rel = parent.to_vec();
        rel.push(final_name);
        self.total_bytes = self.total_bytes.saturating_add(meta.byte_size);
        self.files.push(PlannedExportFile {
            rel_segments: rel,
            item_id: meta.id,
            byte_size: meta.byte_size,
            chunk_size: meta.chunk_size,
        });
    }

    fn push_group_in_parent(
        &mut self,
        parent: &[String],
        group_id: &str,
        group_title: &str,
    ) -> Result<(), VaultError> {
        let mut stmt = self.db.conn().prepare(
            "SELECT i.id, i.title, i.mime_type, i.byte_size, i.chunk_size, i.deleted_at
             FROM group_items gi
             JOIN items i ON i.id = gi.item_id
             WHERE gi.group_id = ?1 AND i.deleted_at IS NULL
             ORDER BY gi.page_order ASC",
        )?;
        let rows = stmt.query_map(params![group_id], |r| {
            let byte_size: i64 = r.get(3)?;
            let chunk_size: i64 = r.get(4)?;
            Ok(ExportItemMeta {
                id: r.get(0)?,
                title: r.get(1)?,
                mime_type: r.get(2)?,
                byte_size: byte_size.max(0) as u64,
                chunk_size: chunk_size.max(0) as usize,
                deleted_at: r.get(5)?,
            })
        })?;
        let mut pages = Vec::new();
        for row in rows {
            pages.push(row?);
        }
        if pages.is_empty() {
            return Ok(());
        }

        let base_dir = build_group_dir_name(group_title, group_id);
        let final_dir = self.allocator.reserve_dir(parent, &base_dir);
        let mut group_parent = parent.to_vec();
        group_parent.push(final_dir);

        self.group_count += 1;
        let total_pages = pages.len();
        for (idx, page) in pages.into_iter().enumerate() {
            let page_name =
                build_group_page_filename(idx + 1, total_pages, &page.title, &page.mime_type);
            let final_name = self.allocator.reserve_file(&group_parent, &page_name);
            let mut rel = group_parent.clone();
            rel.push(final_name);
            self.total_bytes = self.total_bytes.saturating_add(page.byte_size);
            self.files.push(PlannedExportFile {
                rel_segments: rel,
                item_id: page.id,
                byte_size: page.byte_size,
                chunk_size: page.chunk_size,
            });
        }
        Ok(())
    }

    fn push_folder_recursive(
        &mut self,
        parent: &[String],
        folder_id: &str,
        include_subfolders: bool,
        ancestors: &mut HashSet<String>,
    ) -> Result<(), VaultError> {
        if !ancestors.insert(folder_id.to_string()) {
            return Ok(());
        }
        let folder_name = self
            .folders_by_id
            .get(folder_id)
            .map(|f| f.name.clone())
            .ok_or_else(|| VaultError::NotFound(folder_id.to_string()))?;

        let base_dir = build_folder_dir_name(&folder_name, folder_id);
        let final_dir = self.allocator.reserve_dir(parent, &base_dir);
        let mut cur_parent = parent.to_vec();
        cur_parent.push(final_dir);
        self.folder_count += 1;

        // 1. Standalone items in folder (non-trash, not belonging to a group)
        {
            let mut stmt = self.db.conn().prepare(
                "SELECT i.id, i.title, i.mime_type, i.byte_size, i.chunk_size, i.deleted_at
                 FROM folder_items fi
                 JOIN items i ON i.id = fi.item_id
                 WHERE fi.folder_id = ?1
                   AND i.deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)
                 ORDER BY i.created_at ASC, i.id ASC",
            )?;
            let rows = stmt.query_map(params![folder_id], |r| {
                let byte_size: i64 = r.get(3)?;
                let chunk_size: i64 = r.get(4)?;
                Ok(ExportItemMeta {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    mime_type: r.get(2)?,
                    byte_size: byte_size.max(0) as u64,
                    chunk_size: chunk_size.max(0) as usize,
                    deleted_at: r.get(5)?,
                })
            })?;
            let mut items = Vec::new();
            for r in rows {
                items.push(r?);
            }
            for item in items {
                self.push_item_in_parent(&cur_parent, item);
            }
        }

        // 2. Manga groups in folder (non-trash)
        {
            let mut stmt = self.db.conn().prepare(
                "SELECT g.id, g.title
                 FROM folder_groups fg
                 JOIN groups g ON g.id = fg.group_id
                 WHERE fg.folder_id = ?1 AND g.deleted_at IS NULL
                 ORDER BY g.created_at ASC, g.id ASC",
            )?;
            let rows = stmt.query_map(params![folder_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut groups = Vec::new();
            for r in rows {
                groups.push(r?);
            }
            for (gid, gtitle) in groups {
                self.push_group_in_parent(&cur_parent, &gid, &gtitle)?;
            }
        }

        // 3. Subfolders
        if include_subfolders {
            let child_ids = self
                .children_by_parent
                .get(&Some(folder_id.to_string()))
                .cloned()
                .unwrap_or_default();
            for child_id in child_ids {
                self.push_folder_recursive(&cur_parent, &child_id, true, ancestors)?;
            }
        }

        ancestors.remove(folder_id);
        Ok(())
    }
}

fn default_export_archive_name() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d, hh, mm, ss) = unix_secs_to_utc_ymdhms(secs);
    format!("tuck-export-{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}.zip")
}

pub fn build_plain_export_plan(
    db: &VaultDb,
    entries: &[EntryRef],
    folder_ids: &[String],
    include_subfolders: bool,
) -> Result<PlainExportPlan, VaultError> {
    let mut builder = PlanBuilder::new(db)?;
    let mut seen_images = HashSet::new();
    let mut seen_groups = HashSet::new();
    let mut seen_folders = HashSet::new();

    for entry in entries {
        match entry.kind {
            EntryKind::Image => {
                if !seen_images.insert(entry.id.clone()) {
                    continue;
                }
                let meta: Option<ExportItemMeta> = db
                    .conn()
                    .query_row(
                        "SELECT id, title, mime_type, byte_size, chunk_size, deleted_at
                         FROM items
                         WHERE id = ?1 AND deleted_at IS NULL",
                        params![entry.id],
                        |r| {
                            let byte_size: i64 = r.get(3)?;
                            let chunk_size: i64 = r.get(4)?;
                            Ok(ExportItemMeta {
                                id: r.get(0)?,
                                title: r.get(1)?,
                                mime_type: r.get(2)?,
                                byte_size: byte_size.max(0) as u64,
                                chunk_size: chunk_size.max(0) as usize,
                                deleted_at: r.get(5)?,
                            })
                        },
                    )
                    .optional()?;
                if let Some(m) = meta {
                    builder.push_item_in_parent(&[], m);
                }
            }
            EntryKind::Group => {
                if !seen_groups.insert(entry.id.clone()) {
                    continue;
                }
                let group_row: Option<(String, String)> = db
                    .conn()
                    .query_row(
                        "SELECT id, title FROM groups WHERE id = ?1 AND deleted_at IS NULL",
                        params![entry.id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some((gid, gtitle)) = group_row {
                    builder.push_group_in_parent(&[], &gid, &gtitle)?;
                }
            }
        }
    }

    for fid in folder_ids {
        if !seen_folders.insert(fid.clone()) {
            continue;
        }
        let mut ancestors = HashSet::new();
        builder.push_folder_recursive(&[], fid, include_subfolders, &mut ancestors)?;
    }

    if builder.files.is_empty() {
        return Err(VaultError::InvalidInput(
            "書き出す対象のファイルが見つかりません".to_string(),
        ));
    }

    let suggested_archive_name = if entries.is_empty() && folder_ids.len() == 1 {
        let fid = &folder_ids[0];
        let name = builder
            .folders_by_id
            .get(fid)
            .map(|f| build_folder_dir_name(&f.name, &f.id))
            .unwrap_or_else(|| "folder".to_string());
        format!("{name}.zip")
    } else if folder_ids.is_empty() && entries.len() == 1 && entries[0].kind == EntryKind::Group {
        let base = builder
            .files
            .first()
            .and_then(|f| f.rel_segments.first())
            .cloned()
            .unwrap_or_else(|| "manga".to_string());
        format!("{base}.zip")
    } else {
        default_export_archive_name()
    };

    Ok(PlainExportPlan {
        files: builder.files,
        group_count: builder.group_count,
        folder_count: builder.folder_count,
        total_bytes: builder.total_bytes,
        suggested_archive_name,
    })
}

fn paths_refer_to_same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if let (Ok(ca), Ok(cb)) = (fs::canonicalize(a), fs::canonicalize(b)) {
        return ca == cb;
    }
    if let (Some(ap), Some(an), Some(bp), Some(bn)) =
        (a.parent(), a.file_name(), b.parent(), b.file_name())
    {
        if an == bn {
            if let (Ok(cap), Ok(cbp)) = (fs::canonicalize(ap), fs::canonicalize(bp)) {
                return cap == cbp;
            }
        }
    }
    false
}

fn stream_item_via_temp_file(
    db: &VaultDb,
    item_id: &str,
    dest_path: &Path,
    overwrite: bool,
) -> Result<u64, VaultError> {
    if paths_refer_to_same_file(dest_path, db.path()) {
        return Err(VaultError::InvalidInput(
            "現在開いている保管庫ファイルは上書きできません".to_string(),
        ));
    }
    let parent = dest_path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(p) = parent {
        fs::create_dir_all(p)?;
    }

    let tmp_name = format!(".tuck-export-{}.tmp", Uuid::new_v4());
    let tmp_path = match parent {
        Some(p) => p.join(&tmp_name),
        None => PathBuf::from(&tmp_name),
    };

    let write_res = (|| -> Result<u64, VaultError> {
        let file = File::create(&tmp_path)?;
        let mut writer = BufWriter::with_capacity(256 * 1024, file);
        let written = db.write_blob_to_writer(item_id, &mut writer)?;
        writer.flush()?;
        writer
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        Ok(written)
    })();

    let written = match write_res {
        Ok(n) => n,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    if overwrite && dest_path.exists() {
        if let Err(e) = fs::remove_file(dest_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(e.into());
        }
    }
    if let Err(e) = fs::rename(&tmp_path, dest_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e.into());
    }

    Ok(written)
}

pub fn export_plain_item(
    db: &VaultDb,
    req: &PlainExportItemRequest,
) -> Result<PlainExportReport, VaultError> {
    let trimmed = req.dest_path.trim();
    if trimmed.is_empty() {
        return Err(VaultError::InvalidInput(
            "書き出し先のファイルパスを指定してください".to_string(),
        ));
    }
    let dest_path = Path::new(trimmed);
    if paths_refer_to_same_file(dest_path, db.path()) {
        return Err(VaultError::InvalidInput(
            "現在開いている保管庫と同じファイルへは書き出せません".to_string(),
        ));
    }
    if dest_path.exists() && !req.overwrite {
        return Err(VaultError::VaultAlreadyExists(
            dest_path.display().to_string(),
        ));
    }

    let meta = db.get_export_item_meta(&req.item_id)?;
    if meta.deleted_at.is_some() {
        return Err(VaultError::InvalidInput(
            "ゴミ箱内のアイテムは書き出せません".to_string(),
        ));
    }

    let written = stream_item_via_temp_file(db, &meta.id, dest_path, req.overwrite)?;
    Ok(PlainExportReport {
        file_count: 1,
        group_count: 0,
        folder_count: 0,
        total_bytes: written,
        dest_path: dest_path.display().to_string(),
    })
}

fn unique_disk_path(root: &Path, rel_segments: &[String], vault_path: &Path) -> PathBuf {
    let mut dir = root.to_path_buf();
    if rel_segments.len() > 1 {
        for seg in &rel_segments[..rel_segments.len() - 1] {
            dir.push(seg);
        }
    }
    let filename = rel_segments
        .last()
        .map(String::as_str)
        .unwrap_or("export.bin");

    let direct = dir.join(filename);
    if !direct.exists() && !paths_refer_to_same_file(&direct, vault_path) {
        return direct;
    }

    let (stem, ext_with_dot) = match filename.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (filename, String::new()),
    };
    let mut idx = 2usize;
    loop {
        let candidate = dir.join(format!("{stem} ({idx}){ext_with_dot}"));
        if !candidate.exists() && !paths_refer_to_same_file(&candidate, vault_path) {
            return candidate;
        }
        idx += 1;
    }
}

pub fn export_plain_batch(
    db: &VaultDb,
    req: &PlainExportBatchRequest,
) -> Result<PlainExportReport, VaultError> {
    let trimmed_dir = req.dest_dir.trim();
    if trimmed_dir.is_empty() {
        return Err(VaultError::InvalidInput(
            "書き出し先のフォルダを指定してください".to_string(),
        ));
    }
    let dest_dir = Path::new(trimmed_dir);
    if !dest_dir.exists() || !dest_dir.is_dir() {
        return Err(VaultError::InvalidInput(
            "指定された書き出し先フォルダが見つかりません".to_string(),
        ));
    }

    let plan = build_plain_export_plan(db, &req.entries, &req.folder_ids, req.include_subfolders)?;
    let mut total_written = 0u64;

    for file_entry in &plan.files {
        let out_path = unique_disk_path(dest_dir, &file_entry.rel_segments, db.path());
        let written = stream_item_via_temp_file(db, &file_entry.item_id, &out_path, false)?;
        total_written = total_written.saturating_add(written);
    }

    Ok(PlainExportReport {
        file_count: plan.files.len(),
        group_count: plan.group_count,
        folder_count: plan.folder_count,
        total_bytes: total_written,
        dest_path: dest_dir.display().to_string(),
    })
}

fn unix_secs_to_utc_ymdhms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hh = (rem / 3600) as u32;
    let mm = ((rem % 3600) / 60) as u32;
    let ss = (rem % 60) as u32;

    // Howard Hinnant's civil-from-days algorithm
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (y + if m <= 2 { 1 } else { 0 }).max(1980) as u32;
    (year, m as u32, d as u32, hh, mm, ss)
}

fn current_dos_time_date() -> (u16, u16) {
    let secs = (now_ms().max(0) as u64) / 1000;
    let (year, month, day, hh, mm, ss) = unix_secs_to_utc_ymdhms(secs);
    let clamped_year = year.clamp(1980, 2107);
    let dos_time = ((hh as u16) << 11) | ((mm as u16) << 5) | ((ss / 2) as u16);
    let dos_date = (((clamped_year - 1980) as u16) << 9) | ((month as u16) << 5) | (day as u16);
    (dos_time, dos_date)
}

pub fn compute_zip_archive_size(plan: &PlainExportPlan) -> u64 {
    let mut offset = 0u64;
    let mut cd_size = 0u64;

    for entry in &plan.files {
        let name_len = entry.zip_entry_name().len() as u64;
        let need_size64 = entry.byte_size >= u64::from(u32::MAX);
        let lfh_extra_len = if need_size64 { 20u64 } else { 0u64 };
        let dd_len = if need_size64 { 24u64 } else { 16u64 };

        let local_offset = offset;
        offset = offset
            .saturating_add(30)
            .saturating_add(name_len)
            .saturating_add(lfh_extra_len)
            .saturating_add(entry.byte_size)
            .saturating_add(dd_len);

        let need_offset64 = local_offset >= u64::from(u32::MAX);
        let cd_extra_len = if need_size64 || need_offset64 {
            4 + if need_size64 { 16 } else { 0 } + if need_offset64 { 8 } else { 0 }
        } else {
            0u64
        };
        cd_size = cd_size
            .saturating_add(46)
            .saturating_add(name_len)
            .saturating_add(cd_extra_len);
    }

    let need_eocd64 = plan.files.len() >= 0xFFFF
        || cd_size >= u64::from(u32::MAX)
        || offset >= u64::from(u32::MAX);
    let eocd64_overhead = if need_eocd64 { 56 + 20 } else { 0 };
    offset
        .saturating_add(cd_size)
        .saturating_add(eocd64_overhead)
        .saturating_add(22)
}

struct ZipCdRecord {
    name: Vec<u8>,
    crc32: u32,
    size: u64,
    local_header_offset: u64,
    need_size64: bool,
}

pub struct ZipStoreStreamWriter<F> {
    emit: F,
    dos_time: u16,
    dos_date: u16,
    offset: u64,
    cd_records: Vec<ZipCdRecord>,
}

impl<F> ZipStoreStreamWriter<F>
where
    F: FnMut(Bytes) -> Result<(), io::Error>,
{
    pub fn new(emit: F) -> Self {
        let (dos_time, dos_date) = current_dos_time_date();
        Self {
            emit,
            dos_time,
            dos_date,
            offset: 0,
            cd_records: Vec::new(),
        }
    }

    fn write_bytes(&mut self, buf: Bytes) -> Result<(), io::Error> {
        self.offset = self.offset.saturating_add(buf.len() as u64);
        (self.emit)(buf)
    }

    pub fn write_entry_header(
        &mut self,
        name_str: &str,
        expected_size: u64,
    ) -> Result<(), io::Error> {
        let name = name_str.as_bytes();
        let need_size64 = expected_size >= u64::from(u32::MAX);
        let version_needed = if need_size64 {
            ZIP_VERSION_45
        } else {
            ZIP_VERSION_20
        };
        let extra_len: u16 = if need_size64 { 20 } else { 0 };

        let mut hdr = Vec::with_capacity(30 + name.len() + usize::from(extra_len));
        hdr.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        hdr.extend_from_slice(&version_needed.to_le_bytes());
        hdr.extend_from_slice(&ZIP_GPBF_DATA_DESC_UTF8.to_le_bytes());
        hdr.extend_from_slice(&ZIP_METHOD_STORE.to_le_bytes());
        hdr.extend_from_slice(&self.dos_time.to_le_bytes());
        hdr.extend_from_slice(&self.dos_date.to_le_bytes());
        hdr.extend_from_slice(&0u32.to_le_bytes()); // CRC32 (in Data Descriptor)
        hdr.extend_from_slice(&0u32.to_le_bytes()); // Compressed size (in Data Descriptor)
        hdr.extend_from_slice(&0u32.to_le_bytes()); // Uncompressed size (in Data Descriptor)
        hdr.extend_from_slice(&(name.len() as u16).to_le_bytes());
        hdr.extend_from_slice(&extra_len.to_le_bytes());
        hdr.extend_from_slice(name);
        if need_size64 {
            hdr.extend_from_slice(&0x0001u16.to_le_bytes());
            hdr.extend_from_slice(&16u16.to_le_bytes());
            hdr.extend_from_slice(&0u64.to_le_bytes());
            hdr.extend_from_slice(&0u64.to_le_bytes());
        }

        let local_header_offset = self.offset;
        self.cd_records.push(ZipCdRecord {
            name: name.to_vec(),
            crc32: 0,
            size: 0,
            local_header_offset,
            need_size64,
        });
        self.write_bytes(Bytes::from(hdr))
    }

    pub fn write_entry_data(&mut self, chunk: Bytes) -> Result<(), io::Error> {
        if chunk.is_empty() {
            return Ok(());
        }
        if let Some(rec) = self.cd_records.last_mut() {
            rec.crc32 = crc32_update(rec.crc32, &chunk);
            rec.size = rec.size.saturating_add(chunk.len() as u64);
        }
        self.write_bytes(chunk)
    }

    pub fn finish_entry(&mut self) -> Result<(), io::Error> {
        let (crc32, size, need_size64) = self
            .cd_records
            .last()
            .map(|r| (r.crc32, r.size, r.need_size64))
            .ok_or_else(|| io::Error::other("no active zip entry"))?;

        let mut dd = Vec::with_capacity(if need_size64 { 24 } else { 16 });
        dd.extend_from_slice(&0x0807_4B50u32.to_le_bytes());
        dd.extend_from_slice(&crc32.to_le_bytes());
        if need_size64 {
            dd.extend_from_slice(&size.to_le_bytes());
            dd.extend_from_slice(&size.to_le_bytes());
        } else {
            let s32 = size as u32;
            dd.extend_from_slice(&s32.to_le_bytes());
            dd.extend_from_slice(&s32.to_le_bytes());
        }
        self.write_bytes(Bytes::from(dd))
    }

    pub fn finish_archive(mut self) -> Result<(), io::Error> {
        let cd_offset = self.offset;
        let entry_count = self.cd_records.len();
        let mut cd_buf = Vec::new();

        for rec in &self.cd_records {
            let need_offset64 = rec.local_header_offset >= u64::from(u32::MAX);
            let need_zip64 = rec.need_size64 || need_offset64;
            let version_needed = if need_zip64 {
                ZIP_VERSION_45
            } else {
                ZIP_VERSION_20
            };

            let extra_payload_len: u16 =
                (if rec.need_size64 { 16 } else { 0 }) + (if need_offset64 { 8 } else { 0 });
            let extra_len: u16 = if extra_payload_len > 0 {
                4 + extra_payload_len
            } else {
                0
            };

            let comp_u32 = if rec.need_size64 {
                u32::MAX
            } else {
                rec.size as u32
            };
            let off_u32 = if need_offset64 {
                u32::MAX
            } else {
                rec.local_header_offset as u32
            };

            cd_buf.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_UNIX_VERSION_MADE_BY.to_le_bytes());
            cd_buf.extend_from_slice(&version_needed.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_GPBF_DATA_DESC_UTF8.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_METHOD_STORE.to_le_bytes());
            cd_buf.extend_from_slice(&self.dos_time.to_le_bytes());
            cd_buf.extend_from_slice(&self.dos_date.to_le_bytes());
            cd_buf.extend_from_slice(&rec.crc32.to_le_bytes());
            cd_buf.extend_from_slice(&comp_u32.to_le_bytes());
            cd_buf.extend_from_slice(&comp_u32.to_le_bytes());
            cd_buf.extend_from_slice(&(rec.name.len() as u16).to_le_bytes());
            cd_buf.extend_from_slice(&extra_len.to_le_bytes());
            cd_buf.extend_from_slice(&0u16.to_le_bytes()); // file comment len
            cd_buf.extend_from_slice(&0u16.to_le_bytes()); // disk number start
            cd_buf.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            cd_buf.extend_from_slice(&ZIP_UNIX_REGULAR_FILE_ATTRS.to_le_bytes());
            cd_buf.extend_from_slice(&off_u32.to_le_bytes());
            cd_buf.extend_from_slice(&rec.name);
            if extra_len > 0 {
                cd_buf.extend_from_slice(&0x0001u16.to_le_bytes());
                cd_buf.extend_from_slice(&extra_payload_len.to_le_bytes());
                if rec.need_size64 {
                    cd_buf.extend_from_slice(&rec.size.to_le_bytes());
                    cd_buf.extend_from_slice(&rec.size.to_le_bytes());
                }
                if need_offset64 {
                    cd_buf.extend_from_slice(&rec.local_header_offset.to_le_bytes());
                }
            }
        }

        let cd_size = cd_buf.len() as u64;
        let need_eocd64 = entry_count >= 0xFFFF
            || cd_size >= u64::from(u32::MAX)
            || cd_offset >= u64::from(u32::MAX);

        if need_eocd64 {
            let eocd64_offset = cd_offset.saturating_add(cd_size);
            // ZIP64 End of Central Directory Record (56 bytes)
            cd_buf.extend_from_slice(&0x0606_4B50u32.to_le_bytes());
            cd_buf.extend_from_slice(&44u64.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_UNIX_VERSION_MADE_BY.to_le_bytes());
            cd_buf.extend_from_slice(&ZIP_VERSION_45.to_le_bytes());
            cd_buf.extend_from_slice(&0u32.to_le_bytes());
            cd_buf.extend_from_slice(&0u32.to_le_bytes());
            cd_buf.extend_from_slice(&(entry_count as u64).to_le_bytes());
            cd_buf.extend_from_slice(&(entry_count as u64).to_le_bytes());
            cd_buf.extend_from_slice(&cd_size.to_le_bytes());
            cd_buf.extend_from_slice(&cd_offset.to_le_bytes());

            // ZIP64 End of Central Directory Locator (20 bytes)
            cd_buf.extend_from_slice(&0x0706_4B50u32.to_le_bytes());
            cd_buf.extend_from_slice(&0u32.to_le_bytes());
            cd_buf.extend_from_slice(&eocd64_offset.to_le_bytes());
            cd_buf.extend_from_slice(&1u32.to_le_bytes());
        }

        let entries_u16 = entry_count.min(0xFFFF) as u16;
        let cd_size_u32 = cd_size.min(u64::from(u32::MAX)) as u32;
        let cd_offset_u32 = cd_offset.min(u64::from(u32::MAX)) as u32;

        // Standard End of Central Directory Record (22 bytes)
        cd_buf.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        cd_buf.extend_from_slice(&0u16.to_le_bytes());
        cd_buf.extend_from_slice(&0u16.to_le_bytes());
        cd_buf.extend_from_slice(&entries_u16.to_le_bytes());
        cd_buf.extend_from_slice(&entries_u16.to_le_bytes());
        cd_buf.extend_from_slice(&cd_size_u32.to_le_bytes());
        cd_buf.extend_from_slice(&cd_offset_u32.to_le_bytes());
        cd_buf.extend_from_slice(&0u16.to_le_bytes());

        self.write_bytes(Bytes::from(cd_buf))
    }
}

fn pump_item_chunks<F>(
    db_arc: &Arc<Mutex<Option<VaultDb>>>,
    item_id: &str,
    byte_size: u64,
    chunk_size: usize,
    mut on_chunk: F,
) -> Result<(), io::Error>
where
    F: FnMut(Bytes) -> Result<(), io::Error>,
{
    if chunk_size == 0 {
        let bytes = {
            let guard = db_arc
                .lock()
                .map_err(|_| io::Error::other("vault lock poisoned"))?;
            let db = guard
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "vault locked"))?;
            db.get_blob_bytes(item_id)
                .map_err(|e| io::Error::other(e.to_string()))?
        };
        if !bytes.is_empty() {
            on_chunk(Bytes::from(bytes))?;
        }
        return Ok(());
    }

    let total = usize::try_from(byte_size)
        .map_err(|_| io::Error::other("item exceeds addressable range"))?;
    let mut offset = 0usize;
    while offset < total {
        let end = offset
            .saturating_add(STREAM_CHUNK_BYTES)
            .min(total)
            .saturating_sub(1);
        let chunk = {
            let guard = db_arc
                .lock()
                .map_err(|_| io::Error::other("vault lock poisoned"))?;
            let db = guard
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "vault locked"))?;
            db.read_media_range(item_id, offset, end, chunk_size)
                .map_err(|e| io::Error::other(e.to_string()))?
        };
        if chunk.is_empty() {
            break;
        }
        let read_len = chunk.len();
        on_chunk(Bytes::from(chunk))?;
        offset = offset.saturating_add(read_len);
    }
    Ok(())
}

pub fn stream_single_item_to_channel(
    db_arc: Arc<Mutex<Option<VaultDb>>>,
    meta: ExportItemMeta,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) {
    let res = pump_item_chunks(
        &db_arc,
        &meta.id,
        meta.byte_size,
        meta.chunk_size,
        |chunk| {
            tx.blocking_send(Ok(chunk))
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client disconnected"))
        },
    );
    if let Err(err) = res {
        let _ = tx.blocking_send(Err(err));
    }
}

pub fn stream_zip_plan_to_channel(
    db_arc: Arc<Mutex<Option<VaultDb>>>,
    plan: PlainExportPlan,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) {
    let res = (|| -> Result<(), io::Error> {
        let mut zip = ZipStoreStreamWriter::new(|chunk| {
            tx.blocking_send(Ok(chunk))
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client disconnected"))
        });
        for file in &plan.files {
            let entry_name = file.zip_entry_name();
            zip.write_entry_header(&entry_name, file.byte_size)?;
            pump_item_chunks(
                &db_arc,
                &file.item_id,
                file.byte_size,
                file.chunk_size,
                |chunk| zip.write_entry_data(chunk),
            )?;
            zip.finish_entry()?;
        }
        zip.finish_archive()?;
        Ok(())
    })();

    if let Err(err) = res {
        let _ = tx.blocking_send(Err(err));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::sha256_hex;
    use crate::db::ImportOutcome;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;
    use tempfile::TempDir;

    fn make_png(w: u32, h: u32, color: [u8; 4]) -> Vec<u8> {
        let mut img = RgbaImage::new(w, h);
        for (_, _, px) in img.enumerate_pixels_mut() {
            *px = Rgba(color);
        }
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    fn make_jpeg(w: u32, h: u32, color: [u8; 3]) -> Vec<u8> {
        let mut img = image::RgbImage::new(w, h);
        for (_, _, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb(color);
        }
        let mut out = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
            .unwrap();
        out
    }

    fn make_fake_mp4(size: usize) -> Vec<u8> {
        let mut data = Vec::with_capacity(size);
        data.extend_from_slice(&20u32.to_be_bytes());
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(b"isom");
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(b"isom");
        while data.len() < size {
            let chunk = ((data.len() * 131) % 251) as u8;
            data.push(chunk);
        }
        data.truncate(size);
        data
    }

    fn import_item(db: &mut VaultDb, bytes: &[u8], title: Option<&str>) -> String {
        match db.import_bytes(bytes, title).unwrap() {
            ImportOutcome::Imported { id, .. } => id,
            ImportOutcome::Duplicate => panic!("unexpected duplicate"),
        }
    }

    fn parse_zip_store(archive: &[u8]) -> HashMap<String, Vec<u8>> {
        // Locate EOCD scanning backwards
        let eocd_pos = (0..=archive.len().saturating_sub(22))
            .rev()
            .find(|&i| &archive[i..i + 4] == b"PK\x05\x06")
            .expect("EOCD signature missing");
        let total_entries = u16::from_le_bytes([archive[eocd_pos + 10], archive[eocd_pos + 11]]);
        let cd_size = u32::from_le_bytes(archive[eocd_pos + 12..eocd_pos + 16].try_into().unwrap());
        let cd_off = u32::from_le_bytes(archive[eocd_pos + 16..eocd_pos + 20].try_into().unwrap());
        assert_eq!(cd_off + cd_size, eocd_pos as u32);

        let mut map = HashMap::new();
        let mut cursor = cd_off as usize;
        for _ in 0..total_entries {
            assert_eq!(&archive[cursor..cursor + 4], b"PK\x01\x02");
            let gpbf = u16::from_le_bytes([archive[cursor + 8], archive[cursor + 9]]);
            assert_eq!(gpbf, ZIP_GPBF_DATA_DESC_UTF8);
            let method = u16::from_le_bytes([archive[cursor + 10], archive[cursor + 11]]);
            assert_eq!(method, 0);
            let cd_crc = u32::from_le_bytes(archive[cursor + 16..cursor + 20].try_into().unwrap());
            let cd_comp = u32::from_le_bytes(archive[cursor + 20..cursor + 24].try_into().unwrap());
            let cd_uncomp =
                u32::from_le_bytes(archive[cursor + 24..cursor + 28].try_into().unwrap());
            assert_eq!(cd_comp, cd_uncomp);
            let name_len =
                u16::from_le_bytes([archive[cursor + 28], archive[cursor + 29]]) as usize;
            let extra_len =
                u16::from_le_bytes([archive[cursor + 30], archive[cursor + 31]]) as usize;
            let comment_len =
                u16::from_le_bytes([archive[cursor + 32], archive[cursor + 33]]) as usize;
            let lfh_off =
                u32::from_le_bytes(archive[cursor + 42..cursor + 46].try_into().unwrap()) as usize;
            let name = std::str::from_utf8(&archive[cursor + 46..cursor + 46 + name_len])
                .unwrap()
                .to_string();
            cursor += 46 + name_len + extra_len + comment_len;

            assert_eq!(&archive[lfh_off..lfh_off + 4], b"PK\x03\x04");
            let lfh_name_len =
                u16::from_le_bytes([archive[lfh_off + 26], archive[lfh_off + 27]]) as usize;
            let lfh_extra_len =
                u16::from_le_bytes([archive[lfh_off + 28], archive[lfh_off + 29]]) as usize;
            let data_start = lfh_off + 30 + lfh_name_len + lfh_extra_len;
            let data_end = data_start + (cd_comp as usize);
            let payload = archive[data_start..data_end].to_vec();
            assert_eq!(crc32_update(0, &payload), cd_crc);

            // Check Data Descriptor
            assert_eq!(&archive[data_end..data_end + 4], b"PK\x07\x08");
            let dd_crc =
                u32::from_le_bytes(archive[data_end + 4..data_end + 8].try_into().unwrap());
            let dd_size =
                u32::from_le_bytes(archive[data_end + 8..data_end + 12].try_into().unwrap());
            assert_eq!(dd_crc, cd_crc);
            assert_eq!(dd_size, cd_comp);

            map.insert(name, payload);
        }
        map
    }

    #[test]
    fn test_filename_sanitization_and_extensions() {
        assert_eq!(mime_to_extension("image/jpeg"), "jpg");
        assert_eq!(mime_to_extension("video/quicktime"), "mov");
        assert_eq!(
            sanitize_name_segment(".. / \\ : * ? \" < > |", "fb"),
            "_ _ _ _ _ _ _ _ _"
        );
        assert_eq!(sanitize_name_segment("...", "fb"), "fb");
        assert_eq!(sanitize_name_segment("..", "fb"), "fb");
        assert_eq!(sanitize_name_segment("con.txt", "fb"), "_con.txt");
        assert_eq!(sanitize_name_segment("LPT1", "fb"), "_LPT1");
        assert_eq!(
            build_item_filename("vacation.jpeg", "12345678-aaaa", "image/jpeg"),
            "vacation.jpg"
        );
        assert_eq!(
            build_item_filename("日本語イラスト", "12345678-aaaa", "image/png"),
            "日本語イラスト.png"
        );
        assert_eq!(
            build_item_filename("   ", "abcdef12-3456", "image/webp"),
            "item_abcdef12.webp"
        );
        assert_eq!(build_group_page_filename(1, 20, "", "image/png"), "001.png");
        assert_eq!(
            build_group_page_filename(12, 1200, "cover.png", "image/png"),
            "0012_cover.png"
        );

        let cd = content_disposition_attachment("漫画 \"1\".zip");
        assert!(cd.starts_with("attachment; filename=\""));
        assert!(cd.contains("filename*=UTF-8''"));
    }

    #[test]
    fn test_export_plain_item_and_chunked_video_and_vault_overwrite_guard() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("export_single.vault");
        let mut db = VaultDb::create(&vault_path, "pass-1234", false).unwrap();

        let png = make_png(20, 16, [255, 80, 40, 255]);
        let png_id = import_item(&mut db, &png, Some("sample_png"));

        let vid_bytes = make_fake_mp4(150_000);
        let vid_path = dir.path().join("chunked_src.mp4");
        fs::write(&vid_path, &vid_bytes).unwrap();
        let vid_id = match db
            .import_media_file_with_chunk_params(
                &vid_path,
                Some("chunked_movie"),
                None,
                5000,
                64 * 1024,
                32 * 1024,
            )
            .unwrap()
        {
            ImportOutcome::Imported { id, .. } => id,
            ImportOutcome::Duplicate => panic!("unexpected dup"),
        };

        // Vault self-overwrite is forbidden
        let self_err = export_plain_item(
            &db,
            &PlainExportItemRequest {
                item_id: png_id.clone(),
                dest_path: vault_path.to_string_lossy().to_string(),
                overwrite: true,
            },
        );
        assert!(matches!(self_err, Err(VaultError::InvalidInput(_))));

        // Single PNG export matches exact bytes
        let png_out = dir.path().join("out/pic.png");
        let rep1 = export_plain_item(
            &db,
            &PlainExportItemRequest {
                item_id: png_id.clone(),
                dest_path: png_out.to_string_lossy().to_string(),
                overwrite: false,
            },
        )
        .unwrap();
        assert_eq!(rep1.file_count, 1);
        assert_eq!(fs::read(&png_out).unwrap(), png);

        // Existing file rejected unless overwrite = true
        let dup_err = export_plain_item(
            &db,
            &PlainExportItemRequest {
                item_id: png_id,
                dest_path: png_out.to_string_lossy().to_string(),
                overwrite: false,
            },
        );
        assert!(matches!(dup_err, Err(VaultError::VaultAlreadyExists(_))));

        // Chunked video export matches SHA-256 bit-for-bit
        let vid_out = dir.path().join("out/movie.mp4");
        let rep2 = export_plain_item(
            &db,
            &PlainExportItemRequest {
                item_id: vid_id,
                dest_path: vid_out.to_string_lossy().to_string(),
                overwrite: false,
            },
        )
        .unwrap();
        assert_eq!(rep2.total_bytes, vid_bytes.len() as u64);
        assert_eq!(
            sha256_hex(&fs::read(&vid_out).unwrap()),
            sha256_hex(&vid_bytes)
        );
    }

    #[test]
    fn test_export_plain_batch_manga_folders_collisions_and_zip_stream() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("export_batch.vault");
        let mut db = VaultDb::create(&vault_path, "pass-5678", false).unwrap();

        let p1 = make_png(12, 12, [10, 20, 30, 255]);
        let p2 = make_png(14, 14, [40, 50, 60, 255]);
        let j1 = make_jpeg(16, 16, [200, 100, 50]);
        let j2 = make_jpeg(18, 18, [220, 120, 70]);
        let trash_png = make_png(10, 10, [1, 2, 3, 255]);

        let p1_id = import_item(&mut db, &p1, Some("扉絵"));
        let p2_id = import_item(&mut db, &p2, Some(""));
        let j1_id = import_item(&mut db, &j1, Some("Photo"));
        let j2_id = import_item(&mut db, &j2, Some("photo"));
        let trash_id = import_item(&mut db, &trash_png, Some("trashed"));

        let group = db
            .create_group_from_items(&[p1_id, p2_id], Some("作品タイトル"), None)
            .unwrap();
        let root_folder = db.create_folder(None, "旅行", "").unwrap();
        let child_folder = db.create_folder(Some(&root_folder.id), "沖縄", "").unwrap();

        db.add_entries_to_folder(
            &root_folder.id,
            &[
                EntryRef {
                    kind: EntryKind::Image,
                    id: j1_id.clone(),
                },
                EntryRef {
                    kind: EntryKind::Image,
                    id: trash_id.clone(),
                },
            ],
        )
        .unwrap();
        db.add_entries_to_folder(
            &child_folder.id,
            &[
                EntryRef {
                    kind: EntryKind::Image,
                    id: j2_id.clone(),
                },
                EntryRef {
                    kind: EntryKind::Group,
                    id: group.id.clone(),
                },
            ],
        )
        .unwrap();
        db.move_entries_to_trash(&[EntryRef {
            kind: EntryKind::Image,
            id: trash_id,
        }])
        .unwrap();

        // 1. Export folder WITH subfolders -> hierarchy + group + excluded trash
        let out_with_sub = dir.path().join("batch_with_sub");
        fs::create_dir_all(&out_with_sub).unwrap();
        let rep_sub = export_plain_batch(
            &db,
            &PlainExportBatchRequest {
                dest_dir: out_with_sub.to_string_lossy().to_string(),
                entries: vec![],
                folder_ids: vec![root_folder.id.clone()],
                include_subfolders: true,
            },
        )
        .unwrap();
        assert_eq!(rep_sub.file_count, 4);
        assert_eq!(rep_sub.group_count, 1);
        assert_eq!(rep_sub.folder_count, 2);
        assert_eq!(fs::read(out_with_sub.join("旅行/Photo.jpg")).unwrap(), j1);
        assert_eq!(
            fs::read(out_with_sub.join("旅行/沖縄/photo.jpg")).unwrap(),
            j2
        );
        assert_eq!(
            fs::read(out_with_sub.join("旅行/沖縄/作品タイトル/001_扉絵.png")).unwrap(),
            p1
        );
        assert_eq!(
            fs::read(out_with_sub.join("旅行/沖縄/作品タイトル/002.png")).unwrap(),
            p2
        );
        assert!(!out_with_sub.join("旅行/trashed.png").exists());

        // 2. Export folder WITHOUT subfolders -> only Photo.jpg
        let out_no_sub = dir.path().join("batch_no_sub");
        fs::create_dir_all(&out_no_sub).unwrap();
        let rep_nosub = export_plain_batch(
            &db,
            &PlainExportBatchRequest {
                dest_dir: out_no_sub.to_string_lossy().to_string(),
                entries: vec![],
                folder_ids: vec![root_folder.id.clone()],
                include_subfolders: false,
            },
        )
        .unwrap();
        assert_eq!(rep_nosub.file_count, 1);
        assert!(out_no_sub.join("旅行/Photo.jpg").exists());
        assert!(!out_no_sub.join("旅行/沖縄").exists());

        // 3. Collision test when both Photo and photo exported together at root
        let out_collide = dir.path().join("batch_collide");
        fs::create_dir_all(&out_collide).unwrap();
        export_plain_batch(
            &db,
            &PlainExportBatchRequest {
                dest_dir: out_collide.to_string_lossy().to_string(),
                entries: vec![
                    EntryRef {
                        kind: EntryKind::Image,
                        id: j1_id.clone(),
                    },
                    EntryRef {
                        kind: EntryKind::Image,
                        id: j2_id.clone(),
                    },
                ],
                folder_ids: vec![],
                include_subfolders: true,
            },
        )
        .unwrap();
        assert_eq!(fs::read(out_collide.join("Photo.jpg")).unwrap(), j1);
        assert_eq!(fs::read(out_collide.join("photo (2).jpg")).unwrap(), j2);

        // 4. ZIP STORE streaming matches computed size & parses back cleanly
        let plan = build_plain_export_plan(&db, &[], &[root_folder.id], true).unwrap();
        let predicted_size = compute_zip_archive_size(&plan);

        let state = Arc::new(Mutex::new(Some(db)));
        let (tx, mut rx) = mpsc::channel(8);
        let plan_clone = plan.clone();
        std::thread::spawn(move || {
            stream_zip_plan_to_channel(state, plan_clone, tx);
        });
        let mut archive = Vec::new();
        while let Some(item) = rx.blocking_recv() {
            archive.extend_from_slice(&item.unwrap());
        }
        assert_eq!(archive.len() as u64, predicted_size);

        let parsed = parse_zip_store(&archive);
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed.get("旅行/Photo.jpg").unwrap(), &j1);
        assert_eq!(parsed.get("旅行/沖縄/photo.jpg").unwrap(), &j2);
        assert_eq!(
            parsed.get("旅行/沖縄/作品タイトル/001_扉絵.png").unwrap(),
            &p1
        );
        assert_eq!(parsed.get("旅行/沖縄/作品タイトル/002.png").unwrap(), &p2);
    }
}
