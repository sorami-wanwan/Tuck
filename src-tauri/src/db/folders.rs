use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::crypto::normalize_nfc_trimmed;
use crate::error::VaultError;
use crate::models::{EntryKind, EntryRef, FolderNode};

use super::{now_ms, VaultDb};

impl VaultDb {
    pub fn list_folders(&self) -> Result<Vec<FolderNode>, VaultError> {
        let mut counts: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT fi.folder_id, COUNT(*)
                 FROM folder_items fi
                 JOIN items i ON i.id = fi.item_id
                 WHERE i.deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)
                 GROUP BY fi.folder_id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (fid, cnt) = row?;
                *counts.entry(fid).or_insert(0) += cnt;
            }
        }
        {
            let mut stmt = self.conn.prepare(
                "SELECT fg.folder_id, COUNT(*)
                 FROM folder_groups fg
                 JOIN groups g ON g.id = fg.group_id
                 WHERE g.deleted_at IS NULL
                 GROUP BY fg.folder_id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (fid, cnt) = row?;
                *counts.entry(fid).or_insert(0) += cnt;
            }
        }

        let mut stmt = self.conn.prepare(
            "SELECT id, parent_id, name, color, sort_order
             FROM folders
             ORDER BY sort_order ASC, name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            Ok(FolderNode {
                id,
                parent_id: r.get(1)?,
                name: r.get(2)?,
                color: r.get(3)?,
                sort_order: r.get(4)?,
                item_count: 0,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            let mut node = row?;
            node.item_count = counts.get(&node.id).copied().unwrap_or(0);
            out.push(node);
        }
        Ok(out)
    }

    pub fn create_folder(
        &mut self,
        parent_id: Option<&str>,
        name: &str,
        color: &str,
    ) -> Result<FolderNode, VaultError> {
        let clean_name = normalize_nfc_trimmed(name);
        if clean_name.is_empty() {
            return Err(VaultError::InvalidInput(
                "フォルダ名を入力してください".to_string(),
            ));
        }

        if let Some(pid) = parent_id {
            let exists: Option<String> = self
                .conn
                .query_row("SELECT id FROM folders WHERE id = ?1", params![pid], |r| {
                    r.get(0)
                })
                .optional()?;
            if exists.is_none() {
                return Err(VaultError::NotFound(pid.to_string()));
            }
        }

        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        let next_order: i32 = self.conn.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM folders WHERE parent_id IS ?1",
            params![parent_id],
            |r| r.get(0),
        )?;

        self.conn.execute(
            "INSERT INTO folders (id, parent_id, name, color, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![id, parent_id, clean_name, color.trim(), next_order, now],
        )?;

        Ok(FolderNode {
            id,
            parent_id: parent_id.map(|s| s.to_string()),
            name: clean_name,
            color: color.trim().to_string(),
            sort_order: next_order,
            item_count: 0,
        })
    }

    pub fn rename_folder(
        &mut self,
        folder_id: &str,
        name: &str,
        color: Option<&str>,
    ) -> Result<FolderNode, VaultError> {
        let clean_name = normalize_nfc_trimmed(name);
        if clean_name.is_empty() {
            return Err(VaultError::InvalidInput(
                "フォルダ名を入力してください".to_string(),
            ));
        }
        let now = now_ms();
        let affected = if let Some(c) = color {
            self.conn.execute(
                "UPDATE folders SET name = ?2, color = ?3, updated_at = ?4 WHERE id = ?1",
                params![folder_id, clean_name, c.trim(), now],
            )?
        } else {
            self.conn.execute(
                "UPDATE folders SET name = ?2, updated_at = ?3 WHERE id = ?1",
                params![folder_id, clean_name, now],
            )?
        };
        if affected == 0 {
            return Err(VaultError::NotFound(folder_id.to_string()));
        }

        self.list_folders()?
            .into_iter()
            .find(|f| f.id == folder_id)
            .ok_or_else(|| VaultError::NotFound(folder_id.to_string()))
    }

    pub fn move_folder(
        &mut self,
        folder_id: &str,
        new_parent_id: Option<&str>,
    ) -> Result<FolderNode, VaultError> {
        let exists: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(VaultError::NotFound(folder_id.to_string()));
        }

        if let Some(target_pid) = new_parent_id {
            let mut cursor: Option<String> = Some(target_pid.to_string());
            let mut guard_steps = 0usize;
            while let Some(cur) = cursor {
                if cur == folder_id {
                    return Err(VaultError::InvalidInput(
                        "自分自身や子フォルダの配下へは移動できません".to_string(),
                    ));
                }
                guard_steps += 1;
                if guard_steps > 256 {
                    return Err(VaultError::InvalidInput(
                        "フォルダ階層が深すぎます".to_string(),
                    ));
                }
                let row: Option<Option<String>> = self
                    .conn
                    .query_row(
                        "SELECT parent_id FROM folders WHERE id = ?1",
                        params![cur],
                        |r| r.get(0),
                    )
                    .optional()?;
                cursor = match row {
                    Some(parent) => parent,
                    None => return Err(VaultError::NotFound(target_pid.to_string())),
                };
            }
        }

        self.conn.execute(
            "UPDATE folders SET parent_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![folder_id, new_parent_id, now_ms()],
        )?;

        self.list_folders()?
            .into_iter()
            .find(|f| f.id == folder_id)
            .ok_or_else(|| VaultError::NotFound(folder_id.to_string()))
    }

    pub fn delete_folder(&mut self, folder_id: &str) -> Result<(), VaultError> {
        let affected = self
            .conn
            .execute("DELETE FROM folders WHERE id = ?1", params![folder_id])?;
        if affected == 0 {
            return Err(VaultError::NotFound(folder_id.to_string()));
        }
        Ok(())
    }

    pub fn add_entries_to_folder(
        &mut self,
        folder_id: &str,
        entries: &[EntryRef],
    ) -> Result<(), VaultError> {
        let folder_exists: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?;
        if folder_exists.is_none() {
            return Err(VaultError::NotFound(folder_id.to_string()));
        }

        let now = now_ms();
        let tx = self.conn.transaction()?;
        for entry in entries {
            match entry.kind {
                EntryKind::Image => {
                    let active: Option<String> = tx
                        .query_row(
                            "SELECT id FROM items WHERE id = ?1 AND deleted_at IS NULL",
                            params![entry.id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if active.is_some() {
                        tx.execute(
                            "INSERT OR IGNORE INTO folder_items (folder_id, item_id, added_at)
                             VALUES (?1, ?2, ?3)",
                            params![folder_id, entry.id, now],
                        )?;
                    }
                }
                EntryKind::Group => {
                    let active: Option<String> = tx
                        .query_row(
                            "SELECT id FROM groups WHERE id = ?1 AND deleted_at IS NULL",
                            params![entry.id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if active.is_some() {
                        tx.execute(
                            "INSERT OR IGNORE INTO folder_groups (folder_id, group_id, added_at)
                             VALUES (?1, ?2, ?3)",
                            params![folder_id, entry.id, now],
                        )?;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_entries_from_folder(
        &mut self,
        folder_id: &str,
        entries: &[EntryRef],
    ) -> Result<(), VaultError> {
        let tx = self.conn.transaction()?;
        for entry in entries {
            match entry.kind {
                EntryKind::Image => {
                    tx.execute(
                        "DELETE FROM folder_items WHERE folder_id = ?1 AND item_id = ?2",
                        params![folder_id, entry.id],
                    )?;
                }
                EntryKind::Group => {
                    tx.execute(
                        "DELETE FROM folder_groups WHERE folder_id = ?1 AND group_id = ?2",
                        params![folder_id, entry.id],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
}
