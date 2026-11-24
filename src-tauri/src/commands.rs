use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Emitter, State};
use walkdir::WalkDir;

use crate::clipboard;
use crate::db::{ImportOutcome, VaultDb};
use crate::error::VaultError;
use crate::imaging::is_video_mime;
use crate::lan_server::{list_lan_interfaces, start_lan_server, AssetSource, LanHostNotifier};
use crate::models::{
    natural_cmp, BackupReport, BatchTagReport, EntryKind, EntryRef, FolderNode, GalleryEntry,
    GroupDetail, ImportReport, ItemDetail, LanAccessMode, LanInterfaceInfo, LanShareStatus,
    LockSettings, OptimizeReport, PackageExportReport, PackageExportRequest, PackageImportReport,
    PackageImportRequest, PlainExportBatchRequest, PlainExportItemRequest, PlainExportReport,
    SearchQuery, StartLanShareRequest, TagWithCount, UrlImportSettings, VaultHealthInfo,
    VaultStatus, YtdlpStatus,
};
use crate::package;
use crate::plain_export;
use crate::state::AppState;
use crate::ytdlp;
use crate::{clear_global_lock_shortcuts, sync_global_lock_shortcut};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportGroupReport {
    pub report: ImportReport,
    pub group: Option<GroupDetail>,
}

fn with_open_db<F, T>(db_arc: &Arc<Mutex<Option<VaultDb>>>, f: F) -> Result<T, VaultError>
where
    F: FnOnce(&mut VaultDb) -> Result<T, VaultError>,
{
    let mut guard = db_arc
        .lock()
        .map_err(|_| VaultError::InvalidInput("内部状態ロックの取得に失敗しました".to_string()))?;
    let db = guard.as_mut().ok_or(VaultError::VaultNotOpen)?;
    f(db)
}

async fn run_blocking<F, T>(f: F) -> Result<T, VaultError>
where
    F: FnOnce() -> Result<T, VaultError> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| VaultError::InvalidInput(format!("バックグラウンド処理に失敗しました: {e}")))?
}

fn title_from_path(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
}

pub(crate) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex_slice = &input[i + 1..i + 3];
            if let Ok(val) = u8::from_str_radix(hex_slice, 16) {
                out.push(val);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn record_import_outcome(
    report: &mut ImportReport,
    outcome: Result<ImportOutcome, VaultError>,
    label: &str,
) {
    match outcome {
        Ok(ImportOutcome::Imported {
            id,
            mime_type,
            thumb_pending,
        }) => {
            if thumb_pending {
                if is_video_mime(&mime_type) {
                    report.video_pending_ids.push(id.clone());
                } else {
                    report.avif_pending_ids.push(id.clone());
                }
            }
            report.imported_ids.push(id);
        }
        Ok(ImportOutcome::Duplicate) => {
            report.duplicate_count += 1;
        }
        Err(VaultError::UnsupportedFormat(_)) => {
            report.skipped_unsupported_count += 1;
        }
        Err(e) => {
            report.errors.push(format!("{label}: {e}"));
        }
    }
}

#[tauri::command]
pub async fn create_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    passphrase: String,
    overwrite: bool,
) -> Result<VaultStatus, VaultError> {
    let state_clone = state.inner().clone();
    let status = run_blocking(move || {
        state_clone.close_vault();
        let db = VaultDb::create(Path::new(&path), &passphrase, overwrite)?;
        let status = db.status()?;
        {
            let mut p = state_clone.locked_path.lock().map_err(|_| {
                VaultError::InvalidInput("内部状態ロックの取得に失敗しました".to_string())
            })?;
            *p = Some(PathBuf::from(path));
        }
        {
            let mut guard = state_clone.lock_db()?;
            *guard = Some(db);
        }
        Ok(status)
    })
    .await?;

    sync_global_lock_shortcut(&app, &status.lock_settings.shortcut);
    Ok(status)
}

#[tauri::command]
pub async fn open_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    passphrase: String,
) -> Result<VaultStatus, VaultError> {
    let state_clone = state.inner().clone();
    let status = run_blocking(move || {
        state_clone.close_vault();
        let db = VaultDb::open(Path::new(&path), &passphrase)?;
        let status = db.status()?;
        {
            let mut p = state_clone.locked_path.lock().map_err(|_| {
                VaultError::InvalidInput("内部状態ロックの取得に失敗しました".to_string())
            })?;
            *p = Some(PathBuf::from(path));
        }
        {
            let mut guard = state_clone.lock_db()?;
            *guard = Some(db);
        }
        Ok(status)
    })
    .await?;

    sync_global_lock_shortcut(&app, &status.lock_settings.shortcut);
    Ok(status)
}

