use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::crypto::normalize_nfc_trimmed;
use crate::db::{now_ms, paths_refer_to_same_file, prune_orphan_tags, upsert_tag, VaultDb};
use crate::error::VaultError;
use crate::imaging::is_video_mime;
use crate::models::{PackageImportReport, PackageImportRequest};

use super::{topological_folders, RawFolderRow};

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
        let trashed_gids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT g.id
                 FROM group_items gi
                 JOIN groups g ON g.id = gi.group_id
                 WHERE gi.item_id = ?1 AND g.deleted_at IS NOT NULL",
            )?;
            let rows = stmt.query_map(params![existing_id], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for gid in trashed_gids {
            tx.execute(
                "UPDATE groups SET deleted_at = NULL, updated_at = ?2 WHERE id = ?1",
                params![gid, now],
            )?;
            tx.execute(
                "UPDATE items SET deleted_at = NULL, updated_at = ?2
                 WHERE id IN (SELECT item_id FROM group_items WHERE group_id = ?1)
                   AND deleted_at IS NOT NULL",
                params![gid, now],
            )?;
        }
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
    if paths_refer_to_same_file(source_path, dest.path()) {
        return Err(VaultError::InvalidInput(
            "現在開いている保管庫自体は取り込めません".to_string(),
        ));
    }

    let src_db = VaultDb::open_readonly(source_path, &req.passphrase)?;
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

        let norm_name = normalize_nfc_trimmed(&sf.name);
        let folder_name = if norm_name.is_empty() {
            sf.name.trim().to_string()
        } else {
            norm_name
        };

        let existing_fid: Option<String> = tx
            .query_row(
                "SELECT id FROM folders WHERE parent_id IS ?1 AND name = ?2 COLLATE NOCASE",
                params![dest_parent, folder_name],
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
                params![new_fid, dest_parent, folder_name, sf.color, next_order, sf.created_at, now],
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
            let (gid_opt, mime_opt): (Option<String>, Option<String>) = tx
                .query_row(
                    "SELECT (SELECT gi.group_id FROM group_items gi WHERE gi.item_id = i.id LIMIT 1), i.mime_type
                     FROM items i WHERE i.id = ?1",
                    params![dest_pid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .unwrap_or((None, None));
            if let Some(gid) = gid_opt {
                existing_groups_for_pages.insert(gid);
            } else if mime_opt.as_deref().is_some_and(is_video_mime) {
                continue;
            } else if !free_pages.contains(dest_pid) {
                free_pages.push(dest_pid.clone());
            }
        }

        let dest_group_id: Option<String> = if free_pages.is_empty()
            && existing_groups_for_pages.len() == 1
        {
            // Exact duplicate manga group -> restore if trashed, merge empty metadata & tags/folders
            let existing_gid = existing_groups_for_pages
                .into_iter()
                .next()
                .expect("checked len == 1");
            let cur_group: Option<(String, String, i32)> = tx
                .query_row(
                    "SELECT title, memo, rating FROM groups WHERE id = ?1",
                    params![existing_gid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if let Some((cur_title, cur_memo, cur_rating)) = cur_group {
                let next_title = if cur_title.trim().is_empty() && !sg.title.trim().is_empty() {
                    sg.title.clone()
                } else {
                    cur_title
                };
                let next_memo = if cur_memo.trim().is_empty() && !sg.memo.trim().is_empty() {
                    sg.memo.clone()
                } else {
                    cur_memo
                };
                let next_rating = if cur_rating == 0 && sg.rating > 0 {
                    sg.rating
                } else {
                    cur_rating
                };
                tx.execute(
                    "UPDATE groups
                     SET title = ?2, memo = ?3, rating = ?4, deleted_at = NULL, updated_at = ?5
                     WHERE id = ?1",
                    params![existing_gid, next_title, next_memo, next_rating, now],
                )?;
                tx.execute(
                    "UPDATE items SET deleted_at = NULL, updated_at = ?2
                     WHERE id IN (SELECT item_id FROM group_items WHERE group_id = ?1)
                       AND deleted_at IS NOT NULL",
                    params![existing_gid, now],
                )?;
            }
            report.skipped_duplicate_groups += 1;
            Some(existing_gid)
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
                let migrated_tags = tx.execute(
                    "INSERT OR IGNORE INTO group_tags (group_id, tag_id)
                     SELECT ?1, tag_id FROM item_tags WHERE item_id = ?2",
                    params![new_gid, dest_pid],
                )?;
                report.merged_tag_links += migrated_tags;
                tx.execute(
                    "INSERT OR IGNORE INTO folder_groups (folder_id, group_id, added_at)
                     SELECT folder_id, ?1, ?3 FROM folder_items WHERE item_id = ?2",
                    params![new_gid, dest_pid, now],
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

    prune_orphan_tags(&tx)?;
    tx.commit()?;
    drop(src_db);
    dest.refresh_last_item_created_at()?;
    Ok(report)
}
