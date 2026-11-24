mod assets;
mod auth;
mod handlers_read;
mod handlers_write;
mod middleware;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use axum::extract::DefaultBodyLimit;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot, Notify};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::db::now_ms;
use crate::error::VaultError;
use crate::models::{
    EntryRef, LanAccessMode, LanClientInfo, LanInterfaceInfo, LanShareStatus, StartLanShareRequest,
    UrlImportProgressEvent,
};
use crate::state::AppState;

use assets::{handle_serve_index, handle_serve_static};
use auth::{
    generate_6_digit_pin, generate_qr_svg, generate_random_hex, handle_auth_logout,
    handle_auth_pin, handle_auth_status, handle_auth_token,
};
use handlers_read::{
    handle_create_export_batch_ticket, handle_export_batch_by_ticket, handle_export_folder,
    handle_export_group, handle_export_item, handle_get_group_detail, handle_get_image,
    handle_get_item_detail, handle_get_media, handle_get_thumbnail, handle_head_export_folder,
    handle_head_export_group, handle_head_export_item, handle_head_media, handle_list_folders,
    handle_list_tags, handle_query_entries, handle_sse_events, handle_vault_status,
};
use handlers_write::{
    handle_add_entries_to_folder, handle_add_group_tag, handle_add_item_tag,
    handle_add_items_to_group, handle_batch_set_entry_rating, handle_batch_update_entry_tags,
    handle_cancel_url_import, handle_check_ytdlp_status, handle_create_folder, handle_create_group,
    handle_delete_folder, handle_import_bytes, handle_import_url, handle_move_folder,
    handle_remove_entries_from_folder, handle_remove_group_tag, handle_remove_item_tag,
    handle_remove_page_from_group, handle_rename_folder, handle_reorder_group_pages,
    handle_save_item_thumbnail, handle_set_group_memo, handle_set_group_rating,
    handle_set_group_reader_prefs, handle_set_group_title, handle_set_item_memo,
    handle_set_item_rating, handle_set_item_source_url, handle_set_item_title, handle_trash_delete,
    handle_trash_empty, handle_trash_move, handle_trash_restore, handle_ungroup,
};
use middleware::{lan_ip_and_host_middleware, security_headers_middleware};

pub(super) const SESSION_COOKIE_NAME: &str = "tuck_lan_session";
pub(super) const PIN_WINDOW_SECS: u64 = 300;
pub(super) const PIN_COOLDOWN_THRESHOLD: usize = 5;
pub(super) const PIN_COOLDOWN_SECS: u64 = 60;
pub(super) const PIN_HARD_LOCK_THRESHOLD: usize = 15;
pub(super) const MAX_THUMBNAIL_UPLOAD_BYTES: usize = 16 * 1024 * 1024;
pub(super) const STREAM_RAM_BUFFER_THRESHOLD: usize = 64 * 1024 * 1024;
pub(super) const DEFAULT_UPLOAD_LIMIT_MB: u32 = 2048;
pub(super) const EXPORT_TICKET_TTL_SECS: u64 = 60;
pub(super) const MAX_EXPORT_TICKETS: usize = 32;

pub(super) const LAN_BROWSER_CSP: &str = "default-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'";

pub(crate) struct LanApiError(Box<Response>);

impl IntoResponse for LanApiError {
    fn into_response(self) -> Response {
        *self.0
    }
}

pub(crate) struct EphemeralUploadFile {
    path: PathBuf,
    writer: Option<fs::File>,
}

