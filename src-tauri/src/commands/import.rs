use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, Emitter, State};
use walkdir::WalkDir;

use crate::clipboard;
use crate::db::{read_file_head, ImportOutcome};
use crate::error::VaultError;
use crate::imaging::{detect_mime, is_video_mime};
use crate::models::{
    natural_cmp, EntryKind, EntryRef, ImportReport, ItemDetail, UrlImportSettings, YtdlpStatus,
};
use crate::state::AppState;
use crate::ytdlp;

use super::{
    percent_decode, record_import_outcome, run_blocking, title_from_path, with_open_db,
    ImportGroupReport,
};

#[tauri::command]
pub async fn import_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
    folder_id: Option<String>,
) -> Result<ImportReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let report = run_blocking(move || {
        with_open_db(&db_arc, |db| {
            let mut report = ImportReport::default();
            let mut sorted_paths = paths;
            sorted_paths.sort_by(|a, b| natural_cmp(a, b));
            for raw_path in sorted_paths {
                let p = Path::new(&raw_path);
                if p.is_dir() {
                    let mut files: Vec<PathBuf> = WalkDir::new(p)
                        .follow_links(false)
                        .into_iter()
                        .flatten()
                        .filter(|e| e.path().is_file())
                        .map(|e| e.into_path())
                        .collect();
                    files.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
                    for entry_path in files {
                        let label = entry_path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("file");
                        let title = title_from_path(&entry_path);
                        let res = db.import_media_file(&entry_path, title.as_deref(), None, 0);
                        record_import_outcome(&mut report, res, label);
                    }
                } else if p.is_file() {
                    let label = p.file_name().and_then(|s| s.to_str()).unwrap_or("file");
                    let title = title_from_path(p);
                    let res = db.import_media_file(p, title.as_deref(), None, 0);
                    record_import_outcome(&mut report, res, label);
                } else {
                    report
                        .errors
                        .push(format!("{raw_path}: ファイルが見つかりません"));
                }
            }

            if let Some(fid) = folder_id.as_deref() {
                if !report.imported_ids.is_empty() {
                    let refs: Vec<EntryRef> = report
                        .imported_ids
                        .iter()
                        .map(|id| EntryRef {
                            kind: EntryKind::Image,
                            id: id.clone(),
                        })
                        .collect();
                    db.add_entries_to_folder(fid, &refs)?;
                }
            }

            Ok(report)
        })
    })
    .await?;
    if !report.imported_ids.is_empty() {
        state.notify_lan_library_changed();
    }
    Ok(report)
}

#[tauri::command]
pub async fn import_folder_as_group(
    state: State<'_, AppState>,
    dir_path: String,
    folder_id: Option<String>,
) -> Result<ImportGroupReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let res = run_blocking(move || {
        with_open_db(&db_arc, |db| {
            let root = Path::new(&dir_path);
            if !root.is_dir() {
                return Err(VaultError::InvalidInput(
                    "有効なフォルダを選択してください".to_string(),
                ));
            }

            let mut files: Vec<PathBuf> = WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.into_path())
                .collect();
            files.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));

            let mut report = ImportReport::default();
            for file_path in files {
                let label = file_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file");
                let head = match read_file_head(&file_path, 4096) {
                    Ok(h) => h,
                    Err(e) => {
                        report.errors.push(format!("{label}: {e}"));
                        continue;
                    }
                };
                match detect_mime(&head) {
                    Ok(mime) if is_video_mime(mime) => {
                        report.skipped_unsupported_count += 1;
                        continue;
                    }
                    Err(VaultError::UnsupportedFormat(_)) => {
                        report.skipped_unsupported_count += 1;
                        continue;
                    }
                    Err(e) => {
                        report.errors.push(format!("{label}: {e}"));
                        continue;
                    }
                    Ok(_) => {}
                }
                match fs::read(&file_path) {
                    Ok(bytes) => {
                        let title = title_from_path(&file_path);
                        let res = db.import_bytes(&bytes, title.as_deref());
                        record_import_outcome(&mut report, res, label);
                    }
                    Err(e) => report.errors.push(format!("{label}: {e}")),
                }
            }

            let group_candidate_ids: Vec<String> = report
                .imported_ids
                .iter()
                .filter(|id| !report.video_pending_ids.contains(id))
                .cloned()
                .collect();

            let group = if !group_candidate_ids.is_empty() {
                let group_title = root.file_name().and_then(|s| s.to_str());
                Some(db.create_group_from_items(
                    &group_candidate_ids,
                    group_title,
                    folder_id.as_deref(),
                )?)
            } else {
                None
            };

            Ok(ImportGroupReport { report, group })
        })
    })
    .await?;
    if !res.report.imported_ids.is_empty() || res.group.is_some() {
        state.notify_lan_library_changed();
    }
    Ok(res)
}