#[tauri::command]
pub async fn unlock_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    passphrase: String,
) -> Result<VaultStatus, VaultError> {
    let state_clone = state.inner().clone();
    let status = run_blocking(move || {
        let path = {
            let guard = state_clone.locked_path.lock().map_err(|_| {
                VaultError::InvalidInput("内部状態ロックの取得に失敗しました".to_string())
            })?;
            guard.clone().ok_or(VaultError::VaultNotOpen)?
        };
        let db = VaultDb::open(&path, &passphrase)?;
        let status = db.status()?;
        {
            let mut guard = state_clone.lock_db()?;
            *guard = Some(db);
        }
        Ok(status)
    })
    .await?;

    sync_global_lock_shortcut(&app, &status.lock_settings.shortcut);
    Ok(status)
}

#[tauri::command]
pub fn lock_vault(app: AppHandle, state: State<'_, AppState>) -> Result<VaultStatus, VaultError> {
    state.lock_vault();
    clear_global_lock_shortcuts(&app);
    state.current_status()
}

#[tauri::command]
pub fn close_vault(app: AppHandle, state: State<'_, AppState>) -> Result<(), VaultError> {
    state.close_vault();
    clear_global_lock_shortcuts(&app);
    Ok(())
}

#[tauri::command]
pub fn get_vault_status(state: State<'_, AppState>) -> Result<VaultStatus, VaultError> {
    state.current_status()
}

#[tauri::command]
pub fn set_lock_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: LockSettings,
) -> Result<LockSettings, VaultError> {
    let saved = with_open_db(&state.db, |db| db.set_lock_settings(&settings))?;
    sync_global_lock_shortcut(&app, &saved.shortcut);
    Ok(saved)
}

#[tauri::command]
pub fn set_clipboard_watch(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<VaultStatus, VaultError> {
    if enabled {
        if !state.is_open() {
            return Err(VaultError::VaultNotOpen);
        }
        let initial_hash = clipboard::read_clipboard_hash_only();
        if let Ok(mut h) = state.clipboard_last_hash.lock() {
            *h = initial_hash;
        }
        state.clipboard_watch_enabled.store(true, Ordering::SeqCst);
    } else {
        state.stop_clipboard_watch();
    }
    state.current_status()
}

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
            for raw_path in paths {
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
            let outcome = db.import_bytes(&raw_bytes, title.as_deref());
            record_import_outcome(&mut report, outcome, "import_bytes");
            if !report.errors.is_empty() {
                return Err(VaultError::UnsupportedFormat(report.errors[0].clone()));
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
pub async fn query_entries(
    state: State<'_, AppState>,
    query: SearchQuery,
) -> Result<Vec<GalleryEntry>, VaultError> {
    let db_arc = Arc::clone(&state.db);
    run_blocking(move || with_open_db(&db_arc, |db| db.query_entries(&query))).await
}

#[tauri::command]
pub fn get_item_detail(
    state: State<'_, AppState>,
    item_id: String,
) -> Result<ItemDetail, VaultError> {
    with_open_db(&state.db, |db| db.get_item_detail(&item_id))
}

#[tauri::command]
pub async fn get_thumbnail(
    state: State<'_, AppState>,
    item_id: String,
) -> Result<Response, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let bytes =
        run_blocking(move || with_open_db(&db_arc, |db| db.get_thumbnail_bytes(&item_id))).await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub async fn get_image(
    state: State<'_, AppState>,
    item_id: String,
) -> Result<Response, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let bytes =
        run_blocking(move || with_open_db(&db_arc, |db| db.get_blob_bytes(&item_id))).await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub fn set_item_title(
    state: State<'_, AppState>,
    item_id: String,
    title: String,
) -> Result<ItemDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.set_title(&item_id, &title))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn set_item_rating(
    state: State<'_, AppState>,
    item_id: String,
    rating: i32,
) -> Result<ItemDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.set_rating(&item_id, rating))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn set_item_memo(
    state: State<'_, AppState>,
    item_id: String,
    memo: String,
) -> Result<ItemDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.set_memo(&item_id, &memo))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn set_item_source_url(
    state: State<'_, AppState>,
    item_id: String,
    source_url: String,
) -> Result<ItemDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.set_source_url(&item_id, &source_url))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn set_url_import_settings(
    state: State<'_, AppState>,
    settings: UrlImportSettings,
) -> Result<UrlImportSettings, VaultError> {
    with_open_db(&state.db, |db| db.set_url_import_settings(&settings))
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

#[tauri::command]
pub fn add_item_tag(
    state: State<'_, AppState>,
    item_id: String,
    tag_name: String,
) -> Result<ItemDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.add_tag(&item_id, &tag_name))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn remove_item_tag(
    state: State<'_, AppState>,
    item_id: String,
    tag_name: String,
) -> Result<ItemDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.remove_tag(&item_id, &tag_name))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn batch_update_entry_tags(
    state: State<'_, AppState>,
    entries: Vec<EntryRef>,
    add_tags: Vec<String>,
    remove_tags: Vec<String>,
) -> Result<BatchTagReport, VaultError> {
    let res = with_open_db(&state.db, |db| {
        db.batch_update_entry_tags(&entries, &add_tags, &remove_tags)
    })?;
    if res.updated_count > 0 {
        state.notify_lan_library_changed();
    }
    Ok(res)
}

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> Result<Vec<TagWithCount>, VaultError> {
    with_open_db(&state.db, |db| db.list_tags())
}

