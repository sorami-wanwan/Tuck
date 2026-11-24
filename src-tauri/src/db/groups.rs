use std::collections::HashSet;

use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::crypto::normalize_nfc_trimmed;
use crate::error::VaultError;
use crate::imaging::is_video_mime;
use crate::models::{GroupDetail, GroupPage};

use super::{now_ms, parse_palette, prune_orphan_tags, upsert_tag, VaultDb};

impl VaultDb {
    pub fn create_group_from_items(
        &mut self,
        item_ids: &[String],
        title: Option<&str>,
        target_folder_id: Option<&str>,
    ) -> Result<GroupDetail, VaultError> {
        let mut unique_ids = Vec::new();
        let mut seen = HashSet::new();
        for id in item_ids {
            if seen.insert(id.clone()) {
                unique_ids.push(id.clone());
            }
        }
        if unique_ids.is_empty() {
            return Err(VaultError::InvalidInput(
                "グループ化する画像を選択してください".to_string(),
            ));
        }

        let group_id = Uuid::new_v4().to_string();
        let now = now_ms();

        let tx = self.conn.transaction()?;
        let mut first_title = String::new();
        for (idx, item_id) in unique_ids.iter().enumerate() {
            let row: Option<(String, String, Option<i64>)> = tx
                .query_row(
                    "SELECT title, mime_type, deleted_at FROM items WHERE id = ?1",
                    params![item_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let (item_title, mime_type, deleted_at) =
                row.ok_or_else(|| VaultError::NotFound(item_id.clone()))?;
            if deleted_at.is_some() {
                return Err(VaultError::InvalidInput(
                    "ゴミ箱内の画像はグループ化できません".to_string(),
                ));
            }
            if is_video_mime(&mime_type) {
                return Err(VaultError::InvalidInput(
                    "動画は漫画グループにまとめられません".to_string(),
                ));
            }
            let already_grouped: Option<String> = tx
                .query_row(
                    "SELECT group_id FROM group_items WHERE item_id = ?1",
                    params![item_id],
                    |r| r.get(0),
                )
                .optional()?;
            if already_grouped.is_some() {
                return Err(VaultError::InvalidInput(
                    "すでに別のグループに属している画像が含まれています".to_string(),
                ));
            }
            if idx == 0 {
                first_title = item_title;
            }
        }

        let resolved_title = match title.map(normalize_nfc_trimmed) {
            Some(t) if !t.is_empty() => t,
            _ if !first_title.trim().is_empty() => first_title,
            _ => "無題のグループ".to_string(),
        };

        tx.execute(
            "INSERT INTO groups
                (id, title, rating, memo, reading_dir, spread_mode, cover_alone, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, 0, '', 'rtl', 'single', 1, ?3, ?3, NULL)",
            params![group_id, resolved_title, now],
        )?;

        for (idx, item_id) in unique_ids.iter().enumerate() {
            tx.execute(
                "INSERT INTO group_items (group_id, item_id, page_order) VALUES (?1, ?2, ?3)",
                params![group_id, item_id, idx as i32],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO group_tags (group_id, tag_id)
                 SELECT ?1, tag_id FROM item_tags WHERE item_id = ?2",
                params![group_id, item_id],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO folder_groups (folder_id, group_id, added_at)
                 SELECT folder_id, ?1, ?3 FROM folder_items WHERE item_id = ?2",
                params![group_id, item_id, now],
            )?;
            tx.execute("DELETE FROM item_tags WHERE item_id = ?1", params![item_id])?;
            tx.execute(
                "DELETE FROM folder_items WHERE item_id = ?1",
                params![item_id],
            )?;
        }

        if let Some(fid) = target_folder_id {
            let folder_exists: Option<String> = tx
                .query_row("SELECT id FROM folders WHERE id = ?1", params![fid], |r| {
                    r.get(0)
                })
                .optional()?;
            if folder_exists.is_some() {
                tx.execute(
                    "INSERT OR IGNORE INTO folder_groups (folder_id, group_id, added_at)
                     VALUES (?1, ?2, ?3)",
                    params![fid, group_id, now],
                )?;
            }
        }

        tx.commit()?;
        self.get_group_detail(&group_id)
    }

    pub fn add_items_to_group(
        &mut self,
        group_id: &str,
        item_ids: &[String],
    ) -> Result<GroupDetail, VaultError> {
        let tx = self.conn.transaction()?;
        let group_active: Option<Option<i64>> = tx
            .query_row(
                "SELECT deleted_at FROM groups WHERE id = ?1",
                params![group_id],
                |r| r.get(0),
            )
            .optional()?;
        match group_active {
            Some(None) => {}
            Some(Some(_)) => {
                return Err(VaultError::InvalidInput(
                    "ゴミ箱のグループには追加できません".to_string(),
                ))
            }
            None => return Err(VaultError::NotFound(group_id.to_string())),
        }

        let mut next_order: i32 = tx.query_row(
            "SELECT COALESCE(MAX(page_order), -1) + 1 FROM group_items WHERE group_id = ?1",
            params![group_id],
            |r| r.get(0),
        )?;

        for item_id in item_ids {
            let row: Option<(String, Option<i64>)> = tx
                .query_row(
                    "SELECT mime_type, deleted_at FROM items WHERE id = ?1",
                    params![item_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match row {
                Some((mime, None)) => {
                    if is_video_mime(&mime) {
                        return Err(VaultError::InvalidInput(
                            "動画は漫画グループに追加できません".to_string(),
                        ));
                    }
                }
                Some((_, Some(_))) => continue,
                None => return Err(VaultError::NotFound(item_id.clone())),
            }
            let in_group: Option<String> = tx
                .query_row(
                    "SELECT group_id FROM group_items WHERE item_id = ?1",
                    params![item_id],
                    |r| r.get(0),
                )
                .optional()?;
            if in_group.is_some() {
                continue;
            }
            tx.execute(
                "INSERT INTO group_items (group_id, item_id, page_order) VALUES (?1, ?2, ?3)",
                params![group_id, item_id, next_order],
            )?;
            next_order += 1;

            tx.execute(
                "INSERT OR IGNORE INTO group_tags (group_id, tag_id)
                 SELECT ?1, tag_id FROM item_tags WHERE item_id = ?2",
                params![group_id, item_id],
            )?;
            tx.execute("DELETE FROM item_tags WHERE item_id = ?1", params![item_id])?;
            tx.execute(
                "DELETE FROM folder_items WHERE item_id = ?1",
                params![item_id],
            )?;
        }

        tx.execute(
            "UPDATE groups SET updated_at = ?2 WHERE id = ?1",
            params![group_id, now_ms()],
        )?;
        tx.commit()?;
        self.get_group_detail(group_id)
    }

    pub fn reorder_group_pages(
        &mut self,
        group_id: &str,
        ordered_item_ids: &[String],
    ) -> Result<GroupDetail, VaultError> {
        let tx = self.conn.transaction()?;
        let current_ids: HashSet<String> = {
            let mut stmt = tx.prepare("SELECT item_id FROM group_items WHERE group_id = ?1")?;
            let rows = stmt.query_map(params![group_id], |r| r.get::<_, String>(0))?;
            let mut set = HashSet::new();
            for row in rows {
                set.insert(row?);
            }
            set
        };

        if current_ids.is_empty() {
            return Err(VaultError::NotFound(group_id.to_string()));
        }

        let incoming: HashSet<String> = ordered_item_ids.iter().cloned().collect();
        if incoming.len() != current_ids.len() || incoming != current_ids {
            return Err(VaultError::InvalidInput(
                "ページ並び替えの要素が一致しません".to_string(),
            ));
        }

        for (idx, item_id) in ordered_item_ids.iter().enumerate() {
            tx.execute(
                "UPDATE group_items SET page_order = ?3 WHERE group_id = ?1 AND item_id = ?2",
                params![group_id, item_id, idx as i32],
            )?;
        }
        tx.execute(
            "UPDATE groups SET updated_at = ?2 WHERE id = ?1",
            params![group_id, now_ms()],
        )?;
        tx.commit()?;
        self.get_group_detail(group_id)
    }

    pub fn remove_page_from_group(
        &mut self,
        group_id: &str,
        item_id: &str,
    ) -> Result<Option<GroupDetail>, VaultError> {
        let now = now_ms();
        let tx = self.conn.transaction()?;
        let affected = tx.execute(
            "DELETE FROM group_items WHERE group_id = ?1 AND item_id = ?2",
            params![group_id, item_id],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(item_id.to_string()));
        }

        // Preserve group's tags & folder links onto the extracted standalone item
        tx.execute(
            "INSERT OR IGNORE INTO item_tags (item_id, tag_id)
             SELECT ?1, tag_id FROM group_tags WHERE group_id = ?2",
            params![item_id, group_id],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO folder_items (folder_id, item_id, added_at)
             SELECT folder_id, ?1, ?3 FROM folder_groups WHERE group_id = ?2",
            params![item_id, group_id, now],
        )?;

        let remaining: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT item_id FROM group_items WHERE group_id = ?1 ORDER BY page_order ASC",
            )?;
            let rows = stmt.query_map(params![group_id], |r| r.get::<_, String>(0))?;
            let mut vec = Vec::new();
            for row in rows {
                vec.push(row?);
            }
            vec
        };

        if remaining.is_empty() {
            tx.execute("DELETE FROM groups WHERE id = ?1", params![group_id])?;
            prune_orphan_tags(&tx)?;
            tx.commit()?;
            return Ok(None);
        }

        for (idx, rem_id) in remaining.iter().enumerate() {
            tx.execute(
                "UPDATE group_items SET page_order = ?3 WHERE group_id = ?1 AND item_id = ?2",
                params![group_id, rem_id, idx as i32],
            )?;
        }
        tx.execute(
            "UPDATE groups SET updated_at = ?2 WHERE id = ?1",
            params![group_id, now],
        )?;
        tx.commit()?;
        Ok(Some(self.get_group_detail(group_id)?))
    }

    pub fn ungroup(&mut self, group_id: &str) -> Result<Vec<String>, VaultError> {
        let now = now_ms();
        let tx = self.conn.transaction()?;
        let exists: Option<String> = tx
            .query_row(
                "SELECT id FROM groups WHERE id = ?1",
                params![group_id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(VaultError::NotFound(group_id.to_string()));
        }

        let member_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT item_id FROM group_items WHERE group_id = ?1 ORDER BY page_order ASC",
            )?;
            let rows = stmt.query_map(params![group_id], |r| r.get::<_, String>(0))?;
            let mut vec = Vec::new();
            for row in rows {
                vec.push(row?);
            }
            vec
        };

        for item_id in &member_ids {
            tx.execute(
                "INSERT OR IGNORE INTO item_tags (item_id, tag_id)
                 SELECT ?1, tag_id FROM group_tags WHERE group_id = ?2",
                params![item_id, group_id],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO folder_items (folder_id, item_id, added_at)
                 SELECT folder_id, ?1, ?3 FROM folder_groups WHERE group_id = ?2",
                params![item_id, group_id, now],
            )?;
        }

        tx.execute("DELETE FROM groups WHERE id = ?1", params![group_id])?;
        prune_orphan_tags(&tx)?;
        tx.commit()?;
        Ok(member_ids)
    }

