use tauri::{AppHandle, State};

use crate::error::VaultError;
use crate::lan_server::{
    list_lan_interfaces, start_lan_server, AssetSource, LanHostNotifier, LanServerHandle,
};
use crate::models::{LanAccessMode, LanInterfaceInfo, LanShareStatus, StartLanShareRequest};
use crate::state::AppState;

fn with_running_lan_handle<F, T>(state: &AppState, f: F) -> Result<T, VaultError>
where
    F: FnOnce(LanServerHandle) -> Result<T, VaultError>,
{
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
    f(h)
}

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
    with_running_lan_handle(&state, |h| Ok(h.set_mode(mode)))
}

#[tauri::command]
pub fn extend_lan_share_timer(
    state: State<'_, AppState>,
    extra_minutes: u32,
) -> Result<LanShareStatus, VaultError> {
    with_running_lan_handle(&state, |h| Ok(h.extend_timer(extra_minutes)))
}

#[tauri::command]
pub fn regenerate_lan_credentials(
    state: State<'_, AppState>,
    disconnect_existing: bool,
) -> Result<LanShareStatus, VaultError> {
    with_running_lan_handle(&state, |h| h.regenerate_credentials(disconnect_existing))
}

#[tauri::command]
pub fn disconnect_lan_clients(state: State<'_, AppState>) -> Result<LanShareStatus, VaultError> {
    with_running_lan_handle(&state, |h| Ok(h.disconnect_all_clients()))
}
