use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::header::{self, HeaderMap};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::stream::unfold;
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};

use crate::error::VaultError;
use crate::media_stream::resolve_media_slice;
use crate::models::{
    EntryKind, EntryRef, SearchQuery, VaultStatus, WebBatchExportRequest,
    WebBatchExportTicketResponse,
};
use crate::plain_export::{
    build_item_filename, build_plain_export_plan, compute_zip_archive_size,
    content_disposition_attachment, stream_single_item_to_channel, stream_zip_plan_to_channel,
};

use super::auth::{
    csrf_forbidden_response, error_response, generate_random_hex, require_media_auth,
    require_read_checked, require_read_session_checked, run_db, verify_csrf,
};
use super::{
    ExportTicket, LanApiError, LanServerRuntime, LanSseEvent, EXPORT_TICKET_TTL_SECS,
    MAX_EXPORT_TICKETS,
};

pub(super) async fn handle_sse_events(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>>, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    let rx = runtime.events_tx.subscribe();
    let stream = futures_util::stream::unfold((rx, false), |(mut rx, finished)| async move {
        if finished {
            return None;
        }
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let is_terminal = matches!(
                        ev,
                        LanSseEvent::VaultLocked | LanSseEvent::LanStopped { .. }
                    );
                    let (name, data) = match &ev {
                        LanSseEvent::VaultLocked => ("vault_locked", "{}".to_string()),
                        LanSseEvent::LanStopped { .. } => (
                            "lan_stopped",
                            serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string()),
                        ),
                        LanSseEvent::LanModeChanged { .. } => (
                            "lan_mode_changed",
                            serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string()),
                        ),
                        LanSseEvent::LibraryChanged => ("library_changed", "{}".to_string()),
                        LanSseEvent::UrlImportProgress { payload } => (
                            "url_import_progress",
                            serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string()),
                        ),
                    };
                    let item = Ok(SseEvent::default().event(name).data(data));
                    return Some((item, (rx, is_terminal)));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))))
}

fn sanitize_vault_status(mut status: VaultStatus) -> VaultStatus {
    status.clipboard_watch_enabled = false;
    status.skip_plain_export_confirm = false;
    status.url_import_settings.ytdlp_path.clear();
    status.url_import_settings.ffmpeg_path.clear();
    status.url_import_settings.cookie_browser.clear();
    status.url_import_settings.cookies_file.clear();
    status
}

pub(super) async fn handle_vault_status(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
) -> Result<Json<VaultStatus>, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    let status = runtime.app_state.current_status().map_err(error_response)?;
    Ok(Json(sanitize_vault_status(status)))
}

pub(super) async fn handle_query_entries(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(query): Json<SearchQuery>,
) -> Result<Response, LanApiError> {
    if let Err(e) = verify_csrf(&headers) {
        return Err(csrf_forbidden_response(e).into());
    }
    require_read_checked(&runtime, &headers)?;
    let entries = run_db(&runtime, move |db| db.query_entries(&query)).await?;
    Ok(Json(entries).into_response())
}

pub(super) async fn handle_get_item_detail(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    let detail = run_db(&runtime, move |db| db.get_item_detail(&id)).await?;
    Ok(Json(detail).into_response())
}

pub(super) async fn handle_get_group_detail(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    let detail = run_db(&runtime, move |db| db.get_group_detail(&id)).await?;
    Ok(Json(detail).into_response())
}

pub(super) async fn handle_list_folders(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    let folders = run_db(&runtime, |db| db.list_folders()).await?;
    Ok(Json(folders).into_response())
}

pub(super) async fn handle_list_tags(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    let tags = run_db(&runtime, |db| db.list_tags()).await?;
    Ok(Json(tags).into_response())
}

pub(super) async fn handle_get_thumbnail(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    let bytes = run_db(&runtime, move |db| db.get_thumbnail_bytes(&id)).await?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/webp")
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|e| LanApiError::from(VaultError::LanShare(e.to_string())))
}

pub(super) async fn handle_get_image(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    let (mime_type, bytes) = run_db(&runtime, move |db| db.get_media_blob_with_mime(&id)).await?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|e| LanApiError::from(VaultError::LanShare(e.to_string())))
}

#[derive(Deserialize)]
pub(super) struct MediaQuery {
    #[serde(rename = "mt")]
    mt: Option<String>,
}

