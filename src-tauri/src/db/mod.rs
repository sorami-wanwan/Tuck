mod folders;
mod groups;
mod media;
mod query;
mod schema;
mod settings;
#[cfg(test)]
mod tests;

pub use media::ExportItemMeta;

use std::fs;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, types::Value, Connection, OptionalExtension};
use uuid::Uuid;

use crate::crypto::apply_sqlcipher_key;
use crate::error::VaultError;
use crate::models::{BackupReport, OptimizeReport, VaultHealthInfo, VaultStatus};

use schema::{
    SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_VERSION_V1, SCHEMA_VERSION_V2,
    SCHEMA_VERSION_V3, SCHEMA_VERSION_V4,
};

pub(super) fn read_up_to<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize, io::Error> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

pub enum ImportOutcome {
    Imported {
        id: String,
        mime_type: String,
        thumb_pending: bool,
    },
    Duplicate,
}

pub struct VaultDb {
    conn: Connection,
    path: PathBuf,
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn apply_runtime_pragmas(conn: &Connection) -> Result<(), VaultError> {
    conn.execute_batch(
        "
        PRAGMA temp_store = MEMORY;
        PRAGMA secure_delete = ON;
        PRAGMA journal_mode = DELETE;
        PRAGMA synchronous = FULL;
        PRAGMA foreign_keys = ON;
        ",
    )?;
    Ok(())
}

pub(super) fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

pub(crate) fn parse_palette(json_str: &str) -> Vec<String> {
    serde_json::from_str(json_str).unwrap_or_default()
}

pub(super) fn memo_preview(memo: &str) -> String {
    let mut preview: String = memo.chars().take(120).collect();
    if memo.chars().count() > 120 {
        preview.push('…');
    }
    preview
}

pub(crate) fn prune_orphan_tags(tx: &rusqlite::Transaction<'_>) -> Result<(), VaultError> {
    tx.execute(
        "DELETE FROM tags
         WHERE NOT EXISTS (SELECT 1 FROM item_tags it WHERE it.tag_id = tags.id)
           AND NOT EXISTS (SELECT 1 FROM group_tags gt WHERE gt.tag_id = tags.id)",
        [],
    )?;
    Ok(())
}

pub(crate) fn upsert_tag(
    tx: &rusqlite::Transaction<'_>,
    normalized: &str,
) -> Result<i64, VaultError> {
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
            params![normalized],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    tx.execute("INSERT INTO tags (name) VALUES (?1)", params![normalized])?;
    Ok(tx.last_insert_rowid())
}

impl VaultDb {
    pub fn create(path: &Path, passphrase: &str, overwrite: bool) -> Result<Self, VaultError> {
        if path.exists() {
            if !overwrite {
                return Err(VaultError::VaultAlreadyExists(path.display().to_string()));
            }
            fs::remove_file(path)?;
            let journal_path_str = format!("{}-journal", path.display());
            let journal_path = Path::new(&journal_path_str);
            if journal_path.exists() {
                let _ = fs::remove_file(journal_path);
            }
        }

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }

        let init_result = (|| -> Result<Self, VaultError> {
            let conn = Connection::open(path)?;
            apply_sqlcipher_key(&conn, passphrase)?;
            apply_runtime_pragmas(&conn)?;

            conn.execute_batch(SCHEMA_V1)?;
            conn.execute_batch(SCHEMA_V2)?;
            conn.execute_batch(SCHEMA_V3)?;
            conn.execute_batch(SCHEMA_V4)?;
            let created = now_ms().to_string();
            let vault_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO vault_meta (key, value) VALUES (?1, ?2)",
                params!["schema_version", SCHEMA_VERSION_V4],
            )?;
            conn.execute(
                "INSERT INTO vault_meta (key, value) VALUES (?1, ?2)",
                params!["vault_uuid", vault_uuid],
            )?;
            conn.execute(
                "INSERT INTO vault_meta (key, value) VALUES (?1, ?2)",
                params!["created_at", created],
            )?;

            Ok(Self {
                conn,
                path: path.to_path_buf(),
            })
        })();

