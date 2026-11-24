use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use qrcode::render::svg;
use qrcode::QrCode;
use serde::Deserialize;

use crate::db::now_ms;
use crate::error::VaultError;
use crate::models::{LanAccessMode, LanAuthStatus};

use super::{
    ClientSession, LanApiError, LanServerRuntime, LanSseEvent, PIN_COOLDOWN_SECS,
    PIN_COOLDOWN_THRESHOLD, PIN_HARD_LOCK_THRESHOLD, PIN_WINDOW_SECS, SESSION_COOKIE_NAME,
};

pub(super) fn generate_random_hex(num_bytes: usize) -> Result<String, VaultError> {
    let mut buf = vec![0u8; num_bytes];
    getrandom::fill(&mut buf)
        .map_err(|e| VaultError::LanShare(format!("乱数生成に失敗しました: {e}")))?;
    Ok(hex::encode(buf))
}

pub(super) fn generate_6_digit_pin() -> Result<String, VaultError> {
    const BOUND: u32 = (u32::MAX / 1_000_000) * 1_000_000;
    loop {
        let mut buf = [0u8; 4];
        getrandom::fill(&mut buf)
            .map_err(|e| VaultError::LanShare(format!("PIN 生成に失敗しました: {e}")))?;
        let val = u32::from_le_bytes(buf);
        if val < BOUND {
            return Ok(format!("{:06}", val % 1_000_000));
        }
    }
}

pub(super) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub(super) fn generate_qr_svg(qr_url: &str) -> Result<String, VaultError> {
    let code = QrCode::new(qr_url.as_bytes())
        .map_err(|e| VaultError::LanShare(format!("QRコードの生成に失敗しました: {e}")))?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .quiet_zone(true)
        .dark_color(svg::Color("#111318"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

pub(super) fn verify_csrf(headers: &HeaderMap) -> Result<(), VaultError> {
    let client_hdr = headers
        .get("x-tuck-client")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    if client_hdr != Some("web") {
        return Err(VaultError::LanShare(
            "不正なリクエスト元のため拒否されました".to_string(),
        ));
    }

    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                VaultError::LanShare("Host ヘッダが一致しないため拒否されました".to_string())
            })?;
        let origin_authority = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))
            .map(|rest| rest.split('/').next().unwrap_or(""))
            .unwrap_or("");
        if !origin_authority.eq_ignore_ascii_case(host) {
            return Err(VaultError::LanShare(
                "クロスオリジンからの書き込みリクエストは許可されていません".to_string(),
            ));
        }
    }

    Ok(())
}

pub(super) fn extract_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once('=') {
            if k.trim() == name {
                let val = v.trim();
                if !val.is_empty() {
                    return Some(val);
                }
            }
        }
    }
    None
}

fn parse_device_label(headers: &HeaderMap) -> String {
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let os = if ua.contains("iPhone") {
        "iPhone"
    } else if ua.contains("iPad") {
        "iPad"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Macintosh") || ua.contains("Mac OS") {
        "Mac"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "ブラウザ端末"
    };

    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("CriOS") || ua.contains("Chrome/") {
        "Chrome"
    } else if ua.contains("FxiOS") || ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        ""
    };

    if browser.is_empty() {
        os.to_string()
    } else {
        format!("{os} / {browser}")
    }
}

pub(super) fn require_session(
    runtime: &LanServerRuntime,
    headers: &HeaderMap,
) -> Result<(String, LanAccessMode), VaultError> {
    let sid = extract_cookie(headers, SESSION_COOKIE_NAME).ok_or(VaultError::LanUnauthorized)?;
    let guard = runtime.store_read();
    if !guard.running {
        return Err(VaultError::LanUnauthorized);
    }
    if let Some(exp) = guard.expires_at_ms {
        if now_ms() >= exp {
            return Err(VaultError::LanUnauthorized);
        }
    }
    let mode = guard.mode;
    let session = guard.sessions.get(sid).ok_or(VaultError::LanUnauthorized)?;
    session.last_seen_at_ms.store(now_ms(), Ordering::Relaxed);
    Ok((sid.to_string(), mode))
}