async fn serve_media_inner(
    runtime: Arc<LanServerRuntime>,
    headers: HeaderMap,
    id: String,
    mt: Option<String>,
    is_head: bool,
) -> Result<Response, LanApiError> {
    require_media_auth(&runtime, &headers, mt.as_deref()).map_err(error_response)?;
    if !runtime.app_state.is_open() {
        return Err(VaultError::VaultNotOpen.into());
    }

    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let state_clone = runtime.app_state.clone();

    let slice = tokio::task::spawn_blocking(move || {
        resolve_media_slice(&state_clone, &id, is_head, range_header.as_deref())
    })
    .await
    .map_err(|e| error_response(VaultError::LanShare(e.to_string())))?
    .map_err(error_response)?;

    let status = StatusCode::from_u16(slice.status.as_u16()).unwrap_or(StatusCode::OK);
    let mut builder = Response::builder()
        .status(status)
        .header(header::ACCEPT_RANGES, "bytes");

    if status != StatusCode::RANGE_NOT_SATISFIABLE {
        builder = builder
            .header(header::CONTENT_TYPE, slice.mime_type)
            .header(header::CONTENT_LENGTH, slice.content_length.to_string());
    }
    if let Some(cr) = slice.content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }

    builder
        .body(Body::from(slice.body))
        .map_err(|e| LanApiError::from(VaultError::LanShare(e.to_string())))
}

pub(super) async fn handle_get_media(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(q): Query<MediaQuery>,
) -> Result<Response, LanApiError> {
    serve_media_inner(runtime, headers, id, q.mt, false).await
}

pub(super) async fn handle_head_media(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(q): Query<MediaQuery>,
) -> Result<Response, LanApiError> {
    serve_media_inner(runtime, headers, id, q.mt, true).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FolderExportQuery {
    include_subfolders: Option<bool>,
}

async fn export_item_inner(
    runtime: Arc<LanServerRuntime>,
    headers: HeaderMap,
    id: String,
    is_head: bool,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    let item_id_for_err = id.clone();
    let meta = run_db(&runtime, move |db| {
        let m = db.get_export_item_meta(&id)?;
        if m.deleted_at.is_some() {
            return Err(VaultError::NotFound(item_id_for_err));
        }
        Ok(m)
    })
    .await?;

    let filename = build_item_filename(&meta.title, &meta.id, &meta.mime_type);
    let cd = content_disposition_attachment(&filename);
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, meta.mime_type.clone())
        .header(header::CONTENT_LENGTH, meta.byte_size.to_string())
        .header(header::CONTENT_DISPOSITION, cd);

    if is_head {
        return builder
            .body(Body::empty())
            .map_err(|e| LanApiError::from(VaultError::LanShare(e.to_string())));
    }

    let (tx, rx) = mpsc::channel(4);
    let db_arc = Arc::clone(&runtime.app_state.db);
    tokio::task::spawn_blocking(move || stream_single_item_to_channel(db_arc, meta, tx));

    let stream = unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    builder
        .body(Body::from_stream(stream))
        .map_err(|e| LanApiError::from(VaultError::LanShare(e.to_string())))
}

async fn export_zip_inner(
    runtime: Arc<LanServerRuntime>,
    entries: Vec<EntryRef>,
    folder_ids: Vec<String>,
    include_subfolders: bool,
    is_head: bool,
) -> Result<Response, LanApiError> {
    let plan = run_db(&runtime, move |db| {
        if entries.len() == 1 && folder_ids.is_empty() && entries[0].kind == EntryKind::Group {
            let gid = &entries[0].id;
            let exists: Option<String> =
                rusqlite::OptionalExtension::optional(db.conn().query_row(
                    "SELECT id FROM groups WHERE id = ?1 AND deleted_at IS NULL",
                    rusqlite::params![gid],
                    |r| r.get(0),
                ))?;
            if exists.is_none() {
                return Err(VaultError::NotFound(gid.clone()));
            }
        }
        build_plain_export_plan(db, &entries, &folder_ids, include_subfolders)
    })
    .await?;

    let zip_size = compute_zip_archive_size(&plan);
    let cd = content_disposition_attachment(&plan.suggested_archive_name);
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_LENGTH, zip_size.to_string())
        .header(header::CONTENT_DISPOSITION, cd);

    if is_head {
        return builder
            .body(Body::empty())
            .map_err(|e| LanApiError::from(VaultError::LanShare(e.to_string())));
    }

    let (tx, rx) = mpsc::channel(4);
    let db_arc = Arc::clone(&runtime.app_state.db);
    tokio::task::spawn_blocking(move || stream_zip_plan_to_channel(db_arc, plan, tx));

    let stream = unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    builder
        .body(Body::from_stream(stream))
        .map_err(|e| LanApiError::from(VaultError::LanShare(e.to_string())))
}

