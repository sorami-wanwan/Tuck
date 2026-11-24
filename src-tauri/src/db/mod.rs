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

use rusqlite::{params, types::Value, Connection, OpenFlags, OptionalExtension};
use uuid::Uuid;

use crate::crypto::{apply_sqlcipher_key, validate_passphrase};
use crate::error::VaultError;
use crate::models::{BackupReport, OptimizeReport, VaultHealthInfo, VaultStatus};

use schema::{
    PERFORMANCE_INDEXES, SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_VERSION_V1,
    SCHEMA_VERSION_V2, SCHEMA_VERSION_V3, SCHEMA_VERSION_V4,
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

pub(crate) fn read_file_head(path: &Path, max_len: usize) -> Result<Vec<u8>, io::Error> {
    let mut file = fs::File::open(path)?;
    let mut buf = vec![0u8; max_len];
    let n = read_up_to(&mut file, &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

pub(crate) fn paths_refer_to_same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if let (Ok(ca), Ok(cb)) = (fs::canonicalize(a), fs::canonicalize(b)) {
        return ca == cb;
    }
    if let (Some(ap), Some(an), Some(bp), Some(bn)) =
        (a.parent(), a.file_name(), b.parent(), b.file_name())
    {
        let same_name = if cfg!(any(target_os = "macos", target_os = "windows")) {
            an.to_string_lossy()
                .eq_ignore_ascii_case(&bn.to_string_lossy())
        } else {
            an == bn
        };
        if same_name {
            let ap_resolved = if ap.as_os_str().is_empty() {
                Path::new(".")
            } else {
                ap
            };
            let bp_resolved = if bp.as_os_str().is_empty() {
                Path::new(".")
            } else {
                bp
            };
            if let (Ok(cap), Ok(cbp)) = (
                fs::canonicalize(ap_resolved),
                fs::canonicalize(bp_resolved),
            ) {
                return cap == cbp;
            }
        }
    }
    false
}

pub(crate) fn atomic_replace_file(
    tmp_path: &Path,
    dest_path: &Path,
    overwrite: bool,
) -> Result<(), VaultError> {
    if !overwrite && dest_path.exists() {
        let _ = fs::remove_file(tmp_path);
        return Err(VaultError::VaultAlreadyExists(
            dest_path.display().to_string(),
        ));
    }

    #[cfg(not(unix))]
    if overwrite && dest_path.exists() {
        if let Err(e) = fs::remove_file(dest_path) {
            let _ = fs::remove_file(tmp_path);
            return Err(e.into());
        }
    }

    if let Err(e) = fs::rename(tmp_path, dest_path) {
        let _ = fs::remove_file(tmp_path);
        return Err(e.into());
    }
    Ok(())
}

pub(crate) struct TempVaultCopy(PathBuf);

impl Drop for TempVaultCopy {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let journal = PathBuf::from(format!("{}-journal", self.0.display()));
        if journal.exists() {
            let _ = fs::remove_file(journal);
        }
    }
}

pub struct ReadonlyVaultHandle {
    db: VaultDb,
    _temp_file: Option<TempVaultCopy>,
}

impl ReadonlyVaultHandle {
    pub(crate) fn conn(&self) -> &Connection {
        &self.db.conn
    }
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
    last_item_created_at: i64,
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
        validate_passphrase(passphrase)?;

        if path.exists() && !overwrite {
            return Err(VaultError::VaultAlreadyExists(path.display().to_string()));
        }

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }

        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("vault.vault");
        let tmp_path = path.with_file_name(format!(".{file_name}.init.tmp.{}", Uuid::new_v4()));

        let init_res = (|| -> Result<(), VaultError> {
            let mut conn = Connection::open(&tmp_path)?;
            apply_sqlcipher_key(&conn, passphrase)?;
            apply_runtime_pragmas(&conn)?;

            let tx = conn.transaction()?;
            tx.execute_batch(SCHEMA_V1)?;
            tx.execute_batch(SCHEMA_V2)?;
            tx.execute_batch(SCHEMA_V3)?;
            tx.execute_batch(SCHEMA_V4)?;
            tx.execute_batch(PERFORMANCE_INDEXES)?;
            let created = now_ms().to_string();
            let vault_uuid = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO vault_meta (key, value) VALUES (?1, ?2)",
                params!["schema_version", SCHEMA_VERSION_V4],
            )?;
            tx.execute(
                "INSERT INTO vault_meta (key, value) VALUES (?1, ?2)",
                params!["vault_uuid", vault_uuid],
            )?;
            tx.execute(
                "INSERT INTO vault_meta (key, value) VALUES (?1, ?2)",
                params!["created_at", created],
            )?;
            tx.commit()?;
            drop(conn);
            fs::OpenOptions::new()
                .write(true)
                .open(&tmp_path)?
                .sync_all()?;
            Ok(())
        })();

        if let Err(e) = init_res {
            let _ = fs::remove_file(&tmp_path);
            let tmp_journal = PathBuf::from(format!("{}-journal", tmp_path.display()));
            if tmp_journal.exists() {
                let _ = fs::remove_file(tmp_journal);
            }
            return Err(e);
        }

        atomic_replace_file(&tmp_path, path, overwrite)?;
        let journal_path = PathBuf::from(format!("{}-journal", path.display()));
        if journal_path.exists() {
            let _ = fs::remove_file(journal_path);
        }

        let conn = Connection::open(path)?;
        apply_sqlcipher_key(&conn, passphrase)?;
        apply_runtime_pragmas(&conn)?;

        Ok(Self {
            conn,
            path: path.to_path_buf(),
            last_item_created_at: 0,
        })
    }

    pub fn open_readonly(
        path: &Path,
        passphrase: &str,
    ) -> Result<ReadonlyVaultHandle, VaultError> {
        if !path.exists() {
            return Err(VaultError::VaultNotFound(path.display().to_string()));
        }
        let meta = fs::metadata(path)?;
        if meta.len() == 0 {
            return Err(VaultError::WrongPassword);
        }

        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
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

        match version.as_deref() {
            Some(SCHEMA_VERSION_V4) => {
                conn.execute_batch("PRAGMA temp_store = MEMORY; PRAGMA query_only = ON;")?;
                Ok(ReadonlyVaultHandle {
                    db: Self {
                        conn,
                        path: path.to_path_buf(),
                        last_item_created_at: 0,
                    },
                    _temp_file: None,
                })
            }
            Some(SCHEMA_VERSION_V1) | Some(SCHEMA_VERSION_V2) | Some(SCHEMA_VERSION_V3) => {
                drop(conn);
                let tmp_path =
                    std::env::temp_dir().join(format!("tuck-pkg-mig-{}.vault", Uuid::new_v4()));
                let copy_guard = TempVaultCopy(tmp_path.clone());
                {
                    let mut src = fs::File::open(path)?;
                    let mut dst = fs::File::create(&tmp_path)?;
                    io::copy(&mut src, &mut dst)?;
                    dst.sync_all()?;
                }
                let migrated = Self::open(&tmp_path, passphrase)?;
                Ok(ReadonlyVaultHandle {
                    db: migrated,
                    _temp_file: Some(copy_guard),
                })
            }
            _ => Err(VaultError::WrongPassword),
        }
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
        conn.execute_batch(PERFORMANCE_INDEXES)?;

        let last_item_created_at: i64 =
            conn.query_row("SELECT COALESCE(MAX(created_at), 0) FROM items", [], |r| {
                r.get(0)
            })?;

        Ok(Self {
            conn,
            path: path.to_path_buf(),
            last_item_created_at,
        })
    }

    /// Returns a strictly monotonic timestamp in ms for new item inserts so bulk imports keep deterministic order.
    pub(super) fn next_item_created_at(&mut self) -> i64 {
        let now = now_ms();
        let next = if now <= self.last_item_created_at {
            self.last_item_created_at + 1
        } else {
            now
        };
        self.last_item_created_at = next;
        next
    }

    pub(crate) fn refresh_last_item_created_at(&mut self) -> Result<(), VaultError> {
        let max_created: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(created_at), 0) FROM items", [], |r| {
                    r.get(0)
                })?;
        if max_created > self.last_item_created_at {
            self.last_item_created_at = max_created;
        }
        Ok(())
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
        if paths_refer_to_same_file(dest_path, &self.path) {
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

        atomic_replace_file(&tmp_path, dest_path, overwrite)?;

        Ok(BackupReport { bytes_written })
    }

    pub fn status(&self) -> Result<VaultStatus, VaultError> {
        let (standalone_active, uncategorized_items, untagged_items, trash_items, total_bytes): (
            i64,
            i64,
            i64,
            i64,
            i64,
        ) = self.conn.query_row(
            "SELECT
                 COALESCE(SUM(CASE
                     WHEN i.deleted_at IS NULL
                      AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)
                     THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE
                     WHEN i.deleted_at IS NULL
                      AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)
                      AND NOT EXISTS (SELECT 1 FROM folder_items fi WHERE fi.item_id = i.id)
                     THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE
                     WHEN i.deleted_at IS NULL
                      AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)
                      AND NOT EXISTS (SELECT 1 FROM item_tags it WHERE it.item_id = i.id)
                     THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE
                     WHEN i.deleted_at IS NOT NULL
                      AND NOT EXISTS (SELECT 1 FROM group_items gi WHERE gi.item_id = i.id)
                     THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN i.deleted_at IS NULL THEN i.byte_size ELSE 0 END), 0)
             FROM items i",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;

        let (groups_active, uncategorized_groups, untagged_groups, trash_groups): (
            i64,
            i64,
            i64,
            i64,
        ) = self.conn.query_row(
            "SELECT
                 COALESCE(SUM(CASE WHEN g.deleted_at IS NULL THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE
                     WHEN g.deleted_at IS NULL
                      AND NOT EXISTS (SELECT 1 FROM folder_groups fg WHERE fg.group_id = g.id)
                     THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE
                     WHEN g.deleted_at IS NULL
                      AND NOT EXISTS (SELECT 1 FROM group_tags gt WHERE gt.group_id = g.id)
                     THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN g.deleted_at IS NOT NULL THEN 1 ELSE 0 END), 0)
             FROM groups g",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
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
