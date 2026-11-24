use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use tauri::{AppHandle, State};

use crate::clipboard;
use crate::db::VaultDb;
use crate::error::VaultError;
use crate::models::{LockSettings, VaultStatus};
use crate::state::AppState;
use crate::{clear_global_lock_shortcuts, sync_global_lock_shortcut};

use super::{run_blocking, with_open_db};

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
        state_clone.set_db(Some(db))?;
        Ok(status)
    })
    .await?;

    let _ = sync_global_lock_shortcut(&app, &status.lock_settings.shortcut);
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
        state_clone.set_db(Some(db))?;
        Ok(status)
    })
    .await?;

    let _ = sync_global_lock_shortcut(&app, &status.lock_settings.shortcut);
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
        state_clone.set_db(Some(db))?;
        Ok(status)
    })
    .await?;

    let _ = sync_global_lock_shortcut(&app, &status.lock_settings.shortcut);
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
    let prev = with_open_db(&state.db, |db| Ok(db.get_lock_settings()))?;
    if let Err(err) = sync_global_lock_shortcut(&app, &settings.shortcut) {
        let _ = sync_global_lock_shortcut(&app, &prev.shortcut);
        return Err(err);
    }
    match with_open_db(&state.db, |db| db.set_lock_settings(&settings)) {
        Ok(saved) => Ok(saved),
        Err(err) => {
            let _ = sync_global_lock_shortcut(&app, &prev.shortcut);
            Err(err)
        }
    }
}

#[tauri::command]
pub async fn set_clipboard_watch(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<VaultStatus, VaultError> {
    let state_clone = state.inner().clone();
    run_blocking(move || {
        if enabled {
            if !state_clone.is_open() {
                return Err(VaultError::VaultNotOpen);
            }
            let initial_hash = clipboard::read_clipboard_hash_only();
            if let Ok(mut h) = state_clone.clipboard_last_hash.lock() {
                *h = initial_hash;
            }
            state_clone
                .clipboard_watch_enabled
                .store(true, Ordering::SeqCst);
        } else {
            state_clone.stop_clipboard_watch();
        }
        state_clone.current_status()
    })
    .await
}