// ---- Virtual Folder Commands ----

#[tauri::command]
pub fn list_folders(state: State<'_, AppState>) -> Result<Vec<FolderNode>, VaultError> {
    with_open_db(&state.db, |db| db.list_folders())
}

#[tauri::command]
pub fn create_folder(
    state: State<'_, AppState>,
    parent_id: Option<String>,
    name: String,
    color: String,
) -> Result<FolderNode, VaultError> {
    let res = with_open_db(&state.db, |db| {
        db.create_folder(parent_id.as_deref(), &name, &color)
    })?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn rename_folder(
    state: State<'_, AppState>,
    folder_id: String,
    name: String,
    color: Option<String>,
) -> Result<FolderNode, VaultError> {
    let res = with_open_db(&state.db, |db| {
        db.rename_folder(&folder_id, &name, color.as_deref())
    })?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn move_folder(
    state: State<'_, AppState>,
    folder_id: String,
    new_parent_id: Option<String>,
) -> Result<FolderNode, VaultError> {
    let res = with_open_db(&state.db, |db| {
        db.move_folder(&folder_id, new_parent_id.as_deref())
    })?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn delete_folder(state: State<'_, AppState>, folder_id: String) -> Result<(), VaultError> {
    with_open_db(&state.db, |db| db.delete_folder(&folder_id))?;
    state.notify_lan_library_changed();
    Ok(())
}

#[tauri::command]
pub fn add_entries_to_folder(
    state: State<'_, AppState>,
    folder_id: String,
    entries: Vec<EntryRef>,
) -> Result<(), VaultError> {
    with_open_db(&state.db, |db| {
        db.add_entries_to_folder(&folder_id, &entries)
    })?;
    state.notify_lan_library_changed();
    Ok(())
}

#[tauri::command]
pub fn remove_entries_from_folder(
    state: State<'_, AppState>,
    folder_id: String,
    entries: Vec<EntryRef>,
) -> Result<(), VaultError> {
    with_open_db(&state.db, |db| {
        db.remove_entries_from_folder(&folder_id, &entries)
    })?;
    state.notify_lan_library_changed();
    Ok(())
}

// ---- Manga Group Commands ----

#[tauri::command]
pub fn create_group_from_items(
    state: State<'_, AppState>,
    item_ids: Vec<String>,
    title: Option<String>,
    target_folder_id: Option<String>,
) -> Result<GroupDetail, VaultError> {
    let res = with_open_db(&state.db, |db| {
        db.create_group_from_items(&item_ids, title.as_deref(), target_folder_id.as_deref())
    })?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn add_items_to_group(
    state: State<'_, AppState>,
    group_id: String,
    item_ids: Vec<String>,
) -> Result<GroupDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.add_items_to_group(&group_id, &item_ids))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn reorder_group_pages(
    state: State<'_, AppState>,
    group_id: String,
    ordered_item_ids: Vec<String>,
) -> Result<GroupDetail, VaultError> {
    let res = with_open_db(&state.db, |db| {
        db.reorder_group_pages(&group_id, &ordered_item_ids)
    })?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn remove_page_from_group(
    state: State<'_, AppState>,
    group_id: String,
    item_id: String,
) -> Result<Option<GroupDetail>, VaultError> {
    let res = with_open_db(&state.db, |db| {
        db.remove_page_from_group(&group_id, &item_id)
    })?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn ungroup(state: State<'_, AppState>, group_id: String) -> Result<Vec<String>, VaultError> {
    let res = with_open_db(&state.db, |db| db.ungroup(&group_id))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn get_group_detail(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<GroupDetail, VaultError> {
    with_open_db(&state.db, |db| db.get_group_detail(&group_id))
}

#[tauri::command]
pub fn set_group_title(
    state: State<'_, AppState>,
    group_id: String,
    title: String,
) -> Result<GroupDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.set_group_title(&group_id, &title))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn set_group_rating(
    state: State<'_, AppState>,
    group_id: String,
    rating: i32,
) -> Result<GroupDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.set_group_rating(&group_id, rating))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn set_group_memo(
    state: State<'_, AppState>,
    group_id: String,
    memo: String,
) -> Result<GroupDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.set_group_memo(&group_id, &memo))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn set_group_reader_prefs(
    state: State<'_, AppState>,
    group_id: String,
    reading_dir: String,
    spread_mode: String,
    cover_alone: bool,
) -> Result<GroupDetail, VaultError> {
    let res = with_open_db(&state.db, |db| {
        db.set_group_reader_prefs(&group_id, &reading_dir, &spread_mode, cover_alone)
    })?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn add_group_tag(
    state: State<'_, AppState>,
    group_id: String,
    tag_name: String,
) -> Result<GroupDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.add_group_tag(&group_id, &tag_name))?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub fn remove_group_tag(
    state: State<'_, AppState>,
    group_id: String,
    tag_name: String,
) -> Result<GroupDetail, VaultError> {
    let res = with_open_db(&state.db, |db| db.remove_group_tag(&group_id, &tag_name))?;
    state.notify_lan_library_changed();
    Ok(res)
}

// ---- Trash & Permanent Deletion Commands ----

#[tauri::command]
pub fn move_entries_to_trash(
    state: State<'_, AppState>,
    entries: Vec<EntryRef>,
) -> Result<usize, VaultError> {
    let count = with_open_db(&state.db, |db| db.move_entries_to_trash(&entries))?;
    state.notify_lan_library_changed();
    Ok(count)
}

#[tauri::command]
pub fn restore_entries_from_trash(
    state: State<'_, AppState>,
    entries: Vec<EntryRef>,
) -> Result<usize, VaultError> {
    let count = with_open_db(&state.db, |db| db.restore_entries_from_trash(&entries))?;
    state.notify_lan_library_changed();
    Ok(count)
}

#[tauri::command]
pub fn permanently_delete_entries(
    state: State<'_, AppState>,
    entries: Vec<EntryRef>,
) -> Result<usize, VaultError> {
    let count = with_open_db(&state.db, |db| db.permanently_delete_entries(&entries))?;
    let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
    state.invalidate_media_items(&ids);
    state.notify_lan_library_changed();
    Ok(count)
}

#[tauri::command]
pub fn empty_trash(state: State<'_, AppState>) -> Result<usize, VaultError> {
    let count = with_open_db(&state.db, |db| db.empty_trash())?;
    state.clear_media_cache();
    state.notify_lan_library_changed();
    Ok(count)
}

// ---- Phase 3: Maintenance, Optimization, Backup & Exchange Packages ----

#[tauri::command]
pub async fn get_vault_health(state: State<'_, AppState>) -> Result<VaultHealthInfo, VaultError> {
    let db_arc = Arc::clone(&state.db);
    run_blocking(move || with_open_db(&db_arc, |db| db.health_info())).await
}

#[tauri::command]
pub async fn optimize_vault(state: State<'_, AppState>) -> Result<OptimizeReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    run_blocking(move || with_open_db(&db_arc, |db| db.optimize())).await
}

#[tauri::command]
pub async fn backup_vault(
    state: State<'_, AppState>,
    dest_path: String,
    overwrite: bool,
) -> Result<BackupReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    run_blocking(move || with_open_db(&db_arc, |db| db.backup_to(Path::new(&dest_path), overwrite)))
        .await
}

#[tauri::command]
pub async fn export_package(
    state: State<'_, AppState>,
    request: PackageExportRequest,
) -> Result<PackageExportReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    run_blocking(move || with_open_db(&db_arc, |db| package::export_partial_package(db, &request)))
        .await
}

#[tauri::command]
pub async fn import_package(
    state: State<'_, AppState>,
    request: PackageImportRequest,
) -> Result<PackageImportReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let report = run_blocking(move || {
        with_open_db(&db_arc, |db| package::import_partial_package(db, &request))
    })
    .await?;
    if report.imported_items > 0 || report.imported_groups > 0 || report.imported_folders > 0 {
        state.notify_lan_library_changed();
    }
    Ok(report)
}

#[tauri::command]
pub fn set_skip_plain_export_confirm(
    state: State<'_, AppState>,
    skip: bool,
) -> Result<bool, VaultError> {
    with_open_db(&state.db, |db| db.set_skip_plain_export_confirm(skip))
}

#[tauri::command]
pub async fn export_plain_item(
    state: State<'_, AppState>,
    request: PlainExportItemRequest,
) -> Result<PlainExportReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    run_blocking(move || with_open_db(&db_arc, |db| plain_export::export_plain_item(db, &request)))
        .await
}

