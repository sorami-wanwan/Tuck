use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;

use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::crypto::normalize_nfc_trimmed;
use crate::db::{now_ms, upsert_tag, VaultDb};
use crate::error::VaultError;
use crate::imaging::is_video_mime;
use crate::models::{
    EntryKind, PackageExportReport, PackageExportRequest, PackageImportReport, PackageImportRequest,
};

struct RawFolderRow {
    id: String,
    parent_id: Option<String>,
    name: String,
    color: String,
    sort_order: i32,
    created_at: i64,
    updated_at: i64,
}

struct RawItemRow {
    id: String,
    title: String,
    sha256: String,
    mime_type: String,
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

struct RawGroupRow {
    id: String,
    title: String,
    rating: i32,
    memo: String,
    reading_dir: String,
    spread_mode: String,
    cover_alone: i64,
    created_at: i64,
    updated_at: i64,
    page_item_ids: Vec<String>,
}

fn topological_folders(
    rows: Vec<RawFolderRow>,
    allowed: Option<&HashSet<String>>,
) -> Vec<RawFolderRow> {
    let mut pending: VecDeque<RawFolderRow> = rows.into_iter().collect();
    let mut placed = HashSet::new();
    let mut sorted = Vec::with_capacity(pending.len());
    let mut guard = 0usize;
    let max_iters = pending.len().saturating_mul(pending.len() + 2).max(16);

    while let Some(row) = pending.pop_front() {
        guard += 1;
        let parent_ready = match &row.parent_id {
            None => true,
            Some(pid) => {
                if let Some(allow_set) = allowed {
                    !allow_set.contains(pid) || placed.contains(pid)
                } else {
                    placed.contains(pid)
                }
            }
        };

        if parent_ready || guard > max_iters {
            placed.insert(row.id.clone());
            sorted.push(row);
        } else {
            pending.push_back(row);
        }
    }
    sorted
}

pub fn export_partial_package(
    src: &VaultDb,
    req: &PackageExportRequest,
) -> Result<PackageExportReport, VaultError> {
    let dest_path = Path::new(&req.dest_path);
    if dest_path == src.path() {
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
            "SELECT item_id FROM group_items WHERE group_id = ?1 ORDER BY page_order ASC",
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
        fs::OpenOptions::new().write(true).open(&tmp_path)?.sync_all()?;
        Ok((distinct_tags.len(), group_page_item_ids.len()))
    })();