impl EphemeralUploadFile {
    pub fn create() -> Result<Self, std::io::Error> {
        let path = std::env::temp_dir().join(format!("tuck-upload-{}.bin", Uuid::new_v4()));
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?
        };
        #[cfg(not(unix))]
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            path,
            writer: Some(file),
        })
    }

    pub fn write_all(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
        if let Some(ref mut f) = self.writer {
            f.write_all(data)
        } else {
            Err(std::io::Error::other("upload writer already closed"))
        }
    }

    pub fn finish_writing(&mut self) -> Result<(), std::io::Error> {
        if let Some(mut f) = self.writer.take() {
            f.flush()?;
            let _ = f.sync_all();
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for EphemeralUploadFile {
    fn drop(&mut self) {
        self.writer.take();
        if let Ok(meta) = fs::metadata(&self.path) {
            let mut remaining = meta.len();
            if remaining > 0 {
                if let Ok(mut f) = OpenOptions::new().write(true).open(&self.path) {
                    let zeros = vec![0u8; 1024 * 1024];
                    while remaining > 0 {
                        let n = (remaining as usize).min(zeros.len());
                        if f.write_all(&zeros[..n]).is_err() {
                            break;
                        }
                        remaining -= n as u64;
                    }
                    let _ = f.flush();
                    let _ = f.sync_all();
                }
            }
        }
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum AssetSource {
    Tauri(AppHandle),
    Dir(PathBuf),
    None,
}

#[derive(Clone, Default)]
pub struct LanHostNotifier {
    app: Option<AppHandle>,
}

impl LanHostNotifier {
    pub fn from_app(app: AppHandle) -> Self {
        Self { app: Some(app) }
    }

    pub fn emit_library_changed(&self) {
        if let Some(app) = &self.app {
            let _ = app.emit("vault://library-changed", ());
        }
    }

    pub fn emit_lan_status_changed(&self) {
        if let Some(app) = &self.app {
            let _ = app.emit("vault://lan-status-changed", ());
        }
    }

    pub fn emit_url_import_progress(&self, ev: &UrlImportProgressEvent) {
        if let Some(app) = &self.app {
            let _ = app.emit("vault://url-import-progress", ev);
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LanSseEvent {
    VaultLocked,
    LanStopped { reason: String },
    LanModeChanged { mode: LanAccessMode },
    LibraryChanged,
    UrlImportProgress { payload: UrlImportProgressEvent },
}

#[derive(Clone)]
pub(super) struct ClientSession {
    pub(super) id: String,
    pub(super) media_token: String,
    pub(super) ip: String,
    pub(super) device_label: String,
    pub(super) connected_at_ms: i64,
    pub(super) last_seen_at_ms: i64,
}

#[derive(Clone)]
pub(super) struct ExportTicket {
    pub(super) session_id: String,
    pub(super) entries: Vec<EntryRef>,
    pub(super) folder_ids: Vec<String>,
    pub(super) include_subfolders: bool,
    pub(super) created_at: Instant,
}

#[derive(Default)]
pub(super) struct FailureTracker {
    pub(super) recent_failures: Vec<Instant>,
    pub(super) cooldown_until: Option<Instant>,
    pub(super) total_failures: usize,
}

pub(super) struct LanSessionStore {
    pub(super) running: bool,
    pub(super) mode: LanAccessMode,
    pub(super) pin: Zeroizing<String>,
    pub(super) qr_token: Zeroizing<String>,
    pub(super) qr_svg: String,
    pub(super) url: String,
    pub(super) qr_url: String,
    pub(super) port: u16,
    pub(super) selected_ip: String,
    pub(super) interfaces: Vec<LanInterfaceInfo>,
    pub(super) started_at_ms: Option<i64>,
    pub(super) expires_at_ms: Option<i64>,
    pub(super) pause_idle_lock: bool,
    pub(super) sessions: HashMap<String, ClientSession>,
    pub(super) pin_failures: HashMap<IpAddr, FailureTracker>,
    pub(super) export_tickets: HashMap<String, ExportTicket>,
}

pub(super) struct LanServerRuntime {
    pub(super) store: RwLock<LanSessionStore>,
    pub(super) app_state: AppState,
    pub(super) assets: AssetSource,
    pub(super) notifier: LanHostNotifier,
    pub(super) events_tx: broadcast::Sender<LanSseEvent>,
    pub(super) timer_notify: Arc<Notify>,
    pub(super) shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub(super) selected_ip: String,
    pub(super) port: u16,
    pub(super) allow_tailscale: bool,
}

#[derive(Clone)]
pub struct LanServerHandle {
    runtime: Arc<LanServerRuntime>,
}

impl LanServerHandle {
    pub fn status(&self) -> LanShareStatus {
        self.runtime.build_status()
    }

    pub fn set_mode(&self, mode: LanAccessMode) -> LanShareStatus {
        {
            let mut guard = self.runtime.store_write();
            guard.mode = mode;
        }
        let _ = self
            .runtime
            .events_tx
            .send(LanSseEvent::LanModeChanged { mode });
        self.runtime.notifier.emit_lan_status_changed();
        self.runtime.build_status()
    }

    pub fn extend_timer(&self, extra_minutes: u32) -> LanShareStatus {
        let extra_ms = i64::from(extra_minutes.max(1)) * 60_000;
        {
            let mut guard = self.runtime.store_write();
            let base = guard.expires_at_ms.unwrap_or_else(now_ms).max(now_ms());
            guard.expires_at_ms = Some(base.saturating_add(extra_ms));
        }
        self.runtime.timer_notify.notify_waiters();
        self.runtime.notifier.emit_lan_status_changed();
        self.runtime.build_status()
    }

    pub fn regenerate_credentials(
        &self,
        disconnect_existing: bool,
    ) -> Result<LanShareStatus, VaultError> {
        let new_pin = Zeroizing::new(generate_6_digit_pin()?);
        let new_token = Zeroizing::new(generate_random_hex(32)?);
        {
            let mut guard = self.runtime.store_write();
            guard.pin = new_pin;
            guard.qr_token = new_token;
            guard.qr_url = format!("{}/?k={}", guard.url, *guard.qr_token);
            guard.qr_svg = generate_qr_svg(&guard.qr_url)?;
            guard.pin_failures.clear();
            if disconnect_existing {
                guard.sessions.clear();
                guard.export_tickets.clear();
            }
        }
        if disconnect_existing {
            let _ = self.runtime.events_tx.send(LanSseEvent::LanStopped {
                reason: "credentials_regenerated".to_string(),
            });
        }
        self.runtime.notifier.emit_lan_status_changed();
        Ok(self.runtime.build_status())
    }

    pub fn disconnect_all_clients(&self) -> LanShareStatus {
        {
            let mut guard = self.runtime.store_write();
            guard.sessions.clear();
            guard.export_tickets.clear();
        }
        let _ = self.runtime.events_tx.send(LanSseEvent::LanStopped {
            reason: "disconnected".to_string(),
        });
        self.runtime.notifier.emit_lan_status_changed();
        self.runtime.build_status()
    }

    pub fn broadcast_library_changed(&self) {
        let _ = self.runtime.events_tx.send(LanSseEvent::LibraryChanged);
    }

    pub fn broadcast_url_progress(&self, ev: &UrlImportProgressEvent) {
        let _ = self.runtime.events_tx.send(LanSseEvent::UrlImportProgress {
            payload: ev.clone(),
        });
    }

    pub fn stop(&self, reason: &str) {
        {
            let mut guard = self.runtime.store_write();
            if !guard.running {
                return;
            }
            guard.running = false;
            guard.sessions.clear();
            guard.export_tickets.clear();
            guard.pin = Zeroizing::new(String::new());
            guard.qr_token = Zeroizing::new(String::new());
            guard.qr_url.clear();
            guard.pin_failures.clear();
        }
        let ev = if reason == "vault_locked" {
            LanSseEvent::VaultLocked
        } else {
            LanSseEvent::LanStopped {
                reason: reason.to_string(),
            }
        };
        let _ = self.runtime.events_tx.send(ev);
        if let Ok(mut tx_guard) = self.runtime.shutdown_tx.lock() {
            if let Some(tx) = tx_guard.take() {
                let _ = tx.send(());
            }
        }
        self.runtime.timer_notify.notify_waiters();
        self.runtime.notifier.emit_lan_status_changed();
    }
}

impl LanServerRuntime {
    pub(super) fn store_read(&self) -> RwLockReadGuard<'_, LanSessionStore> {
        self.store.read().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn store_write(&self) -> RwLockWriteGuard<'_, LanSessionStore> {
        self.store.write().unwrap_or_else(|e| e.into_inner())
    }

    fn build_status(&self) -> LanShareStatus {
        let guard = self.store_read();
        let mut clients: Vec<LanClientInfo> = guard
            .sessions
            .values()
            .map(|s| LanClientInfo {
                id: s.id.clone(),
                ip: s.ip.clone(),
                device_label: s.device_label.clone(),
                connected_at_ms: s.connected_at_ms,
                last_seen_at_ms: s.last_seen_at_ms,
            })
            .collect();
        clients.sort_by_key(|c| std::cmp::Reverse(c.last_seen_at_ms));
        LanShareStatus {
            running: guard.running,
            mode: guard.mode,
            url: guard.url.clone(),
            qr_url: guard.qr_url.clone(),
            qr_svg: guard.qr_svg.clone(),
            pin: guard.pin.as_str().to_string(),
            port: guard.port,
            selected_ip: guard.selected_ip.clone(),
            interfaces: guard.interfaces.clone(),
            started_at_ms: guard.started_at_ms,
            expires_at_ms: guard.expires_at_ms,
            pause_idle_lock: guard.pause_idle_lock,
            clients,
        }
    }
}

pub fn is_tailscale_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (o[1] & 0b1100_0000) == 64
}

fn is_tailscale_interface_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("tailscale") || lower.starts_with("utun")
}

pub fn is_private_lan_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return true;
            }
            let seg0 = v6.segments()[0];
            // Unique Local Address (fc00::/7) or Unicast Link-Local (fe80::/10)
            (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80
        }
    }
}

pub fn is_allowed_lan_peer_ip(ip: IpAddr, allow_tailscale: bool) -> bool {
    if is_private_lan_ip(ip) {
        return true;
    }
    if allow_tailscale {
        if let IpAddr::V4(v4) = ip.to_canonical() {
            return is_tailscale_ipv4(v4);
        }
    }
    false
}

fn interface_priority(name: &str, ip: Ipv4Addr, is_tailscale: bool) -> (u8, u8, u8, String) {
    let octets = ip.octets();
    let subnet_rank = if octets[0] == 192 && octets[1] == 168 {
        0
    } else if octets[0] == 10 {
        1
    } else {
        2
    };
    let lower = name.to_ascii_lowercase();
    let iface_rank = if lower == "en0" || lower.starts_with("wl") || lower.contains("wi-fi") {
        0
    } else if lower.starts_with("en") || lower.starts_with("eth") {
        1
    } else if lower.starts_with("utun")
        || lower.starts_with("tun")
        || lower.starts_with("tap")
        || lower.starts_with("docker")
        || lower.starts_with("veth")
        || lower.starts_with("bridge")
    {
        3
    } else {
        2
    };
    let tailscale_group = if is_tailscale { 1 } else { 0 };
    (tailscale_group, iface_rank, subnet_rank, name.to_string())
}

pub fn list_lan_interfaces() -> Vec<LanInterfaceInfo> {
    let Ok(addrs) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut seen_ips = HashMap::<String, LanInterfaceInfo>::new();
    for iface in addrs {
        if iface.is_loopback() {
            continue;
        }
        if let IpAddr::V4(v4) = iface.ip() {
            let is_tailscale = if v4.is_private() {
                false
            } else if is_tailscale_ipv4(v4) && is_tailscale_interface_name(&iface.name) {
                true
            } else {
                continue;
            };
            let ip_str = v4.to_string();
            seen_ips.entry(ip_str.clone()).or_insert(LanInterfaceInfo {
                name: iface.name,
                ip: ip_str,
                is_primary: false,
                is_tailscale,
            });
        }
    }

    let mut list: Vec<(LanInterfaceInfo, (u8, u8, u8, String))> = seen_ips
        .into_values()
        .filter_map(|info| {
            let parsed = info.ip.parse::<Ipv4Addr>().ok()?;
            let prio = interface_priority(&info.name, parsed, info.is_tailscale);
            Some((info, prio))
        })
        .collect();

    list.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.ip.cmp(&b.0.ip)));

    let mut result: Vec<LanInterfaceInfo> = list.into_iter().map(|(info, _)| info).collect();
    if let Some(first) = result.first_mut() {
        first.is_primary = true;
    }
    result
}

