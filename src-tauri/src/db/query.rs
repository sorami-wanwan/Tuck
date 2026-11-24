use std::collections::HashMap;

use rusqlite::{params, params_from_iter, types::Value};

use crate::crypto::normalize_nfc_trimmed;
use crate::error::VaultError;
use crate::models::{
    natural_cmp, EntryKind, EntryRef, GalleryEntry, MediaFilter, QueryScope, SearchQuery, SortBy,
};

use super::{escape_like, memo_preview, now_ms, parse_palette, prune_orphan_tags, VaultDb};

impl VaultDb {
    pub fn move_entries_to_trash(&mut self, entries: &[EntryRef]) -> Result<usize, VaultError> {
        if entries.is_empty() {
            return Ok(0);
        }
        let now = now_ms();
        let tx = self.conn.transaction()?;
        let mut count = 0usize;
        for entry in entries {
            match entry.kind {
                EntryKind::Image => {
                    count += tx.execute(
                        "UPDATE items SET deleted_at = ?2, updated_at = ?2
                         WHERE id = ?1 AND deleted_at IS NULL",
                        params![entry.id, now],
                    )?;
                }
                EntryKind::Group => {
                    let g_aff = tx.execute(
                        "UPDATE groups SET deleted_at = ?2, updated_at = ?2
                         WHERE id = ?1 AND deleted_at IS NULL",
                        params![entry.id, now],
                    )?;
                    if g_aff > 0 {
                        tx.execute(
                            "UPDATE items SET deleted_at = ?2, updated_at = ?2
                             WHERE id IN (SELECT item_id FROM group_items WHERE group_id = ?1)",
                            params![entry.id, now],
                        )?;
                        count += g_aff;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn restore_entries_from_trash(
        &mut self,
        entries: &[EntryRef],
    ) -> Result<usize, VaultError> {
        if entries.is_empty() {
            return Ok(0);
        }
        let now = now_ms();
        let tx = self.conn.transaction()?;
        let mut count = 0usize;
        for entry in entries {
            match entry.kind {
                EntryKind::Image => {
                    count += tx.execute(
                        "UPDATE items SET deleted_at = NULL, updated_at = ?2
                         WHERE id = ?1 AND deleted_at IS NOT NULL",
                        params![entry.id, now],
                    )?;
                }
                EntryKind::Group => {
                    let g_aff = tx.execute(
                        "UPDATE groups SET deleted_at = NULL, updated_at = ?2
                         WHERE id = ?1 AND deleted_at IS NOT NULL",
                        params![entry.id, now],
                    )?;
                    if g_aff > 0 {
                        tx.execute(
                            "UPDATE items SET deleted_at = NULL, updated_at = ?2
                             WHERE id IN (SELECT item_id FROM group_items WHERE group_id = ?1)",
                            params![entry.id, now],
                        )?;
                        count += g_aff;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn permanently_delete_entries(
        &mut self,
        entries: &[EntryRef],
    ) -> Result<usize, VaultError> {
        if entries.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let mut deleted = 0usize;
        for entry in entries {
            match entry.kind {
                EntryKind::Image => {
                    deleted += tx.execute("DELETE FROM items WHERE id = ?1", params![entry.id])?;
                }
                EntryKind::Group => {
                    let member_ids: Vec<String> = {
                        let mut stmt =
                            tx.prepare("SELECT item_id FROM group_items WHERE group_id = ?1")?;
                        let rows = stmt.query_map(params![entry.id], |r| r.get::<_, String>(0))?;
                        let mut out = Vec::new();
                        for row in rows {
                            out.push(row?);
                        }
                        out
                    };
                    let g_del =
                        tx.execute("DELETE FROM groups WHERE id = ?1", params![entry.id])?;
                    for mid in member_ids {
                        tx.execute("DELETE FROM items WHERE id = ?1", params![mid])?;
                    }
                    deleted += g_del;
                }
            }
        }
        prune_orphan_tags(&tx)?;
        tx.commit()?;
        Ok(deleted)
    }

    pub fn empty_trash(&mut self) -> Result<usize, VaultError> {
        let tx = self.conn.transaction()?;
        let trashed_group_items: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT gi.item_id
                 FROM group_items gi
                 JOIN groups g ON g.id = gi.group_id
                 WHERE g.deleted_at IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            out
        };

        let del_groups = tx.execute("DELETE FROM groups WHERE deleted_at IS NOT NULL", [])?;
        for mid in trashed_group_items {
            tx.execute("DELETE FROM items WHERE id = ?1", params![mid])?;
        }
        let del_items = tx.execute("DELETE FROM items WHERE deleted_at IS NOT NULL", [])?;
        prune_orphan_tags(&tx)?;
        tx.commit()?;
        Ok(del_groups + del_items)
    }

    // ---- Batch Tag / Folder Lookup Helpers ----

    pub(super) fn fetch_all_item_tags(&self) -> Result<HashMap<String, Vec<String>>, VaultError> {
        let mut stmt = self.conn.prepare(
            "SELECT it.item_id, t.name
             FROM item_tags it
             JOIN tags t ON t.id = it.tag_id
             ORDER BY t.name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (id, name) = row?;
            map.entry(id).or_default().push(name);
        }
        Ok(map)
    }

    pub(super) fn fetch_all_group_tags(&self) -> Result<HashMap<String, Vec<String>>, VaultError> {
        let mut stmt = self.conn.prepare(
            "SELECT gt.group_id, t.name
             FROM group_tags gt
             JOIN tags t ON t.id = gt.tag_id
             ORDER BY t.name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (id, name) = row?;
            map.entry(id).or_default().push(name);
        }
        Ok(map)
    }

    pub(super) fn fetch_all_item_folders(
        &self,
    ) -> Result<HashMap<String, Vec<String>>, VaultError> {
        let mut stmt = self
            .conn
            .prepare("SELECT item_id, folder_id FROM folder_items")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (id, fid) = row?;
            map.entry(id).or_default().push(fid);
        }
        Ok(map)
    }

    pub(super) fn fetch_all_group_folders(
        &self,
    ) -> Result<HashMap<String, Vec<String>>, VaultError> {
        let mut stmt = self
            .conn
            .prepare("SELECT group_id, folder_id FROM folder_groups")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (id, fid) = row?;
            map.entry(id).or_default().push(fid);
        }
        Ok(map)
    }

    pub(super) fn fetch_tags_for_item(&self, item_id: &str) -> Result<Vec<String>, VaultError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name
             FROM item_tags it
             JOIN tags t ON t.id = it.tag_id
             WHERE it.item_id = ?1
             ORDER BY t.name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map(params![item_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub(super) fn fetch_folders_for_item(&self, item_id: &str) -> Result<Vec<String>, VaultError> {
        let mut stmt = self
            .conn
            .prepare("SELECT folder_id FROM folder_items WHERE item_id = ?1")?;
        let rows = stmt.query_map(params![item_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub(super) fn fetch_tags_for_group(&self, group_id: &str) -> Result<Vec<String>, VaultError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name
             FROM group_tags gt
             JOIN tags t ON t.id = gt.tag_id
             WHERE gt.group_id = ?1
             ORDER BY t.name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map(params![group_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub(super) fn fetch_folders_for_group(
        &self,
        group_id: &str,
    ) -> Result<Vec<String>, VaultError> {
        let mut stmt = self
            .conn
            .prepare("SELECT folder_id FROM folder_groups WHERE group_id = ?1")?;
        let rows = stmt.query_map(params![group_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // ---- Unified Gallery Query ----

    pub fn query_entries(&self, query: &SearchQuery) -> Result<Vec<GalleryEntry>, VaultError> {
        let mut results = Vec::new();
        let trimmed_text = normalize_nfc_trimmed(&query.text);
        let include_standalone = matches!(
            query.media_type,
            MediaFilter::All | MediaFilter::Image | MediaFilter::Video
        );
        let include_groups = matches!(query.media_type, MediaFilter::All | MediaFilter::Manga);

        // 1. Standalone Items
        if include_standalone {
            let mut item_tags_map = self.fetch_all_item_tags()?;
            let mut item_folders_map = self.fetch_all_item_folders()?;
            let mut sql = String::from(
                "SELECT i.id, i.title, i.mime_type, i.width, i.height, i.byte_size, i.rating,
                        i.memo, i.palette, i.created_at, i.deleted_at,
                        EXISTS(SELECT 1 FROM thumbnails th WHERE th.item_id = i.id) AS has_thumb,
                        i.duration_ms, i.source_url
                 FROM items i
                 WHERE NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)",
            );
            let mut sql_params: Vec<Value> = Vec::new();

            match query.media_type {
                MediaFilter::Image => sql.push_str(" AND i.mime_type LIKE 'image/%'"),
                MediaFilter::Video => sql.push_str(" AND i.mime_type LIKE 'video/%'"),
                MediaFilter::All | MediaFilter::Manga => {}
            }

            match &query.scope {
                QueryScope::Trash => sql.push_str(" AND i.deleted_at IS NOT NULL"),
                QueryScope::All => sql.push_str(" AND i.deleted_at IS NULL"),
                QueryScope::Uncategorized => sql.push_str(
                    " AND i.deleted_at IS NULL
                      AND NOT EXISTS (SELECT 1 FROM folder_items fi WHERE fi.item_id = i.id)",
                ),
                QueryScope::Untagged => sql.push_str(
                    " AND i.deleted_at IS NULL
                      AND NOT EXISTS (SELECT 1 FROM item_tags it WHERE it.item_id = i.id)",
                ),
                QueryScope::Folder { folder_id } => {
                    sql.push_str(
                        " AND i.deleted_at IS NULL
                          AND EXISTS (SELECT 1 FROM folder_items fi WHERE fi.item_id = i.id AND fi.folder_id = ?)",
                    );
                    sql_params.push(Value::Text(folder_id.clone()));
                }
            }

            for raw_tag in &query.tag_names {
                let normalized = normalize_nfc_trimmed(raw_tag);
                if normalized.is_empty() {
                    continue;
                }
                sql.push_str(
                    " AND EXISTS (
                        SELECT 1 FROM item_tags it
                        JOIN tags t ON t.id = it.tag_id
                        WHERE it.item_id = i.id AND t.name = ? COLLATE NOCASE
                    )",
                );
                sql_params.push(Value::Text(normalized));
            }

            if let Some(min_rating) = query.min_rating {
                sql.push_str(" AND i.rating >= ?");
                sql_params.push(Value::Integer(min_rating as i64));
            }

            if !query.mime_types.is_empty() {
                let placeholders: Vec<&str> = query.mime_types.iter().map(|_| "?").collect();
                sql.push_str(&format!(
                    " AND i.mime_type IN ({})",
                    placeholders.join(", ")
                ));
                for mime in &query.mime_types {
                    sql_params.push(Value::Text(mime.clone()));
                }
            }

            if !trimmed_text.is_empty() {
                let pattern = format!("%{}%", escape_like(&trimmed_text));
                sql.push_str(
                    " AND (
                        i.title LIKE ? ESCAPE '\\' COLLATE NOCASE
                        OR i.memo LIKE ? ESCAPE '\\' COLLATE NOCASE
                        OR i.source_url LIKE ? ESCAPE '\\' COLLATE NOCASE
                        OR EXISTS (
                            SELECT 1 FROM item_tags it
                            JOIN tags t ON t.id = it.tag_id
                            WHERE it.item_id = i.id AND t.name LIKE ? ESCAPE '\\' COLLATE NOCASE
                        )
                    )",
                );
                sql_params.push(Value::Text(pattern.clone()));
                sql_params.push(Value::Text(pattern.clone()));
                sql_params.push(Value::Text(pattern.clone()));
                sql_params.push(Value::Text(pattern));
            }

            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(sql_params.iter()), |r| {
                let id: String = r.get(0)?;
                let memo: String = r.get(7)?;
                let palette_json: String = r.get(8)?;
                Ok(GalleryEntry {
                    kind: EntryKind::Image,
                    id: id.clone(),
                    cover_item_id: id,
                    cover_mime_type: r.get(2)?,
                    cover_has_thumbnail: r.get::<_, i64>(11)? != 0,
                    page_count: 1,
                    title: r.get(1)?,
                    width: r.get(3)?,
                    height: r.get(4)?,
                    byte_size: r.get(5)?,
                    duration_ms: r.get(12)?,
                    source_url: r.get(13)?,
                    rating: r.get(6)?,
                    memo_preview: memo_preview(&memo),
                    tags: Vec::new(),
                    folder_ids: Vec::new(),
                    palette: parse_palette(&palette_json),
                    created_at: r.get(9)?,
                    deleted_at: r.get(10)?,
                })
            })?;

            for row in rows {
                let mut entry = row?;
                if let Some(t) = item_tags_map.remove(&entry.id) {
                    entry.tags = t;
                }
                if let Some(f) = item_folders_map.remove(&entry.id) {
                    entry.folder_ids = f;
                }
                results.push(entry);
            }
        }

        // 2. Manga Groups
        if include_groups {
            let mut group_tags_map = self.fetch_all_group_tags()?;
            let mut group_folders_map = self.fetch_all_group_folders()?;
            let mut sql = String::from(
                "SELECT g.id, g.title, g.rating, g.memo, g.created_at, g.deleted_at,
                        ci.id, ci.mime_type, ci.width, ci.height, ci.palette,
                        EXISTS(SELECT 1 FROM thumbnails th WHERE th.item_id = ci.id) AS cover_has_thumb,
                        (SELECT COUNT(*) FROM group_items gi2 WHERE gi2.group_id = g.id) AS page_cnt,
                        (SELECT COALESCE(SUM(pi.byte_size), 0)
                         FROM group_items gi3 JOIN items pi ON pi.id = gi3.item_id
                         WHERE gi3.group_id = g.id) AS sum_bytes
                 FROM groups g
                 JOIN group_items cgi ON cgi.group_id = g.id
                    AND cgi.page_order = (SELECT MIN(gi0.page_order) FROM group_items gi0 WHERE gi0.group_id = g.id)
                 JOIN items ci ON ci.id = cgi.item_id
                 WHERE 1 = 1",
            );
            let mut sql_params: Vec<Value> = Vec::new();

            match &query.scope {
                QueryScope::Trash => sql.push_str(" AND g.deleted_at IS NOT NULL"),
                QueryScope::All => sql.push_str(" AND g.deleted_at IS NULL"),
                QueryScope::Uncategorized => sql.push_str(
                    " AND g.deleted_at IS NULL
                      AND NOT EXISTS (SELECT 1 FROM folder_groups fg WHERE fg.group_id = g.id)",
                ),
                QueryScope::Untagged => sql.push_str(
                    " AND g.deleted_at IS NULL
                      AND NOT EXISTS (SELECT 1 FROM group_tags gt WHERE gt.group_id = g.id)",
                ),
                QueryScope::Folder { folder_id } => {
                    sql.push_str(
                        " AND g.deleted_at IS NULL
                          AND EXISTS (SELECT 1 FROM folder_groups fg WHERE fg.group_id = g.id AND fg.folder_id = ?)",
                    );
                    sql_params.push(Value::Text(folder_id.clone()));
                }
            }

            for raw_tag in &query.tag_names {
                let normalized = normalize_nfc_trimmed(raw_tag);
                if normalized.is_empty() {
                    continue;
                }
                sql.push_str(
                    " AND EXISTS (
                        SELECT 1 FROM group_tags gt
                        JOIN tags t ON t.id = gt.tag_id
                        WHERE gt.group_id = g.id AND t.name = ? COLLATE NOCASE
                    )",
                );
                sql_params.push(Value::Text(normalized));
            }

            if let Some(min_rating) = query.min_rating {
                sql.push_str(" AND g.rating >= ?");
                sql_params.push(Value::Integer(min_rating as i64));
            }

            if !query.mime_types.is_empty() {
                let placeholders: Vec<&str> = query.mime_types.iter().map(|_| "?").collect();
                sql.push_str(&format!(
                    " AND EXISTS (
                        SELECT 1 FROM group_items mgi
                        JOIN items mi ON mi.id = mgi.item_id
                        WHERE mgi.group_id = g.id AND mi.mime_type IN ({})
                    )",
                    placeholders.join(", ")
                ));
                for mime in &query.mime_types {
                    sql_params.push(Value::Text(mime.clone()));
                }
            }

            if !trimmed_text.is_empty() {
                let pattern = format!("%{}%", escape_like(&trimmed_text));
                sql.push_str(
                    " AND (
                        g.title LIKE ? ESCAPE '\\' COLLATE NOCASE
                        OR g.memo LIKE ? ESCAPE '\\' COLLATE NOCASE
                        OR EXISTS (
                            SELECT 1 FROM group_tags gt
                            JOIN tags t ON t.id = gt.tag_id
                            WHERE gt.group_id = g.id AND t.name LIKE ? ESCAPE '\\' COLLATE NOCASE
                        )
                    )",
                );
                sql_params.push(Value::Text(pattern.clone()));
                sql_params.push(Value::Text(pattern.clone()));
                sql_params.push(Value::Text(pattern));
            }

            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(sql_params.iter()), |r| {
                let memo: String = r.get(3)?;
                let palette_json: String = r.get(10)?;
                Ok(GalleryEntry {
                    kind: EntryKind::Group,
                    id: r.get(0)?,
                    cover_item_id: r.get(6)?,
                    cover_mime_type: r.get(7)?,
                    cover_has_thumbnail: r.get::<_, i64>(11)? != 0,
                    page_count: r.get(12)?,
                    title: r.get(1)?,
                    width: r.get(8)?,
                    height: r.get(9)?,
                    byte_size: r.get(13)?,
                    duration_ms: 0,
                    source_url: String::new(),
                    rating: r.get(2)?,
                    memo_preview: memo_preview(&memo),
                    tags: Vec::new(),
                    folder_ids: Vec::new(),
                    palette: parse_palette(&palette_json),
                    created_at: r.get(4)?,
                    deleted_at: r.get(5)?,
                })
            })?;

            for row in rows {
                let mut entry = row?;
                if let Some(t) = group_tags_map.remove(&entry.id) {
                    entry.tags = t;
                }
                if let Some(f) = group_folders_map.remove(&entry.id) {
                    entry.folder_ids = f;
                }
                results.push(entry);
            }
        }

        // 3. Sort unified gallery list
        results.sort_by(|a, b| match query.sort_by {
            SortBy::CreatedDesc => b
                .created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id)),
            SortBy::CreatedAsc => a
                .created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id)),
            SortBy::RatingDesc => b
                .rating
                .cmp(&a.rating)
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| a.id.cmp(&b.id)),
            SortBy::SizeDesc => b
                .byte_size
                .cmp(&a.byte_size)
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| a.id.cmp(&b.id)),
            SortBy::TitleAsc => natural_cmp(&a.title, &b.title)
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| a.id.cmp(&b.id)),
        });

        Ok(results)
    }
}
