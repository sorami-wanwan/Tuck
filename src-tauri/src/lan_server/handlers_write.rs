use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::header::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::StreamExt;
use serde::Deserialize;

use crate::commands::percent_decode;
use crate::db::ImportOutcome;
use crate::error::VaultError;
use crate::imaging::is_video_mime;
use crate::models::{EntryKind, EntryRef, ImportReport};
use crate::state::AppState;
use crate::ytdlp;

use super::auth::{error_response, notify_changed, require_write_checked, run_db};
use super::{
    EphemeralUploadFile, LanApiError, LanServerRuntime, LanSseEvent, DEFAULT_UPLOAD_LIMIT_MB,
    MAX_THUMBNAIL_UPLOAD_BYTES, STREAM_RAM_BUFFER_THRESHOLD,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TitleBody {
    title: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RatingBody {
    rating: i32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoBody {
    memo: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SourceUrlBody {
    source_url: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TagBody {
    tag_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BatchEntryTagsBody {
    entries: Vec<EntryRef>,
    #[serde(default)]
    add_tags: Vec<String>,
    #[serde(default)]
    remove_tags: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BatchEntryRatingBody {
    entries: Vec<EntryRef>,
    rating: i32,
}

pub(super) async fn handle_set_item_title(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<TitleBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.set_title(&id, &body.title)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_set_item_rating(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<RatingBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.set_rating(&id, body.rating)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_set_item_memo(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<MemoBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.set_memo(&id, &body.memo)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_set_item_source_url(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<SourceUrlBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.set_source_url(&id, &body.source_url)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_add_item_tag(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<TagBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.add_tag(&id, &body.tag_name)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_remove_item_tag(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath((id, tag)): AxumPath<(String, String)>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let decoded_tag = percent_decode(&tag);
    let res = run_db(&runtime, move |db| db.remove_tag(&id, &decoded_tag)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_batch_update_entry_tags(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(body): Json<BatchEntryTagsBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| {
        db.batch_update_entry_tags(&body.entries, &body.add_tags, &body.remove_tags)
    })
    .await?;
    if res.updated_count > 0 {
        notify_changed(&runtime);
    }
    Ok(Json(res).into_response())
}

pub(super) async fn handle_batch_set_entry_rating(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(body): Json<BatchEntryRatingBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| {
        db.batch_set_entry_rating(&body.entries, body.rating)
    })
    .await?;
    if res.updated_count > 0 {
        notify_changed(&runtime);
    }
    Ok(Json(res).into_response())
}

pub(super) async fn handle_save_item_thumbnail(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    body: Body,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
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

    let bytes = axum::body::to_bytes(body, MAX_THUMBNAIL_UPLOAD_BYTES)
        .await
        .map_err(|_| {
            error_response(VaultError::InvalidInput(
                "サムネイル画像が大きすぎます".to_string(),
            ))
        })?;
    let raw_png = bytes.to_vec();
    let res = run_db(&runtime, move |db| {
        db.save_media_thumbnail(&id, &raw_png, width, height, duration_ms)
    })
    .await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateFolderBody {
    parent_id: Option<String>,
    name: String,
    #[serde(default)]
    color: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RenameFolderBody {
    name: String,
    color: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MoveFolderBody {
    new_parent_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EntriesBody {
    entries: Vec<EntryRef>,
}

pub(super) async fn handle_create_folder(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(body): Json<CreateFolderBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| {
        db.create_folder(body.parent_id.as_deref(), &body.name, &body.color)
    })
    .await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_rename_folder(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<RenameFolderBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| {
        db.rename_folder(&id, &body.name, body.color.as_deref())
    })
    .await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_move_folder(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<MoveFolderBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| {
        db.move_folder(&id, body.new_parent_id.as_deref())
    })
    .await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_delete_folder(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    run_db(&runtime, move |db| db.delete_folder(&id)).await?;
    notify_changed(&runtime);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn handle_add_entries_to_folder(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<EntriesBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    run_db(&runtime, move |db| {
        db.add_entries_to_folder(&id, &body.entries)
    })
    .await?;
    notify_changed(&runtime);
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn handle_remove_entries_from_folder(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<EntriesBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    run_db(&runtime, move |db| {
        db.remove_entries_from_folder(&id, &body.entries)
    })
    .await?;
    notify_changed(&runtime);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateGroupBody {
    item_ids: Vec<String>,
    title: Option<String>,
    target_folder_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemIdsBody {
    item_ids: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReorderPagesBody {
    ordered_item_ids: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReaderPrefsBody {
    reading_dir: String,
    spread_mode: String,
    cover_alone: bool,
}

pub(super) async fn handle_create_group(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(body): Json<CreateGroupBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| {
        db.create_group_from_items(
            &body.item_ids,
            body.title.as_deref(),
            body.target_folder_id.as_deref(),
        )
    })
    .await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_add_items_to_group(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<ItemIdsBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| {
        db.add_items_to_group(&id, &body.item_ids)
    })
    .await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_reorder_group_pages(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<ReorderPagesBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| {
        db.reorder_group_pages(&id, &body.ordered_item_ids)
    })
    .await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_remove_page_from_group(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath((id, item_id)): AxumPath<(String, String)>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.remove_page_from_group(&id, &item_id)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_ungroup(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.ungroup(&id)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_set_group_title(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<TitleBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.set_group_title(&id, &body.title)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_set_group_rating(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<RatingBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.set_group_rating(&id, body.rating)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_set_group_memo(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<MemoBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.set_group_memo(&id, &body.memo)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_set_group_reader_prefs(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<ReaderPrefsBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| {
        db.set_group_reader_prefs(&id, &body.reading_dir, &body.spread_mode, body.cover_alone)
    })
    .await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_add_group_tag(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<TagBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let res = run_db(&runtime, move |db| db.add_group_tag(&id, &body.tag_name)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_remove_group_tag(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath((id, tag)): AxumPath<(String, String)>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let decoded_tag = percent_decode(&tag);
    let res = run_db(&runtime, move |db| db.remove_group_tag(&id, &decoded_tag)).await?;
    notify_changed(&runtime);
    Ok(Json(res).into_response())
}

pub(super) async fn handle_trash_move(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(body): Json<EntriesBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let count = run_db(&runtime, move |db| db.move_entries_to_trash(&body.entries)).await?;
    notify_changed(&runtime);
    Ok(Json(count).into_response())
}

pub(super) async fn handle_trash_restore(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(body): Json<EntriesBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let count = run_db(&runtime, move |db| {
        db.restore_entries_from_trash(&body.entries)
    })
    .await?;
    notify_changed(&runtime);
    Ok(Json(count).into_response())
}

pub(super) async fn handle_trash_delete(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(body): Json<EntriesBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let ids: Vec<String> = body.entries.iter().map(|e| e.id.clone()).collect();
    let count = run_db(&runtime, move |db| {
        db.permanently_delete_entries(&body.entries)
    })
    .await?;
    runtime.app_state.invalidate_media_items(&ids);
    notify_changed(&runtime);
    Ok(Json(count).into_response())
}

pub(super) async fn handle_trash_empty(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let count = run_db(&runtime, |db| db.empty_trash()).await?;
    runtime.app_state.clear_media_cache();
    notify_changed(&runtime);
    Ok(Json(count).into_response())
}

pub(super) async fn handle_import_bytes(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;

    let title = headers
        .get("x-title")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .filter(|s| !s.trim().is_empty());
    let folder_id = headers
        .get("x-folder-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let max_mb = run_db(&runtime, |db| {
        Ok(db.get_url_import_settings().max_video_size_mb)
    })
    .await?;
    let limit_mb = if max_mb == 0 {
        DEFAULT_UPLOAD_LIMIT_MB
    } else {
        max_mb
    };
    let max_bytes: usize = usize::try_from(u64::from(limit_mb) * 1024 * 1024).unwrap_or(usize::MAX);

    let mut stream = body.into_data_stream();
    let mut ram_buf: Vec<u8> = Vec::new();
    let mut temp_file: Option<EphemeralUploadFile> = None;
    let mut total_bytes: usize = 0;

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| {
            error_response(VaultError::InvalidInput(format!(
                "アップロード受信中にエラーが発生しました: {e}"
            )))
        })?;
        total_bytes = total_bytes.checked_add(chunk.len()).ok_or_else(|| {
            error_response(VaultError::InvalidInput(
                "ファイルサイズ上限を超えています".to_string(),
            ))
        })?;
        if total_bytes > max_bytes {
            return Err(VaultError::InvalidInput(format!(
                "アップロードサイズが上限 ({limit_mb} MB) を超えています"
            ))
            .into());
        }

        if let Some(tf) = temp_file.as_mut() {
            tf.write_all(&chunk)
                .map_err(|e| error_response(VaultError::Io(e)))?;
        } else if ram_buf.len() + chunk.len() > STREAM_RAM_BUFFER_THRESHOLD {
            let mut tf =
                EphemeralUploadFile::create().map_err(|e| error_response(VaultError::Io(e)))?;
            if !ram_buf.is_empty() {
                tf.write_all(&ram_buf)
                    .map_err(|e| error_response(VaultError::Io(e)))?;
                ram_buf.clear();
                ram_buf.shrink_to_fit();
            }
            tf.write_all(&chunk)
                .map_err(|e| error_response(VaultError::Io(e)))?;
            temp_file = Some(tf);
        } else {
            ram_buf.extend_from_slice(&chunk);
        }
    }

    if let Some(tf) = temp_file.as_mut() {
        tf.finish_writing()
            .map_err(|e| error_response(VaultError::Io(e)))?;
    }

    let report = run_db(&runtime, move |db| {
        let _temp_guard = temp_file;
        let outcome = if let Some(ref tf) = _temp_guard {
            db.import_media_file(tf.path(), title.as_deref(), None, 0)
        } else {
            db.import_media_bytes(&ram_buf, title.as_deref(), None, 0)
        };

        let mut report = ImportReport::default();
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
                if let Some(fid) = folder_id.as_deref() {
                    db.add_entries_to_folder(
                        fid,
                        &[EntryRef {
                            kind: EntryKind::Image,
                            id: id.clone(),
                        }],
                    )?;
                }
                report.imported_ids.push(id);
            }
            Ok(ImportOutcome::Duplicate) => {
                report.duplicate_count += 1;
            }
            Err(e) => return Err(e),
        }
        Ok(report)
    })
    .await?;

    if !report.imported_ids.is_empty() {
        notify_changed(&runtime);
    }
    Ok(Json(report).into_response())
}

pub(super) async fn handle_check_ytdlp_status(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let url_settings = run_db(&runtime, |db| Ok(db.get_url_import_settings())).await?;
    let mut st = tokio::task::spawn_blocking(move || {
        ytdlp::check_ytdlp_status(&url_settings.ytdlp_path, &url_settings.ffmpeg_path)
    })
    .await
    .map_err(|e| error_response(VaultError::UrlImport(e.to_string())))?;
    st.resolved_path = None;
    st.ffmpeg_path = None;
    Ok(Json(st).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UrlImportBody {
    url: String,
    folder_id: Option<String>,
}

struct LanUrlImportSessionGuard {
    state: AppState,
    token: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for LanUrlImportSessionGuard {
    fn drop(&mut self) {
        self.state.finish_url_import(&self.token);
    }
}

pub(super) async fn handle_import_url(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(body): Json<UrlImportBody>,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let url_settings = run_db(&runtime, |db| Ok(db.get_url_import_settings())).await?;
    let cancel_token = runtime
        .app_state
        .begin_url_import()
        .map_err(error_response)?;
    let guard = LanUrlImportSessionGuard {
        state: runtime.app_state.clone(),
        token: Arc::clone(&cancel_token),
    };

    let rt_clone = Arc::clone(&runtime);
    let url_str = body.url;
    let folder_id = body.folder_id;

    let report = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let batch = ytdlp::download_from_url(&url_str, &url_settings, &cancel_token, |ev| {
            rt_clone.notifier.emit_url_import_progress(&ev);
            let _ = rt_clone
                .events_tx
                .send(LanSseEvent::UrlImportProgress { payload: ev });
        })?;

        let mut db_guard = rt_clone.app_state.lock_db()?;
        let db = db_guard.as_mut().ok_or(VaultError::VaultNotOpen)?;
        ytdlp::import_downloaded_url_batch(db, &batch, folder_id.as_deref(), |ev| {
            rt_clone.notifier.emit_url_import_progress(&ev);
            let _ = rt_clone
                .events_tx
                .send(LanSseEvent::UrlImportProgress { payload: ev });
        })
    })
    .await
    .map_err(|e| error_response(VaultError::UrlImport(e.to_string())))?
    .map_err(error_response)?;

    if !report.imported_ids.is_empty() {
        notify_changed(&runtime);
    }
    Ok(Json(report).into_response())
}

pub(super) async fn handle_cancel_url_import(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
) -> Result<Response, LanApiError> {
    require_write_checked(&runtime, &headers)?;
    let cancelled = runtime.app_state.cancel_url_import();
    Ok(Json(cancelled).into_response())
}