pub(super) async fn handle_head_export_item(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    export_item_inner(runtime, headers, id, true).await
}

pub(super) async fn handle_export_item(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    export_item_inner(runtime, headers, id, false).await
}

pub(super) async fn handle_head_export_group(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    export_zip_inner(
        runtime,
        vec![EntryRef {
            kind: EntryKind::Group,
            id,
        }],
        vec![],
        true,
        true,
    )
    .await
}

pub(super) async fn handle_export_group(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    export_zip_inner(
        runtime,
        vec![EntryRef {
            kind: EntryKind::Group,
            id,
        }],
        vec![],
        true,
        false,
    )
    .await
}

pub(super) async fn handle_head_export_folder(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(q): Query<FolderExportQuery>,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    export_zip_inner(
        runtime,
        vec![],
        vec![id],
        q.include_subfolders.unwrap_or(true),
        true,
    )
    .await
}

pub(super) async fn handle_export_folder(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(q): Query<FolderExportQuery>,
) -> Result<Response, LanApiError> {
    require_read_checked(&runtime, &headers)?;
    export_zip_inner(
        runtime,
        vec![],
        vec![id],
        q.include_subfolders.unwrap_or(true),
        false,
    )
    .await
}

pub(super) async fn handle_create_export_batch_ticket(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    Json(req): Json<WebBatchExportRequest>,
) -> Result<Json<WebBatchExportTicketResponse>, LanApiError> {
    if let Err(e) = verify_csrf(&headers) {
        return Err(csrf_forbidden_response(e).into());
    }
    let session_id = require_read_session_checked(&runtime, &headers)?;
    if req.entries.is_empty() && req.folder_ids.is_empty() {
        return Err(VaultError::InvalidInput("書き出す対象を選択してください".to_string()).into());
    }

    let entries_clone = req.entries.clone();
    let folders_clone = req.folder_ids.clone();
    let include_sub = req.include_subfolders;
    let plan = run_db(&runtime, move |db| {
        build_plain_export_plan(db, &entries_clone, &folders_clone, include_sub)
    })
    .await?;

    let ticket = generate_random_hex(24)?;
    let response = WebBatchExportTicketResponse {
        ticket: ticket.clone(),
        file_count: plan.files.len(),
        total_bytes: plan.total_bytes,
        filename: plan.suggested_archive_name,
    };

    {
        let mut guard = runtime.store_write();
        guard
            .export_tickets
            .retain(|_, t| t.created_at.elapsed().as_secs() <= EXPORT_TICKET_TTL_SECS);
        if guard.export_tickets.len() >= MAX_EXPORT_TICKETS {
            if let Some(oldest_key) = guard
                .export_tickets
                .iter()
                .min_by_key(|(_, t)| t.created_at)
                .map(|(k, _)| k.clone())
            {
                guard.export_tickets.remove(&oldest_key);
            }
        }
        guard.export_tickets.insert(
            ticket,
            ExportTicket {
                session_id,
                entries: req.entries,
                folder_ids: req.folder_ids,
                include_subfolders: req.include_subfolders,
                created_at: Instant::now(),
            },
        );
    }

    Ok(Json(response))
}

pub(super) async fn handle_export_batch_by_ticket(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
    AxumPath(ticket): AxumPath<String>,
) -> Result<Response, LanApiError> {
    let session_id = require_read_session_checked(&runtime, &headers)?;
    let ticket_data = {
        let mut guard = runtime.store_write();
        guard
            .export_tickets
            .retain(|_, t| t.created_at.elapsed().as_secs() <= EXPORT_TICKET_TTL_SECS);
        match guard.export_tickets.get(&ticket) {
            Some(t) if t.session_id == session_id => guard.export_tickets.remove(&ticket),
            _ => None,
        }
    };

    let Some(t) = ticket_data else {
        return Err(VaultError::NotFound(
            "ダウンロード用チケットが見つからないか、有効期限が切れています".to_string(),
        )
        .into());
    };

    export_zip_inner(
        runtime,
        t.entries,
        t.folder_ids,
        t.include_subfolders,
        false,
    )
    .await
}