pub async fn start_lan_server(
    app_state: AppState,
    request: StartLanShareRequest,
    assets: AssetSource,
    notifier: LanHostNotifier,
) -> Result<LanShareStatus, VaultError> {
    let interfaces = list_lan_interfaces();
    let selected_ipv4: Ipv4Addr = if let Some(req_ip) = request
        .bind_ip
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let parsed: Ipv4Addr = req_ip.parse().map_err(|_| {
            VaultError::InvalidInput("指定された IP アドレスが不正です".to_string())
        })?;
        if !parsed.is_private() && !parsed.is_loopback() && !is_tailscale_ipv4(parsed) {
            return Err(VaultError::InvalidInput(
                "LAN または Tailscale のプライベート IP のみ指定できます".to_string(),
            ));
        }
        parsed
    } else {
        interfaces
            .first()
            .and_then(|i| i.ip.parse::<Ipv4Addr>().ok())
            .unwrap_or(Ipv4Addr::LOCALHOST)
    };
    let selected_ip = selected_ipv4.to_string();

    // 選択したインターフェースの IP のみに厳格バインドし、未選択 NIC への露出を防ぐ
    let bind_addr = SocketAddr::from((selected_ipv4, 0));
    let (handle, status) = start_lan_server_on_addr(
        app_state.clone(),
        request,
        bind_addr,
        selected_ip,
        interfaces,
        assets,
        notifier,
    )
    .await?;

    {
        let mut guard = app_state
            .lan_server
            .lock()
            .map_err(|_| VaultError::LanShare("内部状態ロックの取得に失敗しました".to_string()))?;
        if let Some(old) = guard.take() {
            old.stop("restarted");
        }
        *guard = Some(handle);
    }

    Ok(status)
}

