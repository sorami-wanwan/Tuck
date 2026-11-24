use std::path::Path;
use std::sync::Arc;

use tauri::State;

use crate::error::VaultError;
use crate::models::{
    BackupReport, OptimizeReport, PackageExportReport, PackageExportRequest, PackageImportReport,
    PackageImportRequest, PlainExportBatchRequest, PlainExportItemRequest, PlainExportReport,
    VaultHealthInfo,
};
use crate::package;
use crate::plain_export;
use crate::state::AppState;

use super::{run_blocking, with_open_db};

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
    if report.imported_items > 0
        || report.imported_groups > 0
        || report.imported_folders > 0
        || report.merged_duplicate_items > 0
        || report.merged_tag_links > 0
    {
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