    pub fn get_group_detail(&self, group_id: &str) -> Result<GroupDetail, VaultError> {
        let (
            title,
            rating,
            memo,
            reading_dir,
            spread_mode,
            cover_alone_i,
            created_at,
            updated_at,
            deleted_at,
        ): (
            String,
            i32,
            String,
            String,
            String,
            i64,
            i64,
            i64,
            Option<i64>,
        ) = self
            .conn
            .query_row(
                "SELECT title, rating, memo, reading_dir, spread_mode, cover_alone,
                        created_at, updated_at, deleted_at
                 FROM groups WHERE id = ?1",
                params![group_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| VaultError::NotFound(group_id.to_string()))?;

        let pages: Vec<GroupPage> = {
            let mut stmt = self.conn.prepare(
                "SELECT i.id, gi.page_order, i.title, i.mime_type, i.width, i.height, i.byte_size,
                        EXISTS(SELECT 1 FROM thumbnails th WHERE th.item_id = i.id),
                        i.created_at
                 FROM group_items gi
                 JOIN items i ON i.id = gi.item_id
                 WHERE gi.group_id = ?1
                 ORDER BY gi.page_order ASC",
            )?;
            let rows = stmt.query_map(params![group_id], |r| {
                Ok(GroupPage {
                    item_id: r.get(0)?,
                    page_order: r.get(1)?,
                    title: r.get(2)?,
                    mime_type: r.get(3)?,
                    width: r.get(4)?,
                    height: r.get(5)?,
                    byte_size: r.get(6)?,
                    has_thumbnail: r.get::<_, i64>(7)? != 0,
                    created_at: r.get(8)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            out
        };

        let total_bytes: i64 = pages.iter().map(|p| p.byte_size).sum();
        let page_count = pages.len() as i32;

        let palette = if let Some(first) = pages.first() {
            self.conn
                .query_row(
                    "SELECT palette FROM items WHERE id = ?1",
                    params![first.item_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .map(|s| parse_palette(&s))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let tags = self.fetch_tags_for_group(group_id)?;
        let folder_ids = self.fetch_folders_for_group(group_id)?;

        Ok(GroupDetail {
            id: group_id.to_string(),
            title,
            rating,
            memo,
            reading_dir,
            spread_mode,
            cover_alone: cover_alone_i != 0,
            tags,
            folder_ids,
            palette,
            total_bytes,
            page_count,
            pages,
            created_at,
            updated_at,
            deleted_at,
        })
    }

    pub fn set_group_title(
        &mut self,
        group_id: &str,
        title: &str,
    ) -> Result<GroupDetail, VaultError> {
        let normalized = normalize_nfc_trimmed(title);
        let affected = self.conn.execute(
            "UPDATE groups SET title = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
            params![group_id, normalized, now_ms()],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(group_id.to_string()));
        }
        self.get_group_detail(group_id)
    }

    pub fn set_group_rating(
        &mut self,
        group_id: &str,
        rating: i32,
    ) -> Result<GroupDetail, VaultError> {
        if !(0..=5).contains(&rating) {
            return Err(VaultError::InvalidInput(
                "評価は 0〜5 の範囲で指定してください".to_string(),
            ));
        }
        let affected = self.conn.execute(
            "UPDATE groups SET rating = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
            params![group_id, rating, now_ms()],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(group_id.to_string()));
        }
        self.get_group_detail(group_id)
    }

    pub fn set_group_memo(
        &mut self,
        group_id: &str,
        memo: &str,
    ) -> Result<GroupDetail, VaultError> {
        let normalized = unicode_normalization::UnicodeNormalization::nfc(memo).collect::<String>();
        let affected = self.conn.execute(
            "UPDATE groups SET memo = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
            params![group_id, normalized, now_ms()],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(group_id.to_string()));
        }
        self.get_group_detail(group_id)
    }

    pub fn set_group_reader_prefs(
        &mut self,
        group_id: &str,
        reading_dir: &str,
        spread_mode: &str,
        cover_alone: bool,
    ) -> Result<GroupDetail, VaultError> {
        if reading_dir != "rtl" && reading_dir != "ltr" {
            return Err(VaultError::InvalidInput("綴じ方向が不正です".to_string()));
        }
        if spread_mode != "single" && spread_mode != "spread" {
            return Err(VaultError::InvalidInput("表示モードが不正です".to_string()));
        }
        let affected = self.conn.execute(
            "UPDATE groups
             SET reading_dir = ?2, spread_mode = ?3, cover_alone = ?4, updated_at = ?5
             WHERE id = ?1",
            params![
                group_id,
                reading_dir,
                spread_mode,
                if cover_alone { 1 } else { 0 },
                now_ms()
            ],
        )?;
        if affected == 0 {
            return Err(VaultError::NotFound(group_id.to_string()));
        }
        self.get_group_detail(group_id)
    }

    pub fn add_group_tag(
        &mut self,
        group_id: &str,
        tag_name: &str,
    ) -> Result<GroupDetail, VaultError> {
        let normalized = normalize_nfc_trimmed(tag_name);
        if normalized.is_empty() {
            return Err(VaultError::InvalidInput(
                "タグ名を入力してください".to_string(),
            ));
        }

        let tx = self.conn.transaction()?;
        let exists: Option<String> = tx
            .query_row(
                "SELECT id FROM groups WHERE id = ?1 AND deleted_at IS NULL",
                params![group_id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(VaultError::NotFound(group_id.to_string()));
        }

        let tag_id = upsert_tag(&tx, &normalized)?;
        tx.execute(
            "INSERT OR IGNORE INTO group_tags (group_id, tag_id) VALUES (?1, ?2)",
            params![group_id, tag_id],
        )?;
        tx.execute(
            "UPDATE groups SET updated_at = ?2 WHERE id = ?1",
            params![group_id, now_ms()],
        )?;
        tx.commit()?;
        self.get_group_detail(group_id)
    }

    pub fn remove_group_tag(
        &mut self,
        group_id: &str,
        tag_name: &str,
    ) -> Result<GroupDetail, VaultError> {
        let normalized = normalize_nfc_trimmed(tag_name);
        let tx = self.conn.transaction()?;
        let exists: Option<String> = tx
            .query_row(
                "SELECT id FROM groups WHERE id = ?1 AND deleted_at IS NULL",
                params![group_id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(VaultError::NotFound(group_id.to_string()));
        }

        tx.execute(
            "DELETE FROM group_tags
             WHERE group_id = ?1
               AND tag_id IN (SELECT id FROM tags WHERE name = ?2 COLLATE NOCASE)",
            params![group_id, normalized],
        )?;
        prune_orphan_tags(&tx)?;
        tx.execute(
            "UPDATE groups SET updated_at = ?2 WHERE id = ?1",
            params![group_id, now_ms()],
        )?;
        tx.commit()?;
        self.get_group_detail(group_id)
    }
}