pub(super) fn require_media_auth(
    runtime: &LanServerRuntime,
    headers: &HeaderMap,
    mt: Option<&str>,
) -> Result<(), VaultError> {
    if require_session(runtime, headers).is_ok() {
        return Ok(());
    }
    let token = mt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(VaultError::LanUnauthorized)?;
    let guard = runtime.store_read();
    if !guard.running {
        return Err(VaultError::LanUnauthorized);
    }
    if let Some(exp) = guard.expires_at_ms {
        if now_ms() >= exp {
            return Err(VaultError::LanUnauthorized);
        }
    }
    if let Some((_, session)) = guard
        .sessions
        .iter()
        .find(|(_, s)| ct_eq(s.media_token.as_bytes(), token.as_bytes()))
    {
        session.last_seen_at_ms.store(now_ms(), Ordering::Relaxed);
        return Ok(());
    }
    Err(VaultError::LanUnauthorized)
}

fn require_read_open(
    runtime: &LanServerRuntime,
    headers: &HeaderMap,
) -> Result<String, VaultError> {
    let (sid, _) = require_session(runtime, headers)?;
    if !runtime.app_state.is_open() {
        return Err(VaultError::VaultNotOpen);
    }
    Ok(sid)
}

pub(super) fn error_response(err: VaultError) -> Response {
    let status = err.http_status();
    (status, Json(err)).into_response()
}

pub(super) fn csrf_forbidden_response(err: VaultError) -> Response {
    (StatusCode::FORBIDDEN, Json(err)).into_response()
}

impl From<Response> for LanApiError {
    fn from(resp: Response) -> Self {
        Self(Box::new(resp))
    }
}

impl From<VaultError> for LanApiError {
    fn from(err: VaultError) -> Self {
        Self(Box::new(error_response(err)))
    }
}

pub(super) fn require_write_session_no_csrf(
    runtime: &LanServerRuntime,
    headers: &HeaderMap,
) -> Result<(), LanApiError> {
    let (_, mode) = require_session(runtime, headers)?;
    if !runtime.app_state.is_open() {
        return Err(VaultError::VaultNotOpen.into());
    }
    if mode != LanAccessMode::ReadWrite {
        return Err(VaultError::LanReadOnly.into());
    }
    Ok(())
}

pub(super) fn require_write_checked(
    runtime: &LanServerRuntime,
    headers: &HeaderMap,
) -> Result<(), LanApiError> {
    if let Err(e) = verify_csrf(headers) {
        return Err(csrf_forbidden_response(e).into());
    }
    require_write_session_no_csrf(runtime, headers)
}

pub(super) fn require_read_checked(
    runtime: &LanServerRuntime,
    headers: &HeaderMap,
) -> Result<(), LanApiError> {
    require_read_open(runtime, headers)?;
    Ok(())
}

pub(super) fn require_read_session_checked(
    runtime: &LanServerRuntime,
    headers: &HeaderMap,
) -> Result<String, LanApiError> {
    Ok(require_read_open(runtime, headers)?)
}

pub(super) async fn run_db<F, T>(runtime: &LanServerRuntime, f: F) -> Result<T, LanApiError>
where
    F: FnOnce(&mut crate::db::VaultDb) -> Result<T, VaultError> + Send + 'static,
    T: Send + 'static,
{
    let db_arc = Arc::clone(&runtime.app_state.db);
    let res = tokio::task::spawn_blocking(move || {
        let mut guard = db_arc.lock().map_err(|_| {
            VaultError::InvalidInput("内部状態ロックの取得に失敗しました".to_string())
        })?;
        let db = guard.as_mut().ok_or(VaultError::VaultNotOpen)?;
        f(db)
    })
    .await
    .map_err(|e| VaultError::InvalidInput(format!("処理に失敗しました: {e}")))??;
    Ok(res)
}

pub(super) fn notify_changed(runtime: &LanServerRuntime) {
    let _ = runtime.events_tx.send(LanSseEvent::LibraryChanged);
    runtime.notifier.emit_library_changed();
}