#[tauri::command]
pub async fn export_plain_batch(
    state: State<'_, AppState>,
    request: PlainExportBatchRequest,
) -> Result<PlainExportReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    run_blocking(move || with_open_db(&db_arc, |db| plain_export::export_plain_batch(db, &request)))
        .await
}

// ---- LAN Temporary Browser Sharing Commands (Host Only) ----

#[tauri::command]
pub fn get_lan_network_interfaces() -> Vec<LanInterfaceInfo> {
    list_lan_interfaces()
}

#[tauri::command]
pub fn get_lan_share_status(state: State<'_, AppState>) -> LanShareStatus {
    state.current_lan_status()
}

#[tauri::command]
pub async fn start_lan_share(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartLanShareRequest,
) -> Result<LanShareStatus, VaultError> {
    let state_clone = state.inner().clone();
    let assets = AssetSource::Tauri(app.clone());
    let notifier = LanHostNotifier::from_app(app);
    start_lan_server(state_clone, request, assets, notifier).await
}

#[tauri::command]
pub fn stop_lan_share(state: State<'_, AppState>) -> LanShareStatus {
    state.stop_lan_share_with_reason("stopped");
    state.current_lan_status()
}

#[tauri::command]
pub fn update_lan_share_mode(
    state: State<'_, AppState>,
    mode: LanAccessMode,
) -> Result<LanShareStatus, VaultError> {
    let handle = state
        .lan_server
        .lock()
        .map_err(|_| VaultError::LanShare("内部状態ロックの取得に失敗しました".to_string()))?
        .clone();
    let Some(h) = handle else {
        return Err(VaultError::LanShare(
            "現在 LAN 公開は起動していません".to_string(),
        ));
    };
    Ok(h.set_mode(mode))
}

