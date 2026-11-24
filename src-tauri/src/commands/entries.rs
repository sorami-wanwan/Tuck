use std::sync::Arc;

use tauri::ipc::Response;
use tauri::State;

use crate::error::VaultError;
use crate::models::{
    BatchRatingReport, BatchTagReport, EntryRef, GalleryEntry, ItemDetail, SearchQuery,
    TagWithCount,
};
use crate::state::AppState;

use super::{run_blocking, with_open_db};

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
pub async fn batch_update_entry_tags(
    state: State<'_, AppState>,
    entries: Vec<EntryRef>,
    add_tags: Vec<String>,
    remove_tags: Vec<String>,
) -> Result<BatchTagReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let res = run_blocking(move || {
        with_open_db(&db_arc, |db| {
            db.batch_update_entry_tags(&entries, &add_tags, &remove_tags)
        })
    })
    .await?;
    if res.updated_count > 0 {
        state.notify_lan_library_changed();
    }
    Ok(res)
}

#[tauri::command]
pub async fn batch_set_entry_rating(
    state: State<'_, AppState>,
    entries: Vec<EntryRef>,
    rating: i32,
) -> Result<BatchRatingReport, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let res = run_blocking(move || {
        with_open_db(&db_arc, |db| db.batch_set_entry_rating(&entries, rating))
    })
    .await?;
    if res.updated_count > 0 {
        state.notify_lan_library_changed();
    }
    Ok(res)
}

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> Result<Vec<TagWithCount>, VaultError> {
    with_open_db(&state.db, |db| db.list_tags())
}

// ---- Trash & Permanent Deletion Commands ----

#[tauri::command]
pub async fn move_entries_to_trash(
    state: State<'_, AppState>,
    entries: Vec<EntryRef>,
) -> Result<usize, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let count =
        run_blocking(move || with_open_db(&db_arc, |db| db.move_entries_to_trash(&entries)))
            .await?;
    state.notify_lan_library_changed();
    Ok(count)
}

#[tauri::command]
pub async fn restore_entries_from_trash(
    state: State<'_, AppState>,
    entries: Vec<EntryRef>,
) -> Result<usize, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let count =
        run_blocking(move || with_open_db(&db_arc, |db| db.restore_entries_from_trash(&entries)))
            .await?;
    state.notify_lan_library_changed();
    Ok(count)
}

#[tauri::command]
pub async fn permanently_delete_entries(
    state: State<'_, AppState>,
    entries: Vec<EntryRef>,
) -> Result<usize, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let (count, deleted_item_ids) = run_blocking(move || {
        with_open_db(&db_arc, |db| db.permanently_delete_entries_detailed(&entries))
    })
    .await?;
    state.invalidate_media_items(&deleted_item_ids);
    state.notify_lan_library_changed();
    Ok(count)
}

#[tauri::command]
pub async fn empty_trash(state: State<'_, AppState>) -> Result<usize, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let count = run_blocking(move || with_open_db(&db_arc, |db| db.empty_trash())).await?;
    state.clear_media_cache();
    state.notify_lan_library_changed();
    Ok(count)
}
