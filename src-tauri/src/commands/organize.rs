use std::sync::Arc;

use tauri::State;

use crate::error::VaultError;
use crate::models::{EntryRef, FolderNode, GroupDetail};
use crate::state::AppState;

use super::{run_blocking, with_open_db};

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
pub async fn add_entries_to_folder(
    state: State<'_, AppState>,
    folder_id: String,
    entries: Vec<EntryRef>,
) -> Result<(), VaultError> {
    let db_arc = Arc::clone(&state.db);
    run_blocking(move || with_open_db(&db_arc, |db| db.add_entries_to_folder(&folder_id, &entries)))
        .await?;
    state.notify_lan_library_changed();
    Ok(())
}

#[tauri::command]
pub async fn remove_entries_from_folder(
    state: State<'_, AppState>,
    folder_id: String,
    entries: Vec<EntryRef>,
) -> Result<(), VaultError> {
    let db_arc = Arc::clone(&state.db);
    run_blocking(move || {
        with_open_db(&db_arc, |db| {
            db.remove_entries_from_folder(&folder_id, &entries)
        })
    })
    .await?;
    state.notify_lan_library_changed();
    Ok(())
}

// ---- Manga Group Commands ----

#[tauri::command]
pub async fn create_group_from_items(
    state: State<'_, AppState>,
    item_ids: Vec<String>,
    title: Option<String>,
    target_folder_id: Option<String>,
) -> Result<GroupDetail, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let res = run_blocking(move || {
        with_open_db(&db_arc, |db| {
            db.create_group_from_items(&item_ids, title.as_deref(), target_folder_id.as_deref())
        })
    })
    .await?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub async fn add_items_to_group(
    state: State<'_, AppState>,
    group_id: String,
    item_ids: Vec<String>,
) -> Result<GroupDetail, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let res =
        run_blocking(move || with_open_db(&db_arc, |db| db.add_items_to_group(&group_id, &item_ids)))
            .await?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub async fn reorder_group_pages(
    state: State<'_, AppState>,
    group_id: String,
    ordered_item_ids: Vec<String>,
) -> Result<GroupDetail, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let res = run_blocking(move || {
        with_open_db(&db_arc, |db| {
            db.reorder_group_pages(&group_id, &ordered_item_ids)
        })
    })
    .await?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub async fn remove_page_from_group(
    state: State<'_, AppState>,
    group_id: String,
    item_id: String,
) -> Result<Option<GroupDetail>, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let res = run_blocking(move || {
        with_open_db(&db_arc, |db| db.remove_page_from_group(&group_id, &item_id))
    })
    .await?;
    state.notify_lan_library_changed();
    Ok(res)
}

#[tauri::command]
pub async fn ungroup(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Vec<String>, VaultError> {
    let db_arc = Arc::clone(&state.db);
    let res = run_blocking(move || with_open_db(&db_arc, |db| db.ungroup(&group_id))).await?;
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
