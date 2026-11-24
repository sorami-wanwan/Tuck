use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;

use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::crypto::normalize_nfc_trimmed;
use crate::db::{atomic_replace_file, paths_refer_to_same_file, upsert_tag, VaultDb};
use crate::error::VaultError;
use crate::models::{EntryKind, PackageExportReport, PackageExportRequest};

use super::{topological_folders, RawFolderRow};

pub fn export_partial_package(
    src: &VaultDb,
    req: &PackageExportRequest,
) -> Result<PackageExportReport, VaultError> {
    let dest_path = Path::new(&req.dest_path);
    if paths_refer_to_same_file(dest_path, src.path()) {
        return Err(VaultError::InvalidInput(
            "現在開いている保管庫と同じファイルへは書き出せません".to_string(),
        ));
    }
    if dest_path.exists() && !req.overwrite {
        return Err(VaultError::VaultAlreadyExists(
            dest_path.display().to_string(),
        ));
    }
    if req.passphrase.is_empty() || req.passphrase.contains('\0') {
        return Err(VaultError::InvalidPassphrase(
            "パッケージに設定するパスワードを入力してください".to_string(),
        ));
    }

    let conn = src.conn();

    // 1. Expand selected folders (optionally including descendants)
    let mut all_folders = Vec::new();
    let mut children_by_parent: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, name, color, sort_order, created_at, updated_at FROM folders",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(RawFolderRow {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                name: r.get(2)?,
                color: r.get(3)?,
                sort_order: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
            })
        })?;
        for r in rows {
            let row = r?;
            if let Some(pid) = &row.parent_id {
                children_by_parent
                    .entry(pid.clone())
                    .or_default()
                    .push(row.id.clone());
            }
            all_folders.push(row);
        }
    }

    let mut selected_folder_ids: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = req.folder_ids.iter().cloned().collect();
    while let Some(fid) = queue.pop_front() {
        if !selected_folder_ids.insert(fid.clone()) {
            continue;
        }
        if req.include_subfolders {
            if let Some(children) = children_by_parent.get(&fid) {
                for cid in children {
                    queue.push_back(cid.clone());
                }
            }
        }
    }

    // 2. Collect standalone items & groups from explicit entries, folders, and tags
    let mut standalone_item_ids: HashSet<String> = HashSet::new();
    let mut group_ids: HashSet<String> = HashSet::new();

    for entry in &req.entries {
        match entry.kind {
            EntryKind::Image => {
                let valid: Option<String> = conn
                    .query_row(
                        "SELECT i.id FROM items i
                         WHERE i.id = ?1
                           AND i.deleted_at IS NULL
                           AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)",
                        params![entry.id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(id) = valid {
                    standalone_item_ids.insert(id);
                }
            }
            EntryKind::Group => {
                let valid: Option<String> = conn
                    .query_row(
                        "SELECT id FROM groups WHERE id = ?1 AND deleted_at IS NULL",
                        params![entry.id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(id) = valid {
                    group_ids.insert(id);
                }
            }
        }
    }

    for fid in &selected_folder_ids {
        let mut stmt_i = conn.prepare(
            "SELECT fi.item_id
             FROM folder_items fi
             JOIN items i ON i.id = fi.item_id
             WHERE fi.folder_id = ?1
               AND i.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)",
        )?;
        for id in stmt_i.query_map(params![fid], |r| r.get::<_, String>(0))? {
            standalone_item_ids.insert(id?);
        }

        let mut stmt_g = conn.prepare(
            "SELECT fg.group_id
             FROM folder_groups fg
             JOIN groups g ON g.id = fg.group_id
             WHERE fg.folder_id = ?1 AND g.deleted_at IS NULL",
        )?;
        for gid in stmt_g.query_map(params![fid], |r| r.get::<_, String>(0))? {
            group_ids.insert(gid?);
        }
    }

    for raw_tag in &req.tag_names {
        let normalized = normalize_nfc_trimmed(raw_tag);
        if normalized.is_empty() {
            continue;
        }
        let mut stmt_i = conn.prepare(
            "SELECT it.item_id
             FROM item_tags it
             JOIN tags t ON t.id = it.tag_id
             JOIN items i ON i.id = it.item_id
             WHERE t.name = ?1 COLLATE NOCASE
               AND i.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)",
        )?;
        for id in stmt_i.query_map(params![normalized], |r| r.get::<_, String>(0))? {
            standalone_item_ids.insert(id?);
        }

        let mut stmt_g = conn.prepare(
            "SELECT gt.group_id
             FROM group_tags gt
             JOIN tags t ON t.id = gt.tag_id
             JOIN groups g ON g.id = gt.group_id
             WHERE t.name = ?1 COLLATE NOCASE
               AND g.deleted_at IS NULL",
        )?;
        for gid in stmt_g.query_map(params![normalized], |r| r.get::<_, String>(0))? {
            group_ids.insert(gid?);
        }
    }

    if standalone_item_ids.is_empty() && group_ids.is_empty() {
        return Err(VaultError::InvalidInput(
            "書き出し対象の画像または漫画グループが見つかりません".to_string(),
        ));
    }

    // 3. Collect all page item IDs belonging to selected groups
    let mut group_page_item_ids: HashSet<String> = HashSet::new();
    for gid in &group_ids {
        let mut stmt = conn.prepare(
            "SELECT gi.item_id
             FROM group_items gi
             JOIN items i ON i.id = gi.item_id
             WHERE gi.group_id = ?1 AND i.deleted_at IS NULL
             ORDER BY gi.page_order ASC",
        )?;
        for pid in stmt.query_map(params![gid], |r| r.get::<_, String>(0))? {
            group_page_item_ids.insert(pid?);
        }
    }

    let mut all_item_ids: HashSet<String> = standalone_item_ids.clone();
    all_item_ids.extend(group_page_item_ids.iter().cloned());

    // 4. Create sibling temp package atomically
    if let Some(parent) = dest_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    let file_name = dest_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("share.tuckpack");
    let tmp_path = dest_path.with_file_name(format!(".{file_name}.tmp.{}", Uuid::new_v4()));

    let export_folders: Vec<RawFolderRow> = topological_folders(
        all_folders
            .into_iter()
            .filter(|f| selected_folder_ids.contains(&f.id))
            .collect(),
        Some(&selected_folder_ids),
    );
    let folder_count = export_folders.len();

    struct ExportItemRow {
        title: String,
        sha: String,
        mime: String,
        width: i32,
        height: i32,
        byte_size: i64,
        chunk_size: i64,
        duration_ms: i64,
        source_url: String,
        rating: i32,
        memo: String,
        palette: String,
        created_at: i64,
        updated_at: i64,
    }

    let build_res = (|| -> Result<(usize, usize), VaultError> {
        let mut dest_db = VaultDb::create(&tmp_path, &req.passphrase, true)?;
        let tx = dest_db.conn_mut().transaction()?;

        tx.execute(
            "INSERT OR REPLACE INTO vault_meta (key, value) VALUES ('vault_kind', 'package')",
            [],
        )?;

        // Copy folders (detach missing external parent_ids)
        for f in &export_folders {
            let clean_parent = f
                .parent_id
                .as_deref()
                .filter(|pid| selected_folder_ids.contains(*pid));
            tx.execute(
                "INSERT INTO folders (id, parent_id, name, color, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![f.id, clean_parent, f.name, f.color, f.sort_order, f.created_at, f.updated_at],
            )?;
        }

        // Copy items, blobs/chunks, thumbnails
        for item_id in &all_item_ids {
            let row = conn.query_row(
                "SELECT title, sha256, mime_type, width, height, byte_size, chunk_size, duration_ms, source_url, rating, memo, palette, created_at, updated_at
                 FROM items WHERE id = ?1",
                params![item_id],
                |r| {
                    Ok(ExportItemRow {
                        title: r.get(0)?,
                        sha: r.get(1)?,
                        mime: r.get(2)?,
                        width: r.get(3)?,
                        height: r.get(4)?,
                        byte_size: r.get(5)?,
                        chunk_size: r.get(6)?,
                        duration_ms: r.get(7)?,
                        source_url: r.get(8)?,
                        rating: r.get(9)?,
                        memo: r.get(10)?,
                        palette: r.get(11)?,
                        created_at: r.get(12)?,
                        updated_at: r.get(13)?,
                    })
                },
            )?;
            let chunk_sz = row.chunk_size;
            tx.execute(
                "INSERT INTO items
                    (id, title, sha256, mime_type, width, height, byte_size, chunk_size, duration_ms, source_url, rating, memo, palette, created_at, updated_at, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL)",
                params![
                    item_id,
                    row.title,
                    row.sha,
                    row.mime,
                    row.width,
                    row.height,
                    row.byte_size,
                    chunk_sz,
                    row.duration_ms,
                    row.source_url,
                    row.rating,
                    row.memo,
                    row.palette,
                    row.created_at,
                    row.updated_at
                ],
            )?;

            if chunk_sz > 0 {
                let mut stmt = conn.prepare(
                    "SELECT chunk_index, data FROM blob_chunks WHERE item_id = ?1 ORDER BY chunk_index ASC",
                )?;
                let mut rows = stmt.query(params![item_id])?;
                while let Some(row) = rows.next()? {
                    let c_idx: i64 = row.get(0)?;
                    let c_data: Vec<u8> = row.get(1)?;
                    tx.execute(
                        "INSERT INTO blob_chunks (item_id, chunk_index, data) VALUES (?1, ?2, ?3)",
                        params![item_id, c_idx, c_data],
                    )?;
                }
            } else {
                let blob: Vec<u8> = conn.query_row(
                    "SELECT data FROM blobs WHERE item_id = ?1",
                    params![item_id],
                    |r| r.get(0),
                )?;
                tx.execute(
                    "INSERT INTO blobs (item_id, data) VALUES (?1, ?2)",
                    params![item_id, blob],
                )?;
            }

            let thumb: Option<(String, i32, i32, Vec<u8>)> = conn
                .query_row(
                    "SELECT mime_type, width, height, data FROM thumbnails WHERE item_id = ?1",
                    params![item_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            if let Some((tmime, tw, th, tdata)) = thumb {
                tx.execute(
                    "INSERT INTO thumbnails (item_id, mime_type, width, height, data)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![item_id, tmime, tw, th, tdata],
                )?;
            }
        }

        // Copy groups & group_items
        for gid in &group_ids {
            let (title, rating, memo, reading_dir, spread_mode, cover_alone, created, updated): (
                String,
                i32,
                String,
                String,
                String,
                i64,
                i64,
                i64,
            ) = conn.query_row(
                "SELECT title, rating, memo, reading_dir, spread_mode, cover_alone, created_at, updated_at
                 FROM groups WHERE id = ?1",
                params![gid],
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
                    ))
                },
            )?;
            tx.execute(
                "INSERT INTO groups
                    (id, title, rating, memo, reading_dir, spread_mode, cover_alone, created_at, updated_at, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
                params![gid, title, rating, memo, reading_dir, spread_mode, cover_alone, created, updated],
            )?;

            let mut stmt_p = conn.prepare(
                "SELECT item_id, page_order FROM group_items WHERE group_id = ?1 ORDER BY page_order ASC",
            )?;
            for row in stmt_p.query_map(params![gid], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i32>(1)?))
            })? {
                let (pid, order) = row?;
                tx.execute(
                    "INSERT INTO group_items (group_id, item_id, page_order) VALUES (?1, ?2, ?3)",
                    params![gid, pid, order],
                )?;
            }
        }

        // Copy tags referenced by exported standalone items & groups
        let mut distinct_tags: HashSet<String> = HashSet::new();

        for item_id in &standalone_item_ids {
            let mut stmt_t = conn.prepare(
                "SELECT t.name FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE it.item_id = ?1",
            )?;
            for tname in stmt_t.query_map(params![item_id], |r| r.get::<_, String>(0))? {
                let name = tname?;
                let dest_tid = upsert_tag(&tx, &name)?;
                tx.execute(
                    "INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
                    params![item_id, dest_tid],
                )?;
                distinct_tags.insert(name.to_lowercase());
            }
        }

        for gid in &group_ids {
            let mut stmt_t = conn.prepare(
                "SELECT t.name FROM group_tags gt JOIN tags t ON t.id = gt.tag_id WHERE gt.group_id = ?1",
            )?;
            for tname in stmt_t.query_map(params![gid], |r| r.get::<_, String>(0))? {
                let name = tname?;
                let dest_tid = upsert_tag(&tx, &name)?;
                tx.execute(
                    "INSERT OR IGNORE INTO group_tags (group_id, tag_id) VALUES (?1, ?2)",
                    params![gid, dest_tid],
                )?;
                distinct_tags.insert(name.to_lowercase());
            }
        }

        // Copy folder memberships within selected folders
        for fid in &selected_folder_ids {
            let mut stmt_fi =
                conn.prepare("SELECT item_id, added_at FROM folder_items WHERE folder_id = ?1")?;
            for row in stmt_fi.query_map(params![fid], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })? {
                let (iid, added_at) = row?;
                if standalone_item_ids.contains(&iid) {
                    tx.execute(
                        "INSERT OR IGNORE INTO folder_items (folder_id, item_id, added_at) VALUES (?1, ?2, ?3)",
                        params![fid, iid, added_at],
                    )?;
                }
            }

            let mut stmt_fg =
                conn.prepare("SELECT group_id, added_at FROM folder_groups WHERE folder_id = ?1")?;
            for row in stmt_fg.query_map(params![fid], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })? {
                let (gid, added_at) = row?;
                if group_ids.contains(&gid) {
                    tx.execute(
                        "INSERT OR IGNORE INTO folder_groups (folder_id, group_id, added_at) VALUES (?1, ?2, ?3)",
                        params![fid, gid, added_at],
                    )?;
                }
            }
        }

        tx.commit()?;
        drop(dest_db);
        fs::OpenOptions::new()
            .write(true)
            .open(&tmp_path)?
            .sync_all()?;
        Ok((distinct_tags.len(), group_page_item_ids.len()))
    })();

    let (tag_count, page_count) = match build_res {
        Ok(counts) => counts,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    if let Err(e) = atomic_replace_file(&tmp_path, dest_path, req.overwrite) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    let file_size_bytes = fs::metadata(dest_path)?.len();

    Ok(PackageExportReport {
        item_count: standalone_item_ids.len(),
        group_count: group_ids.len(),
        page_count,
        tag_count,
        folder_count,
        file_size_bytes,
    })
}