pub(crate) async fn start_lan_server_on_addr(
    app_state: AppState,
    request: StartLanShareRequest,
    bind_addr: SocketAddr,
    selected_ip: String,
    interfaces: Vec<LanInterfaceInfo>,
    assets: AssetSource,
    notifier: LanHostNotifier,
) -> Result<(LanServerHandle, LanShareStatus), VaultError> {
    if !app_state.is_open() {
        return Err(VaultError::VaultNotOpen);
    }

    let listener = TcpListener::bind(bind_addr)
        .await
        .map_err(|e| VaultError::LanShare(format!("LAN サーバーの待受開始に失敗しました: {e}")))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| VaultError::LanShare(format!("待受ポートの取得に失敗しました: {e}")))?;
    let port = local_addr.port();

    let allow_tailscale = selected_ip.parse::<Ipv4Addr>().is_ok_and(is_tailscale_ipv4);

    let url = format!("http://{selected_ip}:{port}");
    let pin = Zeroizing::new(generate_6_digit_pin()?);
    let qr_token = Zeroizing::new(generate_random_hex(32)?);
    let qr_url = format!("{url}/?k={}", *qr_token);
    let qr_svg = generate_qr_svg(&qr_url)?;

    let started_at_ms = now_ms();
    let expires_at_ms = if request.duration_minutes > 0 {
        Some(started_at_ms.saturating_add(i64::from(request.duration_minutes) * 60_000))
    } else {
        None
    };

    let (events_tx, _) = broadcast::channel::<LanSseEvent>(64);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let timer_notify = Arc::new(Notify::new());

    let runtime = Arc::new(LanServerRuntime {
        store: RwLock::new(LanSessionStore {
            running: true,
            mode: request.mode,
            pin,
            qr_token,
            qr_svg,
            url,
            qr_url,
            port,
            selected_ip: selected_ip.clone(),
            interfaces,
            started_at_ms: Some(started_at_ms),
            expires_at_ms,
            pause_idle_lock: request.pause_idle_lock,
            sessions: HashMap::new(),
            pin_failures: HashMap::new(),
            export_tickets: HashMap::new(),
        }),
        app_state: app_state.clone(),
        assets,
        notifier: notifier.clone(),
        events_tx,
        timer_notify: Arc::clone(&timer_notify),
        shutdown_tx: Mutex::new(Some(shutdown_tx)),
        selected_ip,
        port,
        allow_tailscale,
    });

    let handle = LanServerHandle {
        runtime: Arc::clone(&runtime),
    };

    let router = build_router(Arc::clone(&runtime));
    let serve_handle = handle.clone();
    tokio::spawn(async move {
        let make_svc = router.into_make_service_with_connect_info::<SocketAddr>();
        let _ = axum::serve(listener, make_svc)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
        serve_handle.stop("server_exited");
    });

    // Auto-stop timer watcher
    let timer_runtime = Arc::clone(&runtime);
    let timer_app_state = app_state;
    tokio::spawn(async move {
        loop {
            let (is_running, exp_ms) = {
                let g = timer_runtime.store_read();
                (g.running, g.expires_at_ms)
            };
            if !is_running {
                break;
            }
            match exp_ms {
                None => {
                    timer_runtime.timer_notify.notified().await;
                }
                Some(deadline_ms) => {
                    let remain_ms = deadline_ms - now_ms();
                    if remain_ms <= 0 {
                        timer_app_state.stop_lan_share_with_reason("expired");
                        break;
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(remain_ms as u64)) => {
                            let still_expired = {
                                let g = timer_runtime.store_read();
                                g.running && g.expires_at_ms.is_some_and(|e| now_ms() >= e)
                            };
                            if still_expired {
                                timer_app_state.stop_lan_share_with_reason("expired");
                                break;
                            }
                        }
                        _ = timer_runtime.timer_notify.notified() => {}
                    }
                }
            }
        }
    });

    let status = handle.status();
    notifier.emit_lan_status_changed();
    Ok((handle, status))
}

