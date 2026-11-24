use std::collections::HashSet;
use std::fs;
use std::io::{BufReader, Write};
use std::path::Path;

use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::crypto::{normalize_nfc_trimmed, sha256_file_hex, sha256_hex};
use crate::error::VaultError;
use crate::imaging::{
    detect_mime, is_video_mime, process_client_png_thumbnail, process_image_bytes,
};
use crate::models::{
    BatchRatingReport, BatchTagReport, EntryKind, EntryRef, ItemDetail, TagWithCount,
};

use super::schema::{BLOB_CHUNK_SIZE, CHUNK_STORAGE_THRESHOLD};
use super::{
    now_ms, parse_palette, prune_orphan_tags, read_up_to, upsert_tag, ImportOutcome, VaultDb,
};

#[derive(Debug, Clone)]
pub struct ExportItemMeta {
    pub id: String,
    pub title: String,
    pub mime_type: String,
    pub byte_size: u64,
    pub chunk_size: usize,
    pub deleted_at: Option<i64>,
}

impl VaultDb {
    pub fn import_bytes(
        &mut self,
        bytes: &[u8],
        default_title: Option<&str>,
    ) -> Result<ImportOutcome, VaultError> {
        self.import_media_bytes(bytes, default_title, None, 0)
    }

    pub fn import_media_bytes(
        &mut self,
        bytes: &[u8],
        default_title: Option<&str>,
        source_url: Option<&str>,
        duration_ms: i64,
    ) -> Result<ImportOutcome, VaultError> {
        let sha = sha256_hex(bytes);
        let clean_source = source_url.map(|s| s.trim().to_string()).unwrap_or_default();
        let existing: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT id, source_url FROM items WHERE sha256 = ?1",
                params![sha],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        if let Some((existing_id, existing_url)) = existing {
            if !clean_source.is_empty() && existing_url.trim().is_empty() {
                let _ = self.conn.execute(
                    "UPDATE items SET source_url = ?2, updated_at = ?3 WHERE id = ?1",
                    params![existing_id, clean_source, now_ms()],
                );
            }
            return Ok(ImportOutcome::Duplicate);
        }

        let processed = process_image_bytes(bytes)?;
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        let title = default_title.map(normalize_nfc_trimmed).unwrap_or_default();
        let palette_json =
            serde_json::to_string(&processed.palette).unwrap_or_else(|_| "[]".to_string());
        let byte_size = bytes.len() as i64;
        let thumb_pending = processed.thumbnail.is_none();
        let safe_duration = duration_ms.max(0);

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO items
                (id, title, sha256, mime_type, width, height, byte_size, duration_ms, source_url, rating, memo, palette, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, '', ?10, ?11, ?11, NULL)",
            params![
                id,
                title,
                sha,
                processed.mime_type,
                processed.width,
                processed.height,
                byte_size,
                safe_duration,
                clean_source,
                palette_json,
                now,
            ],
        )?;

        tx.execute(
            "INSERT INTO blobs (item_id, data) VALUES (?1, ?2)",
            params![id, bytes],
        )?;

        if let Some(thumb) = processed.thumbnail {
            tx.execute(
                "INSERT INTO thumbnails (item_id, mime_type, width, height, data)
                 VALUES (?1, 'image/webp', ?2, ?3, ?4)",
                params![id, thumb.width, thumb.height, thumb.data],
            )?;
        }