#[tauri::command]
pub fn extend_lan_share_timer(
    state: State<'_, AppState>,
    extra_minutes: u32,
) -> Result<LanShareStatus, VaultError> {
    let handle = state
        .lan_server
        .lock()
        .map_err(|_| VaultError::LanShare("内部状態ロックの取得に失敗しました".to_string()))?
        .clone();
    let Some(h) = handle else {
        return Err(VaultError::LanShare(
            "現在 LAN 公開は起動していません".to_string(),
        ));
    };
    Ok(h.extend_timer(extra_minutes))
}

#[tauri::command]
pub fn regenerate_lan_credentials(
    state: State<'_, AppState>,
    disconnect_existing: bool,
) -> Result<LanShareStatus, VaultError> {
    let handle = state
        .lan_server
        .lock()
        .map_err(|_| VaultError::LanShare("内部状態ロックの取得に失敗しました".to_string()))?
        .clone();
    let Some(h) = handle else {
        return Err(VaultError::LanShare(
            "現在 LAN 公開は起動していません".to_string(),
        ));
    };
    h.regenerate_credentials(disconnect_existing)
}

#[tauri::command]
pub fn disconnect_lan_clients(state: State<'_, AppState>) -> Result<LanShareStatus, VaultError> {
    let handle = state
        .lan_server
        .lock()
        .map_err(|_| VaultError::LanShare("内部状態ロックの取得に失敗しました".to_string()))?
        .clone();
    let Some(h) = handle else {
        return Err(VaultError::LanShare(
            "現在 LAN 公開は起動していません".to_string(),
        ));
    };
    Ok(h.disconnect_all_clients())
}
