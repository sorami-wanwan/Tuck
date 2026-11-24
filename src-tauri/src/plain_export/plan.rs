use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, OptionalExtension};

use crate::db::{ExportItemMeta, VaultDb};
use crate::error::VaultError;
use crate::models::{EntryKind, EntryRef};

use super::naming::{
    build_folder_dir_name, build_group_dir_name, build_group_page_filename, build_item_filename,
};
use super::unix_secs_to_utc_ymdhms;

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
            let key = format!("{prefix}{candidate}").to_lowercase();
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
            let key = format!("{prefix}{candidate}").to_lowercase();
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
        seen_folders: &mut HashSet<String>,
    ) -> Result<(), VaultError> {
        if !seen_folders.insert(folder_id.to_string()) {
            return Ok(());
        }
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
        let before_files = self.files.len();

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
                self.push_folder_recursive(&cur_parent, &child_id, true, ancestors, seen_folders)?;
            }
        }

        if self.files.len() > before_files {
            self.folder_count += 1;
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

    let selected_folder_set: HashSet<&str> = folder_ids.iter().map(String::as_str).collect();
    for fid in folder_ids {
        if include_subfolders {
            let mut has_selected_ancestor = false;
            let mut walk_seen = HashSet::new();
            walk_seen.insert(fid.as_str());
            let mut cur_parent = builder
                .folders_by_id
                .get(fid)
                .and_then(|f| f.parent_id.as_deref());
            while let Some(pid) = cur_parent {
                if !walk_seen.insert(pid) {
                    break;
                }
                if selected_folder_set.contains(pid) {
                    has_selected_ancestor = true;
                    break;
                }
                cur_parent = builder
                    .folders_by_id
                    .get(pid)
                    .and_then(|f| f.parent_id.as_deref());
            }
            if has_selected_ancestor {
                continue;
            }
        }
        let mut ancestors = HashSet::new();
        builder.push_folder_recursive(
            &[],
            fid,
            include_subfolders,
            &mut ancestors,
            &mut seen_folders,
        )?;
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