        tx.commit()?;
        Ok(ImportOutcome::Imported {
            id,
            mime_type: processed.mime_type.to_string(),
            thumb_pending,
        })
    }

    pub fn import_media_file(
        &mut self,
        path: &Path,
        default_title: Option<&str>,
        source_url: Option<&str>,
        duration_ms: i64,
    ) -> Result<ImportOutcome, VaultError> {
        self.import_media_file_with_chunk_params(
            path,
            default_title,
            source_url,
            duration_ms,
            CHUNK_STORAGE_THRESHOLD,
            BLOB_CHUNK_SIZE,
        )
    }

    pub(crate) fn import_media_file_with_chunk_params(
        &mut self,
        path: &Path,
        default_title: Option<&str>,
        source_url: Option<&str>,
        duration_ms: i64,
        chunk_threshold: u64,
        chunk_size: usize,
    ) -> Result<ImportOutcome, VaultError> {
        let file_len = fs::metadata(path)?.len();
        if file_len <= chunk_threshold {
            let bytes = fs::read(path)?;
            return self.import_media_bytes(&bytes, default_title, source_url, duration_ms);
        }

        let mut head = [0u8; 64];
        let head_len = {
            let mut file = fs::File::open(path)?;
            read_up_to(&mut file, &mut head)?
        };
        let mime_type = detect_mime(&head[..head_len])?;
        if !is_video_mime(mime_type) {
            if file_len <= 512 * 1024 * 1024 {
                let bytes = fs::read(path)?;
                return self.import_media_bytes(&bytes, default_title, source_url, duration_ms);
            }
            return Err(VaultError::InvalidInput(
                "画像ファイルが大きすぎます（512 MB まで）".to_string(),
            ));
        }

        let sha = sha256_file_hex(path)?;
        let clean_source = source_url.map(|s| s.trim().to_string()).unwrap_or_default();
        let existing: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT id, source_url FROM items WHERE sha256 = ?1",
                params![sha],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        if let Some((existing_id, existing_url)) = existing {
            if !clean_source.is_empty() && existing_url.trim().is_empty() {
                let _ = self.conn.execute(
                    "UPDATE items SET source_url = ?2, updated_at = ?3 WHERE id = ?1",
                    params![existing_id, clean_source, now_ms()],
                );
            }
            return Ok(ImportOutcome::Duplicate);
        }

        let effective_chunk_size = chunk_size.max(4096);
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        let title = default_title.map(normalize_nfc_trimmed).unwrap_or_default();
        let safe_duration = duration_ms.max(0);

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO items
                (id, title, sha256, mime_type, width, height, byte_size, chunk_size, duration_ms, source_url, rating, memo, palette, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, ?6, ?7, ?8, 0, '', '[]', ?9, ?9, NULL)",
            params![
                id,
                title,
                sha,
                mime_type,
                file_len as i64,
                effective_chunk_size as i64,
                safe_duration,
                clean_source,
                now,
            ],
        )?;

        let file = fs::File::open(path)?;
        let mut reader = BufReader::with_capacity(effective_chunk_size.min(4 * 1024 * 1024), file);
        let mut buf = vec![0u8; effective_chunk_size];
        let mut chunk_index: i64 = 0;
        loop {
            let n = read_up_to(&mut reader, &mut buf)?;
            if n == 0 {
                break;
            }
            tx.execute(
                "INSERT INTO blob_chunks (item_id, chunk_index, data) VALUES (?1, ?2, ?3)",
                params![id, chunk_index, &buf[..n]],
            )?;
            chunk_index += 1;
        }
        tx.commit()?;

        Ok(ImportOutcome::Imported {
            id,
            mime_type: mime_type.to_string(),
            thumb_pending: true,
        })
    }

    pub fn save_media_thumbnail(
        &mut self,
        item_id: &str,
        png_bytes: &[u8],
        orig_width: i32,
        orig_height: i32,
        duration_ms: i64,
    ) -> Result<ItemDetail, VaultError> {
        let thumb = process_client_png_thumbnail(png_bytes)?;
        let palette_json =
            serde_json::to_string(&thumb.palette).unwrap_or_else(|_| "[]".to_string());
        let now = now_ms();
        let safe_duration = duration_ms.max(0);

        let tx = self.conn.transaction()?;
        let affected = tx.execute(
            "UPDATE items
             SET width = CASE WHEN width = 0 THEN ?2 ELSE width END,
                 height = CASE WHEN height = 0 THEN ?3 ELSE height END,
                 duration_ms = CASE WHEN ?6 > 0 THEN ?6 ELSE duration_ms END,
                 palette = ?4,
                 updated_at = ?5
             WHERE id = ?1",
            params![
                item_id,
                orig_width.max(0),
                orig_height.max(0),
                palette_json,
                now,
                safe_duration
            ],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(item_id.to_string()));
        }

        tx.execute(
            "INSERT INTO thumbnails (item_id, mime_type, width, height, data)
             VALUES (?1, 'image/webp', ?2, ?3, ?4)
             ON CONFLICT(item_id) DO UPDATE SET
                 mime_type = excluded.mime_type,
                 width = excluded.width,
                 height = excluded.height,
                 data = excluded.data",
            params![item_id, thumb.width, thumb.height, thumb.data],
        )?;
        tx.commit()?;

        self.get_item_detail(item_id)
    }

    pub fn get_item_detail(&self, item_id: &str) -> Result<ItemDetail, VaultError> {
        let tags = self.fetch_tags_for_item(item_id)?;
        let folder_ids = self.fetch_folders_for_item(item_id)?;
        let detail = self
            .conn
            .query_row(
                "SELECT id, title, sha256, mime_type, width, height, byte_size, rating,
                        memo, palette, created_at, updated_at, deleted_at,
                        EXISTS(SELECT 1 FROM thumbnails th WHERE th.item_id = items.id) AS has_thumb,
                        duration_ms, source_url
                 FROM items
                 WHERE id = ?1",
                params![item_id],
                |r| {
                    let palette_json: String = r.get(9)?;
                    let has_thumb_int: i64 = r.get(13)?;
                    Ok(ItemDetail {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        sha256: r.get(2)?,
                        mime_type: r.get(3)?,
                        width: r.get(4)?,
                        height: r.get(5)?,
                        byte_size: r.get(6)?,
                        duration_ms: r.get(14)?,
                        source_url: r.get(15)?,
                        rating: r.get(7)?,
                        memo: r.get(8)?,
                        tags: Vec::new(),
                        folder_ids: Vec::new(),
                        palette: parse_palette(&palette_json),
                        has_thumbnail: has_thumb_int != 0,
                        created_at: r.get(10)?,
                        updated_at: r.get(11)?,
                        deleted_at: r.get(12)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| VaultError::NotFound(item_id.to_string()))?;

        Ok(ItemDetail {
            tags,
            folder_ids,
            ..detail
        })
    }

    pub fn get_thumbnail_bytes(&self, item_id: &str) -> Result<Vec<u8>, VaultError> {
        self.conn
            .query_row(
                "SELECT data FROM thumbnails WHERE item_id = ?1",
                params![item_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| VaultError::NotFound(item_id.to_string()))
    }

    pub fn get_media_info(&self, item_id: &str) -> Result<(String, usize, usize), VaultError> {
        let row: Option<(String, i64, i64)> = self
            .conn
            .query_row(
                "SELECT mime_type, byte_size, chunk_size FROM items WHERE id = ?1",
                params![item_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (mime_type, byte_size, chunk_size) =
            row.ok_or_else(|| VaultError::NotFound(item_id.to_string()))?;
        Ok((
            mime_type,
            byte_size.max(0) as usize,
            chunk_size.max(0) as usize,
        ))
    }

    pub fn read_media_range(
        &self,
        item_id: &str,
        start: usize,
        end: usize,
        chunk_size: usize,
    ) -> Result<Vec<u8>, VaultError> {
        if start > end {
            return Ok(Vec::new());
        }
        if chunk_size == 0 {
            let blob = self.get_blob_bytes(item_id)?;
            if start >= blob.len() {
                return Ok(Vec::new());
            }
            let clamped_end = end.min(blob.len() - 1);
            return Ok(blob[start..=clamped_end].to_vec());
        }

        let first_chunk = (start / chunk_size) as i64;
        let last_chunk = (end / chunk_size) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT chunk_index, data
             FROM blob_chunks
             WHERE item_id = ?1 AND chunk_index BETWEEN ?2 AND ?3
             ORDER BY chunk_index ASC",
        )?;
        let rows = stmt.query_map(params![item_id, first_chunk, last_chunk], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;

        let mut out = Vec::with_capacity(end.saturating_sub(start).saturating_add(1));
        for row in rows {
            let (chunk_idx, data) = row?;
            if chunk_idx < 0 || data.is_empty() {
                continue;
            }
            let chunk_start = (chunk_idx as usize).saturating_mul(chunk_size);
            let local_start = start.saturating_sub(chunk_start);
            let local_end_excl = end
                .saturating_add(1)
                .saturating_sub(chunk_start)
                .min(data.len());
            if local_start < local_end_excl {
                out.extend_from_slice(&data[local_start..local_end_excl]);
            }
        }
        Ok(out)
    }

    pub fn get_blob_bytes(&self, item_id: &str) -> Result<Vec<u8>, VaultError> {
        let (_, byte_size, chunk_size) = self.get_media_info(item_id)?;
        if chunk_size == 0 {
            return self
                .conn
                .query_row(
                    "SELECT data FROM blobs WHERE item_id = ?1",
                    params![item_id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| VaultError::NotFound(item_id.to_string()));
        }

        let mut out = Vec::with_capacity(byte_size);
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM blob_chunks WHERE item_id = ?1 ORDER BY chunk_index ASC")?;
        let rows = stmt.query_map(params![item_id], |r| r.get::<_, Vec<u8>>(0))?;
        for row in rows {
            out.extend_from_slice(&row?);
        }
        Ok(out)
    }

    pub fn get_media_blob_with_mime(&self, item_id: &str) -> Result<(String, Vec<u8>), VaultError> {
        let (mime_type, _, _) = self.get_media_info(item_id)?;
        let bytes = self.get_blob_bytes(item_id)?;
        Ok((mime_type, bytes))
    }

    pub fn get_export_item_meta(&self, item_id: &str) -> Result<ExportItemMeta, VaultError> {
        let row: Option<ExportItemMeta> = self
            .conn
            .query_row(
                "SELECT id, title, mime_type, byte_size, chunk_size, deleted_at
                 FROM items
                 WHERE id = ?1",
                params![item_id],
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
        row.ok_or_else(|| VaultError::NotFound(item_id.to_string()))
    }

    pub fn write_blob_to_writer<W: Write>(
        &self,
        item_id: &str,
        writer: &mut W,
    ) -> Result<u64, VaultError> {
        let (_, _, chunk_size) = self.get_media_info(item_id)?;
        if chunk_size == 0 {
            let data: Vec<u8> = self
                .conn
                .query_row(
                    "SELECT data FROM blobs WHERE item_id = ?1",
                    params![item_id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| VaultError::NotFound(item_id.to_string()))?;
            writer.write_all(&data)?;
            return Ok(data.len() as u64);
        }

        let mut total = 0u64;
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM blob_chunks WHERE item_id = ?1 ORDER BY chunk_index ASC")?;
        let rows = stmt.query_map(params![item_id], |r| r.get::<_, Vec<u8>>(0))?;
        for row in rows {
            let chunk = row?;
            writer.write_all(&chunk)?;
            total = total.saturating_add(chunk.len() as u64);
        }
        Ok(total)
    }

    pub fn set_source_url(
        &mut self,
        item_id: &str,
        source_url: &str,
    ) -> Result<ItemDetail, VaultError> {
        let trimmed = source_url.trim();
        let now = now_ms();
        let affected = self.conn.execute(
            "UPDATE items SET source_url = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
            params![item_id, trimmed, now],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(item_id.to_string()));
        }
        self.get_item_detail(item_id)
    }

    pub fn set_title(&mut self, item_id: &str, title: &str) -> Result<ItemDetail, VaultError> {
        let normalized = normalize_nfc_trimmed(title);
        let now = now_ms();
        let affected = self.conn.execute(
            "UPDATE items SET title = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
            params![item_id, normalized, now],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(item_id.to_string()));
        }
        self.get_item_detail(item_id)
    }

    pub fn set_rating(&mut self, item_id: &str, rating: i32) -> Result<ItemDetail, VaultError> {
        if !(0..=5).contains(&rating) {
            return Err(VaultError::InvalidInput(
                "評価は 0〜5 の範囲で指定してください".to_string(),
            ));
        }
        let now = now_ms();
        let affected = self.conn.execute(
            "UPDATE items SET rating = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
            params![item_id, rating, now],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(item_id.to_string()));
        }
        self.get_item_detail(item_id)
    }

    pub fn set_memo(&mut self, item_id: &str, memo: &str) -> Result<ItemDetail, VaultError> {
        let normalized = unicode_normalization::UnicodeNormalization::nfc(memo).collect::<String>();
        let now = now_ms();
        let affected = self.conn.execute(
            "UPDATE items SET memo = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
            params![item_id, normalized, now],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(item_id.to_string()));
        }
        self.get_item_detail(item_id)
    }

    pub fn add_tag(&mut self, item_id: &str, tag_name: &str) -> Result<ItemDetail, VaultError> {
        let normalized = normalize_nfc_trimmed(tag_name);
        if normalized.is_empty() {
            return Err(VaultError::InvalidInput(
                "タグ名を入力してください".to_string(),
            ));
        }

        let tx = self.conn.transaction()?;
        let exists: Option<String> = tx
            .query_row(
                "SELECT id FROM items WHERE id = ?1 AND deleted_at IS NULL",
                params![item_id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(VaultError::NotFound(item_id.to_string()));
        }

        let tag_id = upsert_tag(&tx, &normalized)?;
        tx.execute(
            "INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
            params![item_id, tag_id],
        )?;
        tx.execute(
            "UPDATE items SET updated_at = ?2 WHERE id = ?1",
            params![item_id, now_ms()],
        )?;
        tx.commit()?;

        self.get_item_detail(item_id)
    }

    pub fn remove_tag(&mut self, item_id: &str, tag_name: &str) -> Result<ItemDetail, VaultError> {
        let normalized = normalize_nfc_trimmed(tag_name);
        let tx = self.conn.transaction()?;

        let exists: Option<String> = tx
            .query_row(
                "SELECT id FROM items WHERE id = ?1 AND deleted_at IS NULL",
                params![item_id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(VaultError::NotFound(item_id.to_string()));
        }

        tx.execute(
            "DELETE FROM item_tags
             WHERE item_id = ?1
               AND tag_id IN (SELECT id FROM tags WHERE name = ?2 COLLATE NOCASE)",
            params![item_id, normalized],
        )?;
        prune_orphan_tags(&tx)?;
        tx.execute(
            "UPDATE items SET updated_at = ?2 WHERE id = ?1",
            params![item_id, now_ms()],
        )?;
        tx.commit()?;

        self.get_item_detail(item_id)
    }

    pub fn batch_update_entry_tags(
        &mut self,
        entries: &[EntryRef],
        add_tags: &[String],
        remove_tags: &[String],
    ) -> Result<BatchTagReport, VaultError> {
        let mut normalized_add: Vec<String> = Vec::new();
        let mut add_keys: HashSet<String> = HashSet::new();
        for raw in add_tags {
            let norm = normalize_nfc_trimmed(raw);
            if !norm.is_empty() && add_keys.insert(norm.to_lowercase()) {
                normalized_add.push(norm);
            }
        }

        let mut normalized_remove: Vec<String> = Vec::new();
        let mut remove_keys: HashSet<String> = HashSet::new();
        for raw in remove_tags {
            let norm = normalize_nfc_trimmed(raw);
            if norm.is_empty() {
                continue;
            }
            let key = norm.to_lowercase();
            if !add_keys.contains(&key) && remove_keys.insert(key) {
                normalized_remove.push(norm);
            }
        }

        if !add_tags.is_empty() && normalized_add.is_empty() && normalized_remove.is_empty() {
            return Err(VaultError::InvalidInput(
                "タグ名を入力してください".to_string(),
            ));
        }

        if entries.is_empty() || (normalized_add.is_empty() && normalized_remove.is_empty()) {
            return Ok(BatchTagReport {
                target_count: 0,
                updated_count: 0,
            });
        }

        let tx = self.conn.transaction()?;
        let mut seen_items: HashSet<String> = HashSet::new();
        let mut seen_groups: HashSet<String> = HashSet::new();
        let mut valid_item_ids: Vec<String> = Vec::new();
        let mut valid_group_ids: Vec<String> = Vec::new();

        for entry in entries {
            match entry.kind {
                EntryKind::Image => {
                    if !seen_items.insert(entry.id.clone()) {
                        continue;
                    }
                    let active: Option<String> = tx
                        .query_row(
                            "SELECT id FROM items
                             WHERE id = ?1
                               AND deleted_at IS NULL
                               AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = items.id)",
                            params![entry.id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(id) = active {
                        valid_item_ids.push(id);
                    }
                }
                EntryKind::Group => {
                    if !seen_groups.insert(entry.id.clone()) {
                        continue;
                    }
                    let active: Option<String> = tx
                        .query_row(
                            "SELECT id FROM groups WHERE id = ?1 AND deleted_at IS NULL",
                            params![entry.id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(id) = active {
                        valid_group_ids.push(id);
                    }
                }
            }
        }

        let target_count = valid_item_ids.len() + valid_group_ids.len();
        if target_count == 0 {
            return Ok(BatchTagReport {
                target_count: 0,
                updated_count: 0,
            });
        }

        let mut touched_items: HashSet<String> = HashSet::new();
        let mut touched_groups: HashSet<String> = HashSet::new();

        for tag_name in &normalized_remove {
            for item_id in &valid_item_ids {
                let affected = tx.execute(
                    "DELETE FROM item_tags
                     WHERE item_id = ?1
                       AND tag_id IN (SELECT id FROM tags WHERE name = ?2 COLLATE NOCASE)",
                    params![item_id, tag_name],
                )?;
                if affected > 0 {
                    touched_items.insert(item_id.clone());
                }
            }
            for group_id in &valid_group_ids {
                let affected = tx.execute(
                    "DELETE FROM group_tags
                     WHERE group_id = ?1
                       AND tag_id IN (SELECT id FROM tags WHERE name = ?2 COLLATE NOCASE)",
                    params![group_id, tag_name],
                )?;
                if affected > 0 {
                    touched_groups.insert(group_id.clone());
                }
            }
        }

        for tag_name in &normalized_add {
            let tag_id = upsert_tag(&tx, tag_name)?;
            for item_id in &valid_item_ids {
                let affected = tx.execute(
                    "INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
                    params![item_id, tag_id],
                )?;
                if affected > 0 {
                    touched_items.insert(item_id.clone());
                }
            }
            for group_id in &valid_group_ids {
                let affected = tx.execute(
                    "INSERT OR IGNORE INTO group_tags (group_id, tag_id) VALUES (?1, ?2)",
                    params![group_id, tag_id],
                )?;
                if affected > 0 {
                    touched_groups.insert(group_id.clone());
                }
            }
        }

        if !normalized_remove.is_empty() {
            prune_orphan_tags(&tx)?;
        }

        let now = now_ms();
        for item_id in &touched_items {
            tx.execute(
                "UPDATE items SET updated_at = ?2 WHERE id = ?1",
                params![item_id, now],
            )?;
        }
        for group_id in &touched_groups {
            tx.execute(
                "UPDATE groups SET updated_at = ?2 WHERE id = ?1",
                params![group_id, now],
            )?;
        }

        tx.commit()?;
        Ok(BatchTagReport {
            target_count,
            updated_count: touched_items.len() + touched_groups.len(),
        })
    }

    pub fn batch_set_entry_rating(
        &mut self,
        entries: &[EntryRef],
        rating: i32,
    ) -> Result<BatchRatingReport, VaultError> {
        if !(0..=5).contains(&rating) {
            return Err(VaultError::InvalidInput(
                "評価は 0〜5 の範囲で指定してください".to_string(),
            ));
        }
        if entries.is_empty() {
            return Ok(BatchRatingReport {
                target_count: 0,
                updated_count: 0,
            });
        }

        let tx = self.conn.transaction()?;
        let now = now_ms();
        let mut seen_items: HashSet<String> = HashSet::new();
        let mut seen_groups: HashSet<String> = HashSet::new();
        let mut target_count = 0usize;
        let mut updated_count = 0usize;

        for entry in entries {
            match entry.kind {
                EntryKind::Image => {
                    if !seen_items.insert(entry.id.clone()) {
                        continue;
                    }
                    let current: Option<i32> = tx
                        .query_row(
                            "SELECT rating FROM items
                             WHERE id = ?1
                               AND deleted_at IS NULL
                               AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = items.id)",
                            params![entry.id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(current_rating) = current {
                        target_count += 1;
                        if current_rating != rating {
                            tx.execute(
                                "UPDATE items SET rating = ?2, updated_at = ?3 WHERE id = ?1",
                                params![entry.id, rating, now],
                            )?;
                            updated_count += 1;
                        }
                    }
                }
                EntryKind::Group => {
                    if !seen_groups.insert(entry.id.clone()) {
                        continue;
                    }
                    let current: Option<i32> = tx
                        .query_row(
                            "SELECT rating FROM groups WHERE id = ?1 AND deleted_at IS NULL",
                            params![entry.id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(current_rating) = current {
                        target_count += 1;
                        if current_rating != rating {
                            tx.execute(
                                "UPDATE groups SET rating = ?2, updated_at = ?3 WHERE id = ?1",
                                params![entry.id, rating, now],
                            )?;
                            updated_count += 1;
                        }
                    }
                }
            }
        }

        tx.commit()?;
        Ok(BatchRatingReport {
            target_count,
            updated_count,
        })
    }

    pub fn list_tags(&self) -> Result<Vec<TagWithCount>, VaultError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.name,
                    (SELECT COUNT(*)
                     FROM item_tags it
                     JOIN items i ON i.id = it.item_id
                     WHERE it.tag_id = t.id
                       AND i.deleted_at IS NULL
                       AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id))
                    +
                    (SELECT COUNT(*)
                     FROM group_tags gt
                     JOIN groups g ON g.id = gt.group_id
                     WHERE gt.tag_id = t.id
                       AND g.deleted_at IS NULL) AS total_count
             FROM tags t
             ORDER BY total_count DESC, t.name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TagWithCount {
                id: r.get(0)?,
                name: r.get(1)?,
                count: r.get(2)?,
            })
        })?;
        let mut tags = Vec::new();
        for row in rows {
            let t = row?;
            if t.count > 0 {
                tags.push(t);
            }
        }
        Ok(tags)
    }
}