#[tauri::command]
pub async fn import_bytes(
    state: State<'_, AppState>,
    request: Request<'_>,
) -> Result<ImportReport, VaultError> {
    let raw_bytes = match request.body() {
        InvokeBody::Raw(bytes) => bytes.clone(),
        InvokeBody::Json(_) => {
            return Err(VaultError::InvalidInput(
                "バイナリ形式での送信が必要です".to_string(),
            ))
        }
    };
    let title = request
        .headers()
        .get("x-title")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .filter(|s| !s.trim().is_empty());
    let folder_id = request
        .headers()
        .get("x-folder-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let db_arc = Arc::clone(&state.db);
    let report = run_blocking(move || {
        with_open_db(&db_arc, |db| {
            let mut report = ImportReport::default();
            match db.import_bytes(&raw_bytes, title.as_deref()) {
                Ok(outcome) => record_import_outcome(&mut report, Ok(outcome), "import_bytes"),
                Err(VaultError::UnsupportedFormat(_)) => {
                    report.skipped_unsupported_count += 1;
                }
                Err(e) => return Err(e),
            }
            if let Some(id) = report.imported_ids.first() {
                if let Some(fid) = folder_id.as_deref() {
                    db.add_entries_to_folder(
                        fid,
                        &[EntryRef {
                            kind: EntryKind::Image,
                            id: id.clone(),
                        }],
                    )?;
                }
            }
            Ok(report)
        })
    })
    .await?;
    if !report.imported_ids.is_empty() {
        state.notify_lan_library_changed();
    }
    Ok(report)
}

#[tauri::command]
pub async fn import_clipboard(
    state: State<'_, AppState>,
    folder_id: Option<String>,
) -> Result<ImportReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let report = run_blocking(move || {
        let png_bytes = clipboard::read_clipboard_image_png()?;
        with_open_db(&db_arc, |db| {
            let mut report = ImportReport::default();
            match db.import_bytes(&png_bytes, None)? {
                ImportOutcome::Imported {
                    id,
                    mime_type,
                    thumb_pending,
                } => {
                    if let Some(fid) = folder_id.as_deref() {
                        db.add_entries_to_folder(
                            fid,
                            &[EntryRef {
                                kind: EntryKind::Image,
                                id: id.clone(),
                            }],
                        )?;
                    }
                    if thumb_pending {
                        if is_video_mime(&mime_type) {
                            report.video_pending_ids.push(id.clone());
                        } else {
                            report.avif_pending_ids.push(id.clone());
                        }
                    }
                    report.imported_ids.push(id);
                }
                ImportOutcome::Duplicate => {
                    report.duplicate_count += 1;
                }
            }
            Ok(report)
        })
    })
    .await?;
    if !report.imported_ids.is_empty() {
        state.notify_lan_library_changed();
    }
    Ok(report)
}

#[tauri::command]
pub async fn save_media_thumbnail(
    state: State<'_, AppState>,
    request: Request<'_>,
) -> Result<ItemDetail, VaultError> {
    let raw_png = match request.body() {
        InvokeBody::Raw(bytes) => bytes.clone(),
        InvokeBody::Json(_) => {
            return Err(VaultError::InvalidInput(
                "サムネイルバイナリが必要です".to_string(),
            ))
        }
    };

    let headers = request.headers();
    let item_id = headers
        .get("x-item-id")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| VaultError::InvalidInput("x-item-id ヘッダが必要です".to_string()))?
        .to_string();
    let width = headers
        .get("x-width")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    let height = headers
        .get("x-height")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    let duration_ms = headers
        .get("x-duration-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);

    let db_arc = Arc::clone(&state.db);
    let detail = run_blocking(move || {
        with_open_db(&db_arc, |db| {
            db.save_media_thumbnail(&item_id, &raw_png, width, height, duration_ms)
        })
    })
    .await?;
    state.notify_lan_library_changed();
    Ok(detail)
}

#[tauri::command]
pub fn set_url_import_settings(
    state: State<'_, AppState>,
    settings: UrlImportSettings,
) -> Result<UrlImportSettings, VaultError> {
    let res = with_open_db(&state.db, |db| db.set_url_import_settings(&settings))?;
    state.notify_lan_library_changed();
    Ok(res)
}

struct UrlImportSessionGuard {
    state: AppState,
    token: Arc<AtomicBool>,
}

impl Drop for UrlImportSessionGuard {
    fn drop(&mut self) {
        self.state.finish_url_import(&self.token);
    }
}

#[tauri::command]
pub async fn check_ytdlp_status(
    custom_path: Option<String>,
    custom_ffmpeg_path: Option<String>,
) -> Result<YtdlpStatus, VaultError> {
    run_blocking(move || {
        Ok(ytdlp::check_ytdlp_status(
            custom_path.as_deref().unwrap_or(""),
            custom_ffmpeg_path.as_deref().unwrap_or(""),
        ))
    })
    .await
}

#[tauri::command]
pub fn cancel_url_import(state: State<'_, AppState>) -> Result<bool, VaultError> {
    Ok(state.cancel_url_import())
}

#[tauri::command]
pub async fn import_url(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    folder_id: Option<String>,
) -> Result<ImportReport, VaultError> {
    let state_clone = state.inner().clone();
    let url_settings = with_open_db(&state_clone.db, |db| Ok(db.get_url_import_settings()))?;
    let cancel_token = state_clone.begin_url_import()?;
    let guard = UrlImportSessionGuard {
        state: state_clone.clone(),
        token: Arc::clone(&cancel_token),
    };

    let state_for_notify = state_clone.clone();
    let report = run_blocking(move || {
        let _guard = guard;
        let batch = ytdlp::download_from_url(&url, &url_settings, &cancel_token, |ev| {
            let _ = app.emit("vault://url-import-progress", &ev);
            state_clone.notify_lan_url_progress(&ev);
        })?;

        with_open_db(&state_clone.db, |db| {
            ytdlp::import_downloaded_url_batch(db, &batch, folder_id.as_deref(), |ev| {
                let _ = app.emit("vault://url-import-progress", &ev);
                state_clone.notify_lan_url_progress(&ev);
            })
        })
    })
    .await?;
    if !report.imported_ids.is_empty() {
        state_for_notify.notify_lan_library_changed();
    }
    Ok(report)
}