const MAX_LAN_SESSIONS: usize = 64;

fn create_client_session(
    runtime: &LanServerRuntime,
    ip: IpAddr,
    headers: &HeaderMap,
) -> Result<(String, LanAuthStatus), VaultError> {
    let sid = generate_random_hex(32)?;
    let media_token = generate_random_hex(16)?;
    let session_id = generate_random_hex(8)?;
    let now = now_ms();
    let device_label = parse_device_label(headers);

    let (mode, expires_at_ms) = {
        let mut guard = runtime.store_write();
        let expired = guard.expires_at_ms.is_some_and(|exp| now >= exp);
        if !guard.running || expired || !runtime.app_state.is_open() {
            return Err(VaultError::VaultNotOpen);
        }
        while guard.sessions.len() >= MAX_LAN_SESSIONS {
            if let Some(oldest_sid) = guard
                .sessions
                .iter()
                .min_by_key(|(_, s)| s.last_seen_at_ms.load(Ordering::Relaxed))
                .map(|(k, _)| k.clone())
            {
                guard.sessions.remove(&oldest_sid);
            } else {
                break;
            }
        }
        guard.sessions.insert(
            sid.clone(),
            ClientSession {
                id: session_id,
                media_token: media_token.clone(),
                ip: ip.to_canonical().to_string(),
                device_label,
                connected_at_ms: now,
                last_seen_at_ms: Arc::new(AtomicI64::new(now)),
            },
        );
        (guard.mode, guard.expires_at_ms)
    };

    runtime.notifier.emit_lan_status_changed();
    Ok((
        sid,
        LanAuthStatus {
            authenticated: true,
            mode,
            expires_at_ms,
            media_token: Some(media_token),
            vault_open: runtime.app_state.is_open(),
        },
    ))
}

fn build_session_cookie_header(sid: &str) -> HeaderValue {
    let raw = format!("{SESSION_COOKIE_NAME}={sid}; HttpOnly; SameSite=Lax; Path=/");
    HeaderValue::from_str(&raw).expect("valid cookie header")
}

// ---- Auth Handlers ----

pub(super) async fn handle_auth_status(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
) -> Response {
    let sid = extract_cookie(&headers, SESSION_COOKIE_NAME);
    let guard = runtime.store_read();
    let vault_open = runtime.app_state.is_open();
    let expired = guard.expires_at_ms.is_some_and(|exp| now_ms() >= exp);
    if !guard.running || expired {
        return Json(LanAuthStatus {
            authenticated: false,
            mode: guard.mode,
            expires_at_ms: None,
            media_token: None,
            vault_open,
        })
        .into_response();
    }

    if let Some(sid_val) = sid {
        if let Some(session) = guard.sessions.get(sid_val) {
            session.last_seen_at_ms.store(now_ms(), Ordering::Relaxed);
            let mt = session.media_token.clone();
            return Json(LanAuthStatus {
                authenticated: true,
                mode: guard.mode,
                expires_at_ms: guard.expires_at_ms,
                media_token: Some(mt),
                vault_open,
            })
            .into_response();
        }
    }

    Json(LanAuthStatus {
        authenticated: false,
        mode: guard.mode,
        expires_at_ms: guard.expires_at_ms,
        media_token: None,
        vault_open,
    })
    .into_response()
}

#[derive(Deserialize)]
pub(super) struct PinAuthPayload {
    pin: String,
}