        if init_result.is_err() && path.exists() {
            let _ = fs::remove_file(path);
        }
        init_result
    }

    pub fn open(path: &Path, passphrase: &str) -> Result<Self, VaultError> {
        if !path.exists() {
            return Err(VaultError::VaultNotFound(path.display().to_string()));
        }
        let meta = fs::metadata(path)?;
        if meta.len() == 0 {
            return Err(VaultError::WrongPassword);
        }

        let mut conn = Connection::open(path)?;
        apply_sqlcipher_key(&conn, passphrase)?;

        let verify: Result<i64, _> =
            conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0));
        if verify.is_err() {
            return Err(VaultError::WrongPassword);
        }

        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM vault_meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| VaultError::WrongPassword)?;

        apply_runtime_pragmas(&conn)?;

        match version.as_deref() {
            Some(SCHEMA_VERSION_V1) => {
                let tx = conn.transaction()?;
                tx.execute_batch(SCHEMA_V2)?;
                tx.execute_batch(SCHEMA_V3)?;
                tx.execute_batch(SCHEMA_V4)?;
                tx.execute(
                    "UPDATE vault_meta SET value = ?1 WHERE key = 'schema_version'",
                    params![SCHEMA_VERSION_V4],
                )?;
                tx.commit()?;
            }
            Some(SCHEMA_VERSION_V2) => {
                let tx = conn.transaction()?;
                tx.execute_batch(SCHEMA_V3)?;
                tx.execute_batch(SCHEMA_V4)?;
                tx.execute(
                    "UPDATE vault_meta SET value = ?1 WHERE key = 'schema_version'",
                    params![SCHEMA_VERSION_V4],
                )?;
                tx.commit()?;
            }
            Some(SCHEMA_VERSION_V3) => {
                let tx = conn.transaction()?;
                tx.execute_batch(SCHEMA_V4)?;
                tx.execute(
                    "UPDATE vault_meta SET value = ?1 WHERE key = 'schema_version'",
                    params![SCHEMA_VERSION_V4],
                )?;
                tx.commit()?;
            }
            Some(SCHEMA_VERSION_V4) => {}
            _ => return Err(VaultError::WrongPassword),
        }

        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn health_info(&self) -> Result<VaultHealthInfo, VaultError> {
        let pragma_i64 = |sql: &str| -> Result<i64, VaultError> {
            let val: Value = self.conn.query_row(sql, [], |r| r.get(0))?;
            match val {
                Value::Integer(n) => Ok(n),
                Value::Text(s) => Ok(s.trim().parse::<i64>().unwrap_or(0)),
                _ => Ok(0),
            }
        };

        let file_size_bytes = fs::metadata(&self.path)?.len();
        let page_count = pragma_i64("PRAGMA page_count;")?;
        let freelist_count = pragma_i64("PRAGMA freelist_count;")?;
        let page_size = pragma_i64("PRAGMA page_size;")?;
        let journal_mode: String = self
            .conn
            .query_row("PRAGMA journal_mode;", [], |r| r.get(0))?;

        let mut stmt = self.conn.prepare("PRAGMA quick_check;")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut checks = Vec::new();
        for row in rows {
            checks.push(row?);
        }
        let integrity_ok = checks.len() == 1 && checks[0].eq_ignore_ascii_case("ok");
        let integrity_detail = if checks.is_empty() {
            "unknown".to_string()
        } else {
            checks.join("; ")
        };

        let reclaimable_bytes = (freelist_count.max(0) as u64) * (page_size.max(0) as u64);

        Ok(VaultHealthInfo {
            file_size_bytes,
            page_count,
            freelist_count,
            page_size,
            reclaimable_bytes,
            integrity_ok,
            integrity_detail,
            journal_mode,
        })
    }

    pub fn optimize(&mut self) -> Result<OptimizeReport, VaultError> {
        let size_before_bytes = fs::metadata(&self.path)?.len();
        apply_runtime_pragmas(&self.conn)?;
        self.conn.execute_batch("VACUUM;")?;
        let health = self.health_info()?;
        let size_after_bytes = health.file_size_bytes;
        let reclaimed_bytes = size_before_bytes.saturating_sub(size_after_bytes);
        Ok(OptimizeReport {
            size_before_bytes,
            size_after_bytes,
            reclaimed_bytes,
            integrity_ok: health.integrity_ok,
        })
    }

    pub fn backup_to(&self, dest_path: &Path, overwrite: bool) -> Result<BackupReport, VaultError> {
        if dest_path == self.path {
            return Err(VaultError::InvalidInput(
                "現在開いている保管庫と同じファイルへはバックアップできません".to_string(),
            ));
        }
        if dest_path.exists() && !overwrite {
            return Err(VaultError::VaultAlreadyExists(
                dest_path.display().to_string(),
            ));
        }

        let health = self.health_info()?;
        if !health.integrity_ok {
            return Err(VaultError::InvalidInput(format!(
                "保管庫の整合性チェックに失敗しました: {}",
                health.integrity_detail
            )));
        }

        if let Some(parent) = dest_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }

        let file_name = dest_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("backup.vault");
        let tmp_path = dest_path.with_file_name(format!(".{file_name}.tmp.{}", Uuid::new_v4()));

        let copy_res = (|| -> Result<u64, VaultError> {
            let src_file = fs::File::open(&self.path)?;
            let mut reader = BufReader::with_capacity(256 * 1024, src_file);
            let dst_file = fs::File::create(&tmp_path)?;
            let mut writer = BufWriter::with_capacity(256 * 1024, dst_file);
            let bytes = io::copy(&mut reader, &mut writer)?;
            writer.flush()?;
            writer
                .into_inner()
                .map_err(|e| e.into_error())?
                .sync_all()?;
            Ok(bytes)
        })();

        let bytes_written = match copy_res {
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

        Ok(BackupReport { bytes_written })
    }

    pub fn status(&self) -> Result<VaultStatus, VaultError> {
        let standalone_active: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM items i
             WHERE i.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)",
            [],
            |r| r.get(0),
        )?;
        let groups_active: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM groups WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )?;

        let uncategorized_items: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM items i
             WHERE i.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)
               AND NOT EXISTS (SELECT 1 FROM folder_items fi WHERE fi.item_id = i.id)",
            [],
            |r| r.get(0),
        )?;
        let uncategorized_groups: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM groups g
             WHERE g.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM folder_groups fg WHERE fg.group_id = g.id)",
            [],
            |r| r.get(0),
        )?;

        let untagged_items: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM items i
             WHERE i.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)
               AND NOT EXISTS (SELECT 1 FROM item_tags it WHERE it.item_id = i.id)",
            [],
            |r| r.get(0),
        )?;
        let untagged_groups: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM groups g
             WHERE g.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM group_tags gt WHERE gt.group_id = g.id)",
            [],
            |r| r.get(0),
        )?;

        let trash_items: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM items i
             WHERE i.deleted_at IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)",
            [],
            |r| r.get(0),
        )?;
        let trash_groups: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM groups WHERE deleted_at IS NOT NULL",
            [],
            |r| r.get(0),
        )?;

        let total_bytes: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(byte_size), 0) FROM items WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )?;

        Ok(VaultStatus {
            is_open: true,
            can_unlock: false,
            item_count: standalone_active + groups_active,
            uncategorized_count: uncategorized_items + uncategorized_groups,
            untagged_count: untagged_items + untagged_groups,
            trash_count: trash_items + trash_groups,
            total_bytes,
            clipboard_watch_enabled: false,
            skip_plain_export_confirm: self.get_skip_plain_export_confirm(),
            lock_settings: self.get_lock_settings(),
            url_import_settings: self.get_url_import_settings(),
        })
    }
}
