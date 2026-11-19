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
    read_file_head, validate_media_container_prefix, GeneratedThumbnail, HEADER_SNIFF_BYTES,
    MAX_IMAGE_FILE_BYTES,
};
use crate::models::{
    EntryKind, EntryRef, ItemDetail, TagWithCount, ThumbnailRegenScope, ThumbnailStats,
    ThumbnailTargetItem,
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
        if file_len == 0 {
            return Err(VaultError::CorruptedImage(
                "空のファイル（0 バイト）は取り込めません".to_string(),
            ));
        }

        // Sniff container header before reading entire multi-MB files into memory.
        let head = read_file_head(path, HEADER_SNIFF_BYTES)?;
        let mime_type = detect_mime(&head)?;
        validate_media_container_prefix(mime_type, &head, file_len)?;

        if file_len <= chunk_threshold {
            let bytes = fs::read(path)?;
            return self.import_media_bytes(&bytes, default_title, source_url, duration_ms);
        }

        if !is_video_mime(mime_type) {
            if file_len <= MAX_IMAGE_FILE_BYTES {
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

    pub fn save_generated_thumbnail(
        &mut self,
        item_id: &str,
        thumb: &GeneratedThumbnail,
        orig_width: i32,
        orig_height: i32,
        duration_ms: i64,
    ) -> Result<ItemDetail, VaultError> {
        let palette_json =
            serde_json::to_string(&thumb.palette).unwrap_or_else(|_| "[]".to_string());
        let now = now_ms();
        let safe_duration = duration_ms.max(0);

        let tx = self.conn.transaction()?;
        let affected = tx.execute(
            "UPDATE items
             SET width = CASE WHEN ?2 > 0 THEN ?2 ELSE width END,
                 height = CASE WHEN ?3 > 0 THEN ?3 ELSE height END,
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

    pub fn save_media_thumbnail(
        &mut self,
        item_id: &str,
        png_bytes: &[u8],
        orig_width: i32,
        orig_height: i32,
        duration_ms: i64,
    ) -> Result<ItemDetail, VaultError> {
        let thumb = process_client_png_thumbnail(png_bytes)?;
        self.save_generated_thumbnail(item_id, &thumb, orig_width, orig_height, duration_ms)
    }

    pub fn regenerate_standard_image_thumbnail(
        &mut self,
        item_id: &str,
    ) -> Result<ItemDetail, VaultError> {
        let bytes = self.get_blob_bytes(item_id)?;
        let processed = process_image_bytes(&bytes)?;
        let thumb = processed.thumbnail.ok_or_else(|| {
            VaultError::UnsupportedFormat(
                "AVIF または動画はクライアントまたは ffmpeg での再生成が必要です".to_string(),
            )
        })?;
        self.save_generated_thumbnail(item_id, &thumb, processed.width, processed.height, 0)
    }

    pub fn get_thumbnail_regen_item(
        &self,
        item_id: &str,
    ) -> Result<ThumbnailTargetItem, VaultError> {
        self.conn
            .query_row(
                "SELECT id, mime_type, duration_ms,
                        EXISTS(SELECT 1 FROM thumbnails th WHERE th.item_id = items.id) AS has_thumb
                 FROM items
                 WHERE id = ?1",
                params![item_id],
                |r| {
                    let has_thumb: i64 = r.get(3)?;
                    Ok(ThumbnailTargetItem {
                        item_id: r.get(0)?,
                        mime_type: r.get(1)?,
                        duration_ms: r.get(2)?,
                        has_thumbnail: has_thumb != 0,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| VaultError::NotFound(item_id.to_string()))
    }

    pub fn list_thumbnail_target_items(
        &self,
        scope: ThumbnailRegenScope,
        entries: &[EntryRef],
    ) -> Result<Vec<ThumbnailTargetItem>, VaultError> {
        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ThumbnailTargetItem> {
            let has_thumb: i64 = r.get(3)?;
            Ok(ThumbnailTargetItem {
                item_id: r.get(0)?,
                mime_type: r.get(1)?,
                duration_ms: r.get(2)?,
                has_thumbnail: has_thumb != 0,
            })
        };

        if scope == ThumbnailRegenScope::Entries {
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            for entry in entries {
                match entry.kind {
                    EntryKind::Image => {
                        let item = self
                            .conn
                            .query_row(
                                "SELECT id, mime_type, duration_ms,
                                        EXISTS(SELECT 1 FROM thumbnails th WHERE th.item_id = items.id)
                                 FROM items
                                 WHERE id = ?1 AND deleted_at IS NULL",
                                params![entry.id],
                                map_row,
                            )
                            .optional()?;
                        if let Some(it) = item {
                            if seen.insert(it.item_id.clone()) {
                                out.push(it);
                            }
                        }
                    }
                    EntryKind::Group => {
                        let mut stmt = self.conn.prepare(
                            "SELECT i.id, i.mime_type, i.duration_ms,
                                    EXISTS(SELECT 1 FROM thumbnails th WHERE th.item_id = i.id)
                             FROM group_items gi
                             JOIN groups g ON g.id = gi.group_id
                             JOIN items i ON i.id = gi.item_id
                             WHERE gi.group_id = ?1
                               AND g.deleted_at IS NULL
                               AND i.deleted_at IS NULL
                             ORDER BY gi.page_order ASC",
                        )?;
                        let rows = stmt.query_map(params![entry.id], map_row)?;
                        for row in rows {
                            let it = row?;
                            if seen.insert(it.item_id.clone()) {
                                out.push(it);
                            }
                        }
                    }
                }
            }
            return Ok(out);
        }

        let sql = match scope {
            ThumbnailRegenScope::MissingOnly => {
                "SELECT i.id, i.mime_type, i.duration_ms, 0
                 FROM items i
                 WHERE i.deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM thumbnails th WHERE th.item_id = i.id)
                 ORDER BY i.created_at DESC"
            }
            ThumbnailRegenScope::VideosOnly => {
                "SELECT i.id, i.mime_type, i.duration_ms,
                        EXISTS(SELECT 1 FROM thumbnails th WHERE th.item_id = i.id)
                 FROM items i
                 WHERE i.deleted_at IS NULL
                   AND i.mime_type LIKE 'video/%'
                 ORDER BY i.created_at DESC"
            }
            ThumbnailRegenScope::All => {
                "SELECT i.id, i.mime_type, i.duration_ms,
                        EXISTS(SELECT 1 FROM thumbnails th WHERE th.item_id = i.id)
                 FROM items i
                 WHERE i.deleted_at IS NULL
                 ORDER BY i.created_at DESC"
            }
            ThumbnailRegenScope::Entries => unreachable!(),
        };

        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], map_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn thumbnail_stats(&self, ffmpeg_available: bool) -> Result<ThumbnailStats, VaultError> {
        let (total_items, missing_count, video_count): (i64, i64, i64) = self.conn.query_row(
            "SELECT
                COUNT(*),
                COALESCE(SUM(CASE WHEN NOT EXISTS (SELECT 1 FROM thumbnails th WHERE th.item_id = i.id) THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN i.mime_type LIKE 'video/%' THEN 1 ELSE 0 END), 0)
             FROM items i
             WHERE i.deleted_at IS NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(ThumbnailStats {
            total_items,
            missing_count,
            video_count,
            ffmpeg_available,
        })
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

    pub fn read_blob_chunk(
        &self,
        item_id: &str,
        chunk_index: i64,
    ) -> Result<Option<Vec<u8>>, VaultError> {
        self.conn
            .query_row(
                "SELECT data FROM blob_chunks WHERE item_id = ?1 AND chunk_index = ?2",
                params![item_id, chunk_index],
                |r| r.get(0),
            )
            .optional()
            .map_err(VaultError::from)
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

    #[cfg(test)]
    pub fn get_display_image_bytes(&self, item_id: &str) -> Result<Vec<u8>, VaultError> {
        let (mime_type, bytes) = self.get_media_blob_with_mime(item_id)?;
        crate::imaging::render_display_webp(&mime_type, &bytes)
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