fn build_router(runtime: Arc<LanServerRuntime>) -> Router {
    Router::new()
        // Unauthenticated auth endpoints
        .route("/api/auth/status", get(handle_auth_status))
        .route("/api/auth/pin", post(handle_auth_pin))
        .route("/api/auth/token", post(handle_auth_token))
        .route("/api/auth/logout", post(handle_auth_logout))
        // Authenticated Read endpoints
        .route("/api/events", get(handle_sse_events))
        .route("/api/vault/status", get(handle_vault_status))
        .route("/api/entries/query", post(handle_query_entries))
        .route("/api/items/{id}", get(handle_get_item_detail))
        .route("/api/groups/{id}", get(handle_get_group_detail))
        .route(
            "/api/folders",
            get(handle_list_folders).post(handle_create_folder),
        )
        .route("/api/tags", get(handle_list_tags))
        .route("/api/thumbnails/{id}", get(handle_get_thumbnail))
        .route("/api/images/{id}", get(handle_get_image))
        .route(
            "/api/media/{id}",
            get(handle_get_media).head(handle_head_media),
        )
        .route(
            "/api/export/item/{id}",
            get(handle_export_item).head(handle_head_export_item),
        )
        .route(
            "/api/export/group/{id}",
            get(handle_export_group).head(handle_head_export_group),
        )
        .route(
            "/api/export/folder/{id}",
            get(handle_export_folder).head(handle_head_export_folder),
        )
        .route(
            "/api/export/batch-ticket",
            post(handle_create_export_batch_ticket),
        )
        .route(
            "/api/export/batch/{ticket}",
            get(handle_export_batch_by_ticket),
        )
        // Authenticated Write endpoints
        .route("/api/items/{id}/title", patch(handle_set_item_title))
        .route("/api/items/{id}/rating", patch(handle_set_item_rating))
        .route("/api/items/{id}/memo", patch(handle_set_item_memo))
        .route(
            "/api/items/{id}/source-url",
            patch(handle_set_item_source_url),
        )
        .route("/api/items/{id}/tags", post(handle_add_item_tag))
        .route("/api/items/{id}/tags/{tag}", delete(handle_remove_item_tag))
        .route(
            "/api/entries/tags/batch",
            post(handle_batch_update_entry_tags),
        )
        .route(
            "/api/entries/rating/batch",
            post(handle_batch_set_entry_rating),
        )
        .route(
            "/api/items/{id}/thumbnail",
            post(handle_save_item_thumbnail).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/api/folders/{id}",
            patch(handle_rename_folder).delete(handle_delete_folder),
        )
        .route("/api/folders/{id}/move", post(handle_move_folder))
        .route(
            "/api/folders/{id}/entries",
            post(handle_add_entries_to_folder).delete(handle_remove_entries_from_folder),
        )
        .route("/api/groups", post(handle_create_group))
        .route("/api/groups/{id}/items", post(handle_add_items_to_group))
        .route(
            "/api/groups/{id}/pages/order",
            put(handle_reorder_group_pages),
        )
        .route(
            "/api/groups/{id}/pages/{item_id}",
            delete(handle_remove_page_from_group),
        )
        .route("/api/groups/{id}/ungroup", post(handle_ungroup))
        .route("/api/groups/{id}/title", patch(handle_set_group_title))
        .route("/api/groups/{id}/rating", patch(handle_set_group_rating))
        .route("/api/groups/{id}/memo", patch(handle_set_group_memo))
        .route(
            "/api/groups/{id}/reader-prefs",
            patch(handle_set_group_reader_prefs),
        )
        .route("/api/groups/{id}/tags", post(handle_add_group_tag))
        .route(
            "/api/groups/{id}/tags/{tag}",
            delete(handle_remove_group_tag),
        )
        .route("/api/trash/move", post(handle_trash_move))
        .route("/api/trash/restore", post(handle_trash_restore))
        .route("/api/trash/delete", post(handle_trash_delete))
        .route("/api/trash/empty", post(handle_trash_empty))
        .route(
            "/api/import/bytes",
            post(handle_import_bytes).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/import/ytdlp-status", get(handle_check_ytdlp_status))
        .route("/api/import/url", post(handle_import_url))
        .route("/api/import/url/cancel", post(handle_cancel_url_import))
        // SPA static assets fallback
        .route("/", get(handle_serve_index))
        .route("/{*path}", get(handle_serve_static))
        .layer(from_fn_with_state(
            Arc::clone(&runtime),
            lan_ip_and_host_middleware,
        ))
        .layer(from_fn(security_headers_middleware))
        .with_state(runtime)
}