    let (tag_count, page_count) = match build_res {
        Ok(counts) => counts,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    if req.overwrite && dest_path.exists() {
        if let Err(e) = fs::remove_file(dest_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(e.into());
        }
    }
    if let Err(e) = fs::rename(&tmp_path, dest_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e.into());
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

fn import_or_find_item(
    src_conn: &rusqlite::Connection,
    tx: &Transaction<'_>,
    src_item_id: &str,
    item_map: &mut HashMap<String, String>,
    avif_pending_ids: &mut Vec<String>,
    video_pending_ids: &mut Vec<String>,
) -> Result<(String, bool), VaultError> {
    if let Some(mapped) = item_map.get(src_item_id) {
        return Ok((mapped.clone(), true));
    }

    let src_row: RawItemRow = src_conn.query_row(
        "SELECT id, title, sha256, mime_type, width, height, byte_size, chunk_size, duration_ms, source_url, rating, memo, palette, created_at, updated_at
         FROM items WHERE id = ?1",
        params![src_item_id],
        |r| {
            Ok(RawItemRow {
                id: r.get(0)?,
                title: r.get(1)?,
                sha256: r.get(2)?,
                mime_type: r.get(3)?,
                width: r.get(4)?,
                height: r.get(5)?,
                byte_size: r.get(6)?,
                chunk_size: r.get(7)?,
                duration_ms: r.get(8)?,
                source_url: r.get(9)?,
                rating: r.get(10)?,
                memo: r.get(11)?,
                palette: r.get(12)?,
                created_at: r.get(13)?,
                updated_at: r.get(14)?,
            })
        },
    )?;

    let existing: Option<(String, String, String, i32, i64, String)> = tx
        .query_row(
            "SELECT id, title, memo, rating, duration_ms, source_url FROM items WHERE sha256 = ?1",
            params![src_row.sha256],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;

    if let Some((existing_id, cur_title, cur_memo, cur_rating, cur_dur, cur_url)) = existing {
        let now = now_ms();
        let next_title = if cur_title.trim().is_empty() && !src_row.title.trim().is_empty() {
            src_row.title
        } else {
            cur_title
        };
        let next_memo = if cur_memo.trim().is_empty() && !src_row.memo.trim().is_empty() {
            src_row.memo
        } else {
            cur_memo
        };
        let next_rating = if cur_rating == 0 && src_row.rating > 0 {
            src_row.rating
        } else {
            cur_rating
        };
        let next_dur = if cur_dur == 0 && src_row.duration_ms > 0 {
            src_row.duration_ms
        } else {
            cur_dur
        };
        let next_url = if cur_url.trim().is_empty() && !src_row.source_url.trim().is_empty() {
            src_row.source_url
        } else {
            cur_url
        };
        tx.execute(
            "UPDATE items
             SET title = ?2, memo = ?3, rating = ?4, duration_ms = ?5, source_url = ?6, deleted_at = NULL, updated_at = ?7
             WHERE id = ?1",
            params![existing_id, next_title, next_memo, next_rating, next_dur, next_url, now],
        )?;
        item_map.insert(src_item_id.to_string(), existing_id.clone());
        return Ok((existing_id, true));
    }

    let id_taken: Option<String> = tx
        .query_row(
            "SELECT id FROM items WHERE id = ?1",
            params![src_row.id],
            |r| r.get(0),
        )
        .optional()?;
    let dest_id = if id_taken.is_some() {
        Uuid::new_v4().to_string()
    } else {
        src_row.id
    };

    tx.execute(
        "INSERT INTO items
            (id, title, sha256, mime_type, width, height, byte_size, chunk_size, duration_ms, source_url, rating, memo, palette, created_at, updated_at, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL)",
        params![
            dest_id,
            src_row.title,
            src_row.sha256,
            src_row.mime_type,
            src_row.width,
            src_row.height,
            src_row.byte_size,
            src_row.chunk_size,
            src_row.duration_ms,
            src_row.source_url,
            src_row.rating,
            src_row.memo,
            src_row.palette,
            src_row.created_at,
            src_row.updated_at,
        ],
    )?;

    if src_row.chunk_size > 0 {
        let mut stmt = src_conn.prepare(
            "SELECT chunk_index, data FROM blob_chunks WHERE item_id = ?1 ORDER BY chunk_index ASC",
        )?;
        let mut rows = stmt.query(params![src_item_id])?;
        while let Some(row) = rows.next()? {
            let c_idx: i64 = row.get(0)?;
            let c_data: Vec<u8> = row.get(1)?;
            tx.execute(
                "INSERT INTO blob_chunks (item_id, chunk_index, data) VALUES (?1, ?2, ?3)",
                params![dest_id, c_idx, c_data],
            )?;
        }
    } else {
        let blob_data: Vec<u8> = src_conn.query_row(
            "SELECT data FROM blobs WHERE item_id = ?1",
            params![src_item_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO blobs (item_id, data) VALUES (?1, ?2)",
            params![dest_id, blob_data],
        )?;
    }

    let thumb: Option<(String, i32, i32, Vec<u8>)> = src_conn
        .query_row(
            "SELECT mime_type, width, height, data FROM thumbnails WHERE item_id = ?1",
            params![src_item_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    if let Some((tmime, tw, th, tdata)) = thumb {
        tx.execute(
            "INSERT INTO thumbnails (item_id, mime_type, width, height, data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![dest_id, tmime, tw, th, tdata],
        )?;
    } else if src_row.mime_type == "image/avif" {
        avif_pending_ids.push(dest_id.clone());
    } else if is_video_mime(&src_row.mime_type) {
        video_pending_ids.push(dest_id.clone());
    }

    item_map.insert(src_item_id.to_string(), dest_id.clone());
    Ok((dest_id, false))
}

pub fn import_partial_package(
    dest: &mut VaultDb,
    req: &PackageImportRequest,
) -> Result<PackageImportReport, VaultError> {
    let source_path = Path::new(&req.source_path);
    if source_path == dest.path() {
        return Err(VaultError::InvalidInput(
            "現在開いている保管庫自体は取り込めません".to_string(),
        ));
    }

    let src_db = VaultDb::open(source_path, &req.passphrase)?;
    let src_conn = src_db.conn();

    if let Some(target_fid) = req.target_folder_id.as_deref() {
        let exists: Option<String> = dest
            .conn()
            .query_row(
                "SELECT id FROM folders WHERE id = ?1",
                params![target_fid],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(VaultError::NotFound(target_fid.to_string()));
        }
    }

    // 1. Snapshot folder hierarchy, standalone items, and groups from source
    let src_folders = {
        let mut stmt = src_conn.prepare(
            "SELECT id, parent_id, name, color, sort_order, created_at, updated_at
             FROM folders ORDER BY sort_order ASC, name COLLATE NOCASE ASC",
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
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        topological_folders(out, None)
    };

    let src_standalone_ids: Vec<String> = {
        let mut stmt = src_conn.prepare(
            "SELECT i.id FROM items i
             WHERE i.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)
             ORDER BY i.created_at ASC",
        )?;
        let mut out = Vec::new();
        for r in stmt.query_map([], |r| r.get::<_, String>(0))? {
            out.push(r?);
        }
        out
    };

    let src_groups: Vec<RawGroupRow> = {
        let mut stmt = src_conn.prepare(
            "SELECT id, title, rating, memo, reading_dir, spread_mode, cover_alone, created_at, updated_at
             FROM groups WHERE deleted_at IS NULL ORDER BY created_at ASC",
        )?;
        let mut out = Vec::new();
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i32>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })? {
            let (
                id,
                title,
                rating,
                memo,
                reading_dir,
                spread_mode,
                cover_alone,
                created_at,
                updated_at,
            ) = row?;
            let mut stmt_p = src_conn.prepare(
                "SELECT gi.item_id
                 FROM group_items gi
                 JOIN items i ON i.id = gi.item_id
                 WHERE gi.group_id = ?1 AND i.deleted_at IS NULL
                 ORDER BY gi.page_order ASC",
            )?;
            let mut page_item_ids = Vec::new();
            for p in stmt_p.query_map(params![id], |r| r.get::<_, String>(0))? {
                page_item_ids.push(p?);
            }
            if !page_item_ids.is_empty() {
                out.push(RawGroupRow {
                    id,
                    title,
                    rating,
                    memo,
                    reading_dir,
                    spread_mode,
                    cover_alone,
                    created_at,
                    updated_at,
                    page_item_ids,
                });
            }
        }
        out
    };

    let mut report = PackageImportReport {
        imported_items: 0,
        merged_duplicate_items: 0,
        imported_groups: 0,
        skipped_duplicate_groups: 0,
        imported_folders: 0,
        merged_tag_links: 0,
        avif_pending_ids: Vec::new(),
        video_pending_ids: Vec::new(),
    };

    let now = now_ms();
    let tx = dest.conn_mut().transaction()?;

    // 2. Merge folder hierarchy
    let mut folder_map: HashMap<String, String> = HashMap::new();
    for sf in &src_folders {
        let dest_parent: Option<&str> = sf
            .parent_id
            .as_deref()
            .and_then(|pid| folder_map.get(pid).map(|s| s.as_str()))
            .or(req.target_folder_id.as_deref());

        let existing_fid: Option<String> = tx
            .query_row(
                "SELECT id FROM folders WHERE parent_id IS ?1 AND name = ?2 COLLATE NOCASE",
                params![dest_parent, sf.name],
                |r| r.get(0),
            )
            .optional()?;

        if let Some(eid) = existing_fid {
            folder_map.insert(sf.id.clone(), eid);
        } else {
            let id_taken: Option<String> = tx
                .query_row(
                    "SELECT id FROM folders WHERE id = ?1",
                    params![sf.id],
                    |r| r.get(0),
                )
                .optional()?;
            let new_fid = if id_taken.is_some() {
                Uuid::new_v4().to_string()
            } else {
                sf.id.clone()
            };
            let next_order: i32 = tx.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM folders WHERE parent_id IS ?1",
                params![dest_parent],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO folders (id, parent_id, name, color, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![new_fid, dest_parent, sf.name, sf.color, next_order, sf.created_at, now],
            )?;
            folder_map.insert(sf.id.clone(), new_fid);
            report.imported_folders += 1;
        }
    }

    // 3. Merge standalone items + tags + folder links
    let mut item_map: HashMap<String, String> = HashMap::new();
    for src_iid in &src_standalone_ids {
        let (dest_iid, is_dup) = import_or_find_item(
            src_conn,
            &tx,
            src_iid,
            &mut item_map,
            &mut report.avif_pending_ids,
            &mut report.video_pending_ids,
        )?;
        if is_dup {
            report.merged_duplicate_items += 1;
        } else {
            report.imported_items += 1;
        }

        let existing_group_for_item: Option<String> = tx
            .query_row(
                "SELECT group_id FROM group_items WHERE item_id = ?1",
                params![dest_iid],
                |r| r.get(0),
            )
            .optional()?;

        // Merge item tags
        {
            let mut stmt_t = src_conn.prepare(
                "SELECT t.name FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE it.item_id = ?1",
            )?;
            for tname in stmt_t.query_map(params![src_iid], |r| r.get::<_, String>(0))? {
                let tag_name = normalize_nfc_trimmed(&tname?);
                if tag_name.is_empty() {
                    continue;
                }
                let tid = upsert_tag(&tx, &tag_name)?;
                let inserted = if let Some(ref gid) = existing_group_for_item {
                    tx.execute(
                        "INSERT OR IGNORE INTO group_tags (group_id, tag_id) VALUES (?1, ?2)",
                        params![gid, tid],
                    )?
                } else {
                    tx.execute(
                        "INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
                        params![dest_iid, tid],
                    )?
                };
                report.merged_tag_links += inserted;
            }
        }

        // Merge folder links (only when standalone in destination)
        if existing_group_for_item.is_none() {
            let mut had_folder = false;
            let mut stmt_f =
                src_conn.prepare("SELECT folder_id FROM folder_items WHERE item_id = ?1")?;
            for row in stmt_f.query_map(params![src_iid], |r| r.get::<_, String>(0))? {
                let src_fid = row?;
                if let Some(dest_fid) = folder_map.get(&src_fid) {
                    tx.execute(
                        "INSERT OR IGNORE INTO folder_items (folder_id, item_id, added_at)
                         VALUES (?1, ?2, ?3)",
                        params![dest_fid, dest_iid, now],
                    )?;
                    had_folder = true;
                }
            }
            if !had_folder {
                if let Some(target_fid) = req.target_folder_id.as_deref() {
                    tx.execute(
                        "INSERT OR IGNORE INTO folder_items (folder_id, item_id, added_at)
                         VALUES (?1, ?2, ?3)",
                        params![target_fid, dest_iid, now],
                    )?;
                }
            }
        }
    }

    // 4. Merge manga groups + tags + folder links
    for sg in &src_groups {
        let mut mapped_pages = Vec::with_capacity(sg.page_item_ids.len());
        for pid in &sg.page_item_ids {
            let (dest_pid, is_dup) = import_or_find_item(
                src_conn,
                &tx,
                pid,
                &mut item_map,
                &mut report.avif_pending_ids,
                &mut report.video_pending_ids,
            )?;
            if is_dup {
                report.merged_duplicate_items += 1;
            } else {
                report.imported_items += 1;
            }
            mapped_pages.push(dest_pid);
        }

        let mut existing_groups_for_pages: HashSet<String> = HashSet::new();
        let mut free_pages: Vec<String> = Vec::new();
        for dest_pid in &mapped_pages {
            let gid_opt: Option<String> = tx
                .query_row(
                    "SELECT group_id FROM group_items WHERE item_id = ?1",
                    params![dest_pid],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(gid) = gid_opt {
                existing_groups_for_pages.insert(gid);
            } else if !free_pages.contains(dest_pid) {
                free_pages.push(dest_pid.clone());
            }
        }

        let dest_group_id: Option<String> = if free_pages.is_empty()
            && existing_groups_for_pages.len() == 1
        {
            // Exact duplicate manga group -> merge tags & folders onto existing group
            report.skipped_duplicate_groups += 1;
            existing_groups_for_pages.into_iter().next()
        } else if !free_pages.is_empty() {
            let id_taken: Option<String> = tx
                .query_row("SELECT id FROM groups WHERE id = ?1", params![sg.id], |r| {
                    r.get(0)
                })
                .optional()?;
            let new_gid = if id_taken.is_some() {
                Uuid::new_v4().to_string()
            } else {
                sg.id.clone()
            };

            tx.execute(
                "INSERT INTO groups
                    (id, title, rating, memo, reading_dir, spread_mode, cover_alone, created_at, updated_at, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
                params![
                    new_gid,
                    sg.title,
                    sg.rating,
                    sg.memo,
                    sg.reading_dir,
                    sg.spread_mode,
                    sg.cover_alone,
                    sg.created_at,
                    sg.updated_at,
                ],
            )?;

            for (order, dest_pid) in free_pages.iter().enumerate() {
                tx.execute(
                    "INSERT INTO group_items (group_id, item_id, page_order) VALUES (?1, ?2, ?3)",
                    params![new_gid, dest_pid, order as i32],
                )?;
                tx.execute(
                    "DELETE FROM item_tags WHERE item_id = ?1",
                    params![dest_pid],
                )?;
                tx.execute(
                    "DELETE FROM folder_items WHERE item_id = ?1",
                    params![dest_pid],
                )?;
            }
            report.imported_groups += 1;
            Some(new_gid)
        } else {
            report.skipped_duplicate_groups += 1;
            None
        };

        if let Some(dest_gid) = dest_group_id {
            let mut stmt_t = src_conn.prepare(
                "SELECT t.name FROM group_tags gt JOIN tags t ON t.id = gt.tag_id WHERE gt.group_id = ?1",
            )?;
            for tname in stmt_t.query_map(params![sg.id], |r| r.get::<_, String>(0))? {
                let tag_name = normalize_nfc_trimmed(&tname?);
                if tag_name.is_empty() {
                    continue;
                }
                let tid = upsert_tag(&tx, &tag_name)?;
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO group_tags (group_id, tag_id) VALUES (?1, ?2)",
                    params![dest_gid, tid],
                )?;
                report.merged_tag_links += inserted;
            }

            let mut had_folder = false;
            let mut stmt_f =
                src_conn.prepare("SELECT folder_id FROM folder_groups WHERE group_id = ?1")?;
            for row in stmt_f.query_map(params![sg.id], |r| r.get::<_, String>(0))? {
                let src_fid = row?;
                if let Some(dest_fid) = folder_map.get(&src_fid) {
                    tx.execute(
                        "INSERT OR IGNORE INTO folder_groups (folder_id, group_id, added_at)
                         VALUES (?1, ?2, ?3)",
                        params![dest_fid, dest_gid, now],
                    )?;
                    had_folder = true;
                }
            }
            if !had_folder {
                if let Some(target_fid) = req.target_folder_id.as_deref() {
                    tx.execute(
                        "INSERT OR IGNORE INTO folder_groups (folder_id, group_id, added_at)
                         VALUES (?1, ?2, ?3)",
                        params![target_fid, dest_gid, now],
                    )?;
                }
            }
        }
    }

    tx.commit()?;
    drop(src_db);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ImportOutcome;
    use crate::models::{EntryRef, QueryScope, SearchQuery};
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;
    use tempfile::TempDir;

    fn make_png(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut img = RgbaImage::new(width, height);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = Rgba([
                r.wrapping_add((x % 30) as u8),
                g.wrapping_add((y % 30) as u8),
                b,
                255,
            ]);
        }
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    fn import_id(db: &mut VaultDb, bytes: &[u8], title: Option<&str>) -> String {
        match db.import_bytes(bytes, title).unwrap() {
            ImportOutcome::Imported { id, .. } => id,
            ImportOutcome::Duplicate => panic!("unexpected duplicate"),
        }
    }

    fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn test_partial_package_export_and_import_merge() {
        let dir = TempDir::new().unwrap();
        let vault_a_path = dir.path().join("vault_a.vault");
        let pkg_path = dir.path().join("exchange.tuckpack");
        let vault_b_path = dir.path().join("vault_b.vault");

        let img1_bytes = make_png(80, 80, 20, 100, 200);
        let p1_bytes = make_png(80, 110, 200, 50, 50);
        let p2_bytes = make_png(80, 110, 50, 200, 50);
        let other_bytes = make_png(60, 60, 120, 120, 120);

        // 1. Populate Source Vault A
        {
            let mut db_a = VaultDb::create(&vault_a_path, "vault-A-pass", false).unwrap();
            let f_root = db_a.create_folder(None, "作家X", "#4c8dff").unwrap();
            let f_sub = db_a.create_folder(Some(&f_root.id), "単行本", "").unwrap();

            let img1 = import_id(&mut db_a, &img1_bytes, Some("イラスト1"));
            db_a.add_tag(&img1, "風景").unwrap();
            db_a.add_tag(&img1, "夜").unwrap();
            db_a.set_rating(&img1, 5).unwrap();
            db_a.add_entries_to_folder(
                &f_root.id,
                &[EntryRef {
                    kind: EntryKind::Image,
                    id: img1,
                }],
            )
            .unwrap();

            let gp1 = import_id(&mut db_a, &p1_bytes, Some("m1"));
            let gp2 = import_id(&mut db_a, &p2_bytes, Some("m2"));
            let manga = db_a
                .create_group_from_items(&[gp1, gp2], Some("単行本1巻"), Some(&f_sub.id))
                .unwrap();
            db_a.add_group_tag(&manga.id, "アクション").unwrap();
            db_a.set_group_reader_prefs(&manga.id, "ltr", "spread", false)
                .unwrap();

            let other = import_id(&mut db_a, &other_bytes, Some("除外イラスト"));
            db_a.add_tag(&other, "除外").unwrap();

            let report = export_partial_package(
                &db_a,
                &PackageExportRequest {
                    dest_path: pkg_path.display().to_string(),
                    passphrase: "share-pass-99".to_string(),
                    overwrite: false,
                    folder_ids: vec![f_root.id],
                    include_subfolders: true,
                    tag_names: vec![],
                    entries: vec![],
                },
            )
            .unwrap();

            assert_eq!(report.item_count, 1);
            assert_eq!(report.group_count, 1);
            assert_eq!(report.page_count, 2);
            assert_eq!(report.folder_count, 2);
            assert_eq!(report.tag_count, 3);
        }

        // Verify .tuckpack encryption at rest & password isolation
        let raw_pkg = fs::read(&pkg_path).unwrap();
        assert!(!contains_subsequence(&raw_pkg, "作家X".as_bytes()));
        assert!(!contains_subsequence(&raw_pkg, "風景".as_bytes()));
        assert!(!contains_subsequence(&raw_pkg, "アクション".as_bytes()));
        assert!(!contains_subsequence(
            &raw_pkg,
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        ));
        assert!(matches!(
            VaultDb::open(&pkg_path, "vault-A-pass"),
            Err(VaultError::WrongPassword)
        ));
        assert!(VaultDb::open(&pkg_path, "share-pass-99").is_ok());

        // 2. Import into Destination Vault B (pre-populated with duplicate img1 + existing tag)
        let mut db_b = VaultDb::create(&vault_b_path, "vault-B-pass", false).unwrap();
        let existing_img1 = import_id(&mut db_b, &img1_bytes, Some("既存タイトル"));
        db_b.add_tag(&existing_img1, "既存タグ").unwrap();

        let imp1 = import_partial_package(
            &mut db_b,
            &PackageImportRequest {
                source_path: pkg_path.display().to_string(),
                passphrase: "share-pass-99".to_string(),
                target_folder_id: None,
            },
        )
        .unwrap();

        assert_eq!(imp1.merged_duplicate_items, 1);
        assert_eq!(imp1.imported_items, 2); // 2 manga pages
        assert_eq!(imp1.imported_groups, 1);
        assert_eq!(imp1.imported_folders, 2);

        // Verify merged tags & existing title preserved
        let img1_detail = db_b.get_item_detail(&existing_img1).unwrap();
        assert_eq!(img1_detail.title, "既存タイトル");
        assert!(img1_detail.tags.contains(&"既存タグ".to_string()));
        assert!(img1_detail.tags.contains(&"風景".to_string()));
        assert!(img1_detail.tags.contains(&"夜".to_string()));

        // Verify manga group & nested folders merged intact
        let entries_b = db_b.query_entries(&SearchQuery::default()).unwrap();
        assert_eq!(entries_b.len(), 2); // img1 + manga1 (no "除外イラスト"!)
        let manga_entry = entries_b
            .iter()
            .find(|e| e.kind == EntryKind::Group)
            .unwrap();
        assert_eq!(manga_entry.page_count, 2);
        let manga_detail = db_b.get_group_detail(&manga_entry.id).unwrap();
        assert_eq!(manga_detail.reading_dir, "ltr");
        assert_eq!(manga_detail.spread_mode, "spread");
        assert!(!manga_detail.cover_alone);
        assert_eq!(manga_detail.tags, vec!["アクション".to_string()]);

        let folders_b = db_b.list_folders().unwrap();
        assert_eq!(folders_b.len(), 2);
        let root_b = folders_b.iter().find(|f| f.name == "作家X").unwrap();
        let sub_b = folders_b.iter().find(|f| f.name == "単行本").unwrap();
        assert_eq!(sub_b.parent_id.as_deref(), Some(root_b.id.as_str()));

        // 3. Re-import same package -> must be idempotent
        let imp2 = import_partial_package(
            &mut db_b,
            &PackageImportRequest {
                source_path: pkg_path.display().to_string(),
                passphrase: "share-pass-99".to_string(),
                target_folder_id: None,
            },
        )
        .unwrap();
        assert_eq!(imp2.imported_items, 0);
        assert_eq!(imp2.imported_groups, 0);
        assert_eq!(imp2.skipped_duplicate_groups, 1);
        assert_eq!(imp2.imported_folders, 0);
        assert_eq!(
            db_b.query_entries(&SearchQuery::default()).unwrap().len(),
            2
        );
    }

    #[test]
    fn test_package_export_by_tags_and_entries() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("source.vault");
        // Verify legacy .kvpack extension remains import-compatible as well
        let pkg_path = dir.path().join("tags_only.kvpack");

        let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();
        let img_id = import_id(&mut db, &make_png(40, 40, 10, 20, 30), Some("A"));
        let b = import_id(&mut db, &make_png(40, 40, 50, 20, 30), Some("B"));
        let _c = import_id(&mut db, &make_png(40, 40, 90, 20, 30), Some("C"));
        db.add_tag(&img_id, "お気に入り").unwrap();

        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&20u32.to_be_bytes());
        mp4.extend_from_slice(b"ftyp");
        mp4.extend_from_slice(b"isom");
        mp4.extend_from_slice(&0u32.to_be_bytes());
        mp4.extend_from_slice(b"mp42");
        mp4.extend_from_slice(b"PKG_VIDEO");
        let vid = match db
            .import_media_bytes(
                &mp4,
                Some("Video"),
                Some("https://www.tiktok.com/@pkg/video/77"),
                9_500,
            )
            .unwrap()
        {
            ImportOutcome::Imported { id, .. } => id,
            _ => panic!("expected imported video"),
        };
        db.add_tag(&vid, "お気に入り").unwrap();

        // Empty selection must error cleanly
        let empty_err = export_partial_package(
            &db,
            &PackageExportRequest {
                dest_path: pkg_path.display().to_string(),
                passphrase: "p1".to_string(),
                overwrite: true,
                folder_ids: vec![],
                include_subfolders: true,
                tag_names: vec!["存在しないタグ".to_string()],
                entries: vec![],
            },
        );
        assert!(matches!(empty_err, Err(VaultError::InvalidInput(_))));

        // Combine tag_names + explicit entry
        let rep = export_partial_package(
            &db,
            &PackageExportRequest {
                dest_path: pkg_path.display().to_string(),
                passphrase: "pkg-pass".to_string(),
                overwrite: true,
                folder_ids: vec![],
                include_subfolders: true,
                tag_names: vec!["お気に入り".to_string()],
                entries: vec![EntryRef {
                    kind: EntryKind::Image,
                    id: b,
                }],
            },
        )
        .unwrap();
        assert_eq!(rep.item_count, 3);
        assert_eq!(rep.tag_count, 1);

        let pkg_db = VaultDb::open(&pkg_path, "pkg-pass").unwrap();
        let items = pkg_db
            .query_entries(&SearchQuery {
                scope: QueryScope::All,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(items.len(), 3);
        let pkg_vid = pkg_db.get_item_detail(&vid).unwrap();
        assert_eq!(pkg_vid.duration_ms, 9_500);
        assert_eq!(pkg_vid.source_url, "https://www.tiktok.com/@pkg/video/77");

        // Verify importing video package preserves duration_ms, source_url, and reports video_pending_ids
        let dst_path = dir.path().join("dest.vault");
        let mut dst_db = VaultDb::create(&dst_path, "dst-pass", false).unwrap();
        let imp = import_partial_package(
            &mut dst_db,
            &PackageImportRequest {
                source_path: pkg_path.display().to_string(),
                passphrase: "pkg-pass".to_string(),
                target_folder_id: None,
            },
        )
        .unwrap();
        assert_eq!(imp.imported_items, 3);
        assert_eq!(imp.video_pending_ids, vec![vid.clone()]);
        let imported_vid = dst_db.get_item_detail(&vid).unwrap();
        assert_eq!(imported_vid.duration_ms, 9_500);
        assert_eq!(
            imported_vid.source_url,
            "https://www.tiktok.com/@pkg/video/77"
        );
    }

    #[test]
    fn test_package_export_import_chunked_video() {
        let dir = TempDir::new().unwrap();
        let src_vault = dir.path().join("src_chunk.vault");
        let pkg_path = dir.path().join("chunked.tuckpack");
        let dst_vault = dir.path().join("dst_chunk.vault");
        let video_file = dir.path().join("sample_chunk.mp4");

        let mut payload = Vec::new();
        payload.extend_from_slice(&20u32.to_be_bytes());
        payload.extend_from_slice(b"ftyp");
        payload.extend_from_slice(b"isom");
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(b"mp42");
        for i in 0..10_000u32 {
            payload.push(((i * 19 + 5) % 253) as u8);
        }
        fs::write(&video_file, &payload).unwrap();

        let mut src_db = VaultDb::create(&src_vault, "src-pass", false).unwrap();
        let vid = match src_db
            .import_media_file_with_chunk_params(
                &video_file,
                Some("チャンク動画"),
                Some("https://x.com/example/status/777"),
                180_000,
                1024,
                4096,
            )
            .unwrap()
        {
            ImportOutcome::Imported { id, .. } => id,
            _ => panic!("expected imported chunked video"),
        };

        export_partial_package(
            &src_db,
            &PackageExportRequest {
                dest_path: pkg_path.display().to_string(),
                passphrase: "pkg-chunk-pass".to_string(),
                overwrite: true,
                folder_ids: vec![],
                include_subfolders: true,
                tag_names: vec![],
                entries: vec![EntryRef {
                    kind: EntryKind::Image,
                    id: vid.clone(),
                }],
            },
        )
        .unwrap();

        let mut dst_db = VaultDb::create(&dst_vault, "dst-pass", false).unwrap();
        let imp = import_partial_package(
            &mut dst_db,
            &PackageImportRequest {
                source_path: pkg_path.display().to_string(),
                passphrase: "pkg-chunk-pass".to_string(),
                target_folder_id: None,
            },
        )
        .unwrap();
        assert_eq!(imp.imported_items, 1);
        assert_eq!(imp.video_pending_ids, vec![vid.clone()]);
        assert_eq!(dst_db.get_blob_bytes(&vid).unwrap(), payload);
        let detail = dst_db.get_item_detail(&vid).unwrap();
        assert_eq!(detail.duration_ms, 180_000);
        assert_eq!(detail.source_url, "https://x.com/example/status/777");
    }
}