pub(super) async fn handle_auth_pin(
    State(runtime): State<Arc<LanServerRuntime>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(payload): Json<PinAuthPayload>,
) -> Response {
    if let Err(e) = verify_csrf(&headers) {
        return csrf_forbidden_response(e);
    }
    let ip = addr.ip().to_canonical();
    let submitted = payload.pin.trim();
    if submitted.len() != 6 || !submitted.bytes().all(|b| b.is_ascii_digit()) {
        return error_response(VaultError::InvalidInput(
            "6桁のPINを入力してください".to_string(),
        ));
    }

    let now_inst = Instant::now();
    {
        let mut guard = runtime.store_write();
        let expired = guard.expires_at_ms.is_some_and(|exp| now_ms() >= exp);
        if !guard.running || expired || !runtime.app_state.is_open() {
            return error_response(VaultError::VaultNotOpen);
        }

        let tracker = guard.pin_failures.entry(ip).or_default();
        if tracker.total_failures >= PIN_HARD_LOCK_THRESHOLD {
            return error_response(VaultError::LanRateLimited(
                "試行回数の上限に達しました。ホスト側でPINを再発行してください".to_string(),
            ));
        }
        if let Some(until) = tracker.cooldown_until {
            if now_inst < until {
                let secs = until.saturating_duration_since(now_inst).as_secs().max(1);
                return error_response(VaultError::LanRateLimited(format!(
                    "PINの入力失敗が続いたため {secs} 秒後に再試行してください"
                )));
            }
            tracker.cooldown_until = None;
        }

        if ct_eq(guard.pin.as_bytes(), submitted.as_bytes()) {
            guard.pin_failures.remove(&ip);
        } else {
            let tracker = guard.pin_failures.entry(ip).or_default();
            tracker.recent_failures.retain(|t| {
                now_inst.saturating_duration_since(*t) < Duration::from_secs(PIN_WINDOW_SECS)
            });
            tracker.recent_failures.push(now_inst);
            tracker.total_failures += 1;
            if tracker.total_failures >= PIN_HARD_LOCK_THRESHOLD {
                return error_response(VaultError::LanRateLimited(
                    "試行回数の上限に達しました。ホスト側でPINを再発行してください".to_string(),
                ));
            }
            if tracker.recent_failures.len() >= PIN_COOLDOWN_THRESHOLD {
                tracker.recent_failures.clear();
                tracker.cooldown_until = Some(now_inst + Duration::from_secs(PIN_COOLDOWN_SECS));
                return error_response(VaultError::LanRateLimited(
                    "PINの入力失敗が5回に達したため、60秒間ロックしました".to_string(),
                ));
            }
            return error_response(VaultError::LanUnauthorized);
        }
    }

    match create_client_session(&runtime, ip, &headers) {
        Ok((sid, status)) => {
            let mut res = Json(status).into_response();
            res.headers_mut()
                .insert(header::SET_COOKIE, build_session_cookie_header(&sid));
            res
        }
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
pub(super) struct TokenAuthPayload {
    token: String,
}

pub(super) async fn handle_auth_token(
    State(runtime): State<Arc<LanServerRuntime>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(payload): Json<TokenAuthPayload>,
) -> Response {
    if let Err(e) = verify_csrf(&headers) {
        return csrf_forbidden_response(e);
    }
    let ip = addr.ip().to_canonical();
    let submitted = payload.token.trim();

    {
        let guard = runtime.store_read();
        let expired = guard.expires_at_ms.is_some_and(|exp| now_ms() >= exp);
        if !guard.running || expired || !runtime.app_state.is_open() {
            return error_response(VaultError::VaultNotOpen);
        }
        if submitted.is_empty() || !ct_eq(guard.qr_token.as_bytes(), submitted.as_bytes()) {
            return error_response(VaultError::LanUnauthorized);
        }
    }

    match create_client_session(&runtime, ip, &headers) {
        Ok((sid, status)) => {
            let mut res = Json(status).into_response();
            res.headers_mut()
                .insert(header::SET_COOKIE, build_session_cookie_header(&sid));
            res
        }
        Err(e) => error_response(e),
    }
}

pub(super) async fn handle_auth_logout(
    State(runtime): State<Arc<LanServerRuntime>>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = verify_csrf(&headers) {
        return csrf_forbidden_response(e);
    }
    if let Some(sid) = extract_cookie(&headers, SESSION_COOKIE_NAME) {
        let mut guard = runtime.store_write();
        guard.sessions.remove(sid);
    }
    runtime.notifier.emit_lan_status_changed();
    let mut res = StatusCode::NO_CONTENT.into_response();
    let clear_cookie = format!("{SESSION_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&clear_cookie).expect("valid cookie"),
    );
    res
}
