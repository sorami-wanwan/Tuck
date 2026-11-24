use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::db::{atomic_replace_file, paths_refer_to_same_file, VaultDb};
use crate::error::VaultError;
use crate::models::{PlainExportBatchRequest, PlainExportItemRequest, PlainExportReport};

use super::plan::build_plain_export_plan;

fn stream_item_via_temp_file(
    db: &VaultDb,
    item_id: &str,
    dest_path: &Path,
    overwrite: bool,
) -> Result<u64, VaultError> {
    if paths_refer_to_same_file(dest_path, db.path()) {
        return Err(VaultError::InvalidInput(
            "現在開いている保管庫ファイルは上書きできません".to_string(),
        ));
    }
    let parent = dest_path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(p) = parent {
        fs::create_dir_all(p)?;
    }

    let tmp_name = format!(".tuck-export-{}.tmp", Uuid::new_v4());
    let tmp_path = match parent {
        Some(p) => p.join(&tmp_name),
        None => PathBuf::from(&tmp_name),
    };

    let write_res = (|| -> Result<u64, VaultError> {
        let file = File::create(&tmp_path)?;
        let mut writer = BufWriter::with_capacity(256 * 1024, file);
        let written = db.write_blob_to_writer(item_id, &mut writer)?;
        writer.flush()?;
        writer
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        Ok(written)
    })();

    let written = match write_res {
        Ok(n) => n,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    if let Err(e) = atomic_replace_file(&tmp_path, dest_path, overwrite) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    Ok(written)
}

pub fn export_plain_item(
    db: &VaultDb,
    req: &PlainExportItemRequest,
) -> Result<PlainExportReport, VaultError> {
    let trimmed = req.dest_path.trim();
    if trimmed.is_empty() {
        return Err(VaultError::InvalidInput(
            "書き出し先のファイルパスを指定してください".to_string(),
        ));
    }
    let dest_path = Path::new(trimmed);
    if paths_refer_to_same_file(dest_path, db.path()) {
        return Err(VaultError::InvalidInput(
            "現在開いている保管庫と同じファイルへは書き出せません".to_string(),
        ));
    }
    if dest_path.exists() && !req.overwrite {
        return Err(VaultError::VaultAlreadyExists(
            dest_path.display().to_string(),
        ));
    }

    let meta = db.get_export_item_meta(&req.item_id)?;
    if meta.deleted_at.is_some() {
        return Err(VaultError::InvalidInput(
            "ゴミ箱内のアイテムは書き出せません".to_string(),
        ));
    }

    let written = stream_item_via_temp_file(db, &meta.id, dest_path, req.overwrite)?;
    Ok(PlainExportReport {
        file_count: 1,
        group_count: 0,
        folder_count: 0,
        total_bytes: written,
        dest_path: dest_path.display().to_string(),
    })
}

fn unique_disk_path(root: &Path, rel_segments: &[String], vault_path: &Path) -> PathBuf {
    let mut dir = root.to_path_buf();
    if rel_segments.len() > 1 {
        for seg in &rel_segments[..rel_segments.len() - 1] {
            dir.push(seg);
        }
    }
    let filename = rel_segments
        .last()
        .map(String::as_str)
        .unwrap_or("export.bin");

    let direct = dir.join(filename);
    if !direct.exists() && !paths_refer_to_same_file(&direct, vault_path) {
        return direct;
    }

    let (stem, ext_with_dot) = match filename.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (filename, String::new()),
    };
    let mut idx = 2usize;
    loop {
        let candidate = dir.join(format!("{stem} ({idx}){ext_with_dot}"));
        if !candidate.exists() && !paths_refer_to_same_file(&candidate, vault_path) {
            return candidate;
        }
        idx += 1;
    }
}

pub fn export_plain_batch(
    db: &VaultDb,
    req: &PlainExportBatchRequest,
) -> Result<PlainExportReport, VaultError> {
    let trimmed_dir = req.dest_dir.trim();
    if trimmed_dir.is_empty() {
        return Err(VaultError::InvalidInput(
            "書き出し先のフォルダを指定してください".to_string(),
        ));
    }
    let dest_dir = Path::new(trimmed_dir);
    if !dest_dir.exists() || !dest_dir.is_dir() {
        return Err(VaultError::InvalidInput(
            "指定された書き出し先フォルダが見つかりません".to_string(),
        ));
    }

    let plan = build_plain_export_plan(db, &req.entries, &req.folder_ids, req.include_subfolders)?;
    let mut total_written = 0u64;

    for file_entry in &plan.files {
        let out_path = unique_disk_path(dest_dir, &file_entry.rel_segments, db.path());
        let written = stream_item_via_temp_file(db, &file_entry.item_id, &out_path, false)?;
        total_written = total_written.saturating_add(written);
    }

    Ok(PlainExportReport {
        file_count: plan.files.len(),
        group_count: plan.group_count,
        folder_count: plan.folder_count,
        total_bytes: total_written,
        dest_path: dest_dir.display().to_string(),
    })
}
