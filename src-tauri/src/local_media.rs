use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{ConnectInfo, Path as AxumPath, Query, State};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use zeroize::Zeroizing;

use crate::error::VaultError;
use crate::media_stream::build_http_media_response;
use crate::state::AppState;

#[derive(Clone)]
pub struct LocalMediaServerHandle {
    base_url: String,
    token: Arc<Zeroizing<String>>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    stopped: Arc<AtomicBool>,
}

impl LocalMediaServerHandle {
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn token(&self) -> &str {
        self.token.as_str()
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.shutdown_tx.lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(());
            }
        }
    }
}

struct LocalMediaRuntime {
    app_state: AppState,
    expected_host: String,
    token: Arc<Zeroizing<String>>,
    stopped: Arc<AtomicBool>,
}

#[derive(Deserialize)]
struct TokenQuery {
    t: Option<String>,
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn generate_random_token_hex(num_bytes: usize) -> Result<String, VaultError> {
    let mut buf = vec![0u8; num_bytes];
    getrandom::fill(&mut buf).map_err(|e| {
        VaultError::Io(std::io::Error::other(format!(
            "乱数生成に失敗しました: {e}"
        )))
    })?;
    Ok(hex::encode(buf))
}

fn status_response(status: StatusCode, allowed_origin: Option<&'static str>) -> Response {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::VARY, "Origin")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(origin) = allowed_origin {
        builder = builder.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    builder
        .body(Body::empty())
        .unwrap_or_else(|_| status.into_response())
}

fn map_vault_error_response(err: &VaultError, allowed_origin: Option<&'static str>) -> Response {
    let status = match err {
        VaultError::VaultNotOpen => StatusCode::FORBIDDEN,
        VaultError::NotFound(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    status_response(status, allowed_origin)
}

async fn handle_get_local_media(
    State(runtime): State<Arc<LocalMediaRuntime>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<TokenQuery>,
) -> Response {
    serve_local_media_inner(runtime, addr, headers, id, query, false, false).await
}

async fn handle_head_local_media(
    State(runtime): State<Arc<LocalMediaRuntime>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<TokenQuery>,
) -> Response {
    serve_local_media_inner(runtime, addr, headers, id, query, true, false).await
}

async fn handle_options_local_media(
    State(runtime): State<Arc<LocalMediaRuntime>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<TokenQuery>,
) -> Response {
    serve_local_media_inner(runtime, addr, headers, id, query, false, true).await
}

async fn serve_local_media_inner(
    runtime: Arc<LocalMediaRuntime>,
    addr: SocketAddr,
    headers: HeaderMap,
    id: String,
    query: TokenQuery,
    is_head: bool,
    is_options: bool,
) -> Response {
    if !addr.ip().to_canonical().is_loopback() {
        return StatusCode::FORBIDDEN.into_response();
    }

    let host_ok = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .is_some_and(|h| h.eq_ignore_ascii_case(&runtime.expected_host));
    if !host_ok {
        return StatusCode::FORBIDDEN.into_response();
    }

    let allowed_origin = match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(raw_origin) => match crate::resolve_allowed_origin(raw_origin) {
            Some(matched) => Some(matched),
            None => return StatusCode::FORBIDDEN.into_response(),
        },
        None => None,
    };

    if is_options {
        let mut builder = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header(header::VARY, "Origin")
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Range")
            .header(header::CACHE_CONTROL, "no-store");
        if let Some(origin) = allowed_origin {
            builder = builder.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        }
        return builder
            .body(Body::empty())
            .unwrap_or_else(|_| StatusCode::NO_CONTENT.into_response());
    }

    if runtime.stopped.load(Ordering::SeqCst) || !runtime.app_state.is_open() {
        return status_response(StatusCode::FORBIDDEN, allowed_origin);
    }

    let token_ok = query
        .t
        .as_deref()
        .is_some_and(|t| ct_eq(t.as_bytes(), runtime.token.as_bytes()));
    if !token_ok {
        return status_response(StatusCode::FORBIDDEN, allowed_origin);
    }

    if id.is_empty() {
        return status_response(StatusCode::NOT_FOUND, allowed_origin);
    }

    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match build_http_media_response(runtime.app_state.clone(), id, is_head, range_header).await {
        Ok(mut resp) => {
            let resp_headers = resp.headers_mut();
            resp_headers.insert(header::VARY, HeaderValue::from_static("Origin"));
            resp_headers.insert(
                HeaderName::from_static("access-control-expose-headers"),
                HeaderValue::from_static("Content-Range, Accept-Ranges, Content-Length"),
            );
            if let Some(origin) = allowed_origin {
                resp_headers.insert(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_static(origin),
                );
            }
            resp
        }
        Err(err) => map_vault_error_response(&err, allowed_origin),
    }
}

pub async fn start_local_media_server(
    app_state: AppState,
) -> Result<LocalMediaServerHandle, VaultError> {
    let token = Arc::new(Zeroizing::new(generate_random_token_hex(32)?));
    let bind_addr = SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), 0));
    let listener = TcpListener::bind(bind_addr).await?;
    let port = listener.local_addr()?.port();
    let base_url = format!("http://127.0.0.1:{port}");
    let expected_host = format!("127.0.0.1:{port}");
    let stopped = Arc::new(AtomicBool::new(false));
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let runtime = Arc::new(LocalMediaRuntime {
        app_state,
        expected_host,
        token: Arc::clone(&token),
        stopped: Arc::clone(&stopped),
    });

    let router = Router::new()
        .route(
            "/media/{id}",
            get(handle_get_local_media)
                .head(handle_head_local_media)
                .options(handle_options_local_media),
        )
        .with_state(runtime);

    tokio::spawn(async move {
        let make_svc = router.into_make_service_with_connect_info::<SocketAddr>();
        let _ = axum::serve(listener, make_svc)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    Ok(LocalMediaServerHandle {
        base_url,
        token,
        shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        stopped,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;

    use tempfile::NamedTempFile;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;
    use crate::db::{ImportOutcome, VaultDb};

    struct RawResp {
        status: u16,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    async fn send_raw(
        port: u16,
        method: &str,
        path: &str,
        host_header: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> RawResp {
        let mut stream = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)))
            .await
            .expect("connect to local media server");
        let host = host_header
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("127.0.0.1:{port}"));
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        for (k, v) in extra_headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let split_pos = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("valid HTTP response");
        let head_str = String::from_utf8_lossy(&raw[..split_pos]);
        let body = raw[split_pos + 4..].to_vec();

        let mut lines = head_str.split("\r\n");
        let status_line = lines.next().unwrap();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        RawResp {
            status,
            headers,
            body,
        }
    }

    #[tokio::test]
    async fn test_local_media_server_auth_headers_and_streaming() {
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("local_media_test.vault");
        let mut db = VaultDb::create(&vault_path, "secret-pass", true).unwrap();

        // 1. Non-chunked MP4-like payload
        let mut single_bytes = vec![
            0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2', 0x00, 0x00,
            0x00, 0x00, b'm', b'p', b'4', b'2', b'i', b's', b'o', b'm',
        ];
        single_bytes.extend((0..512usize).map(|i| (i % 251) as u8));
        let single_id = match db
            .import_media_bytes(&single_bytes, Some("single"), None, 1000)
            .unwrap()
        {
            ImportOutcome::Imported { id, .. } => id,
            _ => panic!("expected imported"),
        };

        // 2. Multi-chunk WebM-like payload (4096-byte chunks)
        let mut chunked_file = NamedTempFile::new().unwrap();
        let mut chunked_bytes = vec![
            0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81, 0x01, 0x42, 0x82, 0x84, b'w', b'e',
            b'b', b'm',
        ];
        chunked_bytes.extend((0..12_000usize).map(|i| (i % 247) as u8));
        chunked_file.write_all(&chunked_bytes).unwrap();
        chunked_file.flush().unwrap();

        let chunked_id = match db
            .import_media_file_with_chunk_params(
                chunked_file.path(),
                Some("chunked"),
                None,
                5000,
                100,
                4096,
            )
            .unwrap()
        {
            ImportOutcome::Imported { id, .. } => id,
            _ => panic!("expected chunked imported"),
        };

        let state = AppState::default();
        {
            let mut guard = state.lock_db().unwrap();
            *guard = Some(db);
        }

        let handle = start_local_media_server(state.clone()).await.unwrap();
        let port: u16 = handle
            .base_url()
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let token = handle.token().to_string();

        // Missing or wrong token -> 403
        assert_eq!(
            send_raw(port, "GET", &format!("/media/{single_id}"), None, &[])
                .await
                .status,
            403
        );
        assert_eq!(
            send_raw(
                port,
                "GET",
                &format!("/media/{single_id}?t=wrongtoken"),
                None,
                &[],
            )
            .await
            .status,
            403
        );

        // Bad Host header -> 403
        assert_eq!(
            send_raw(
                port,
                "GET",
                &format!("/media/{single_id}?t={token}"),
                Some("evil.example:80"),
                &[],
            )
            .await
            .status,
            403
        );

        // Bad Origin header -> 403
        assert_eq!(
            send_raw(
                port,
                "GET",
                &format!("/media/{single_id}?t={token}"),
                None,
                &[("Origin", "https://evil.example")],
            )
            .await
            .status,
            403
        );

        // Valid OPTIONS preflight -> 204
        let opt = send_raw(
            port,
            "OPTIONS",
            &format!("/media/{single_id}?t={token}"),
            None,
            &[("Origin", "tauri://localhost")],
        )
        .await;
        assert_eq!(opt.status, 204);
        assert_eq!(
            opt.headers
                .get("access-control-allow-origin")
                .map(String::as_str),
            Some("tauri://localhost")
        );

        // HEAD on non-chunked -> 200 empty body
        let s_head = send_raw(
            port,
            "HEAD",
            &format!("/media/{single_id}?t={token}"),
            None,
            &[("Origin", "tauri://localhost")],
        )
        .await;
        assert_eq!(s_head.status, 200);
        assert_eq!(
            s_head.headers.get("content-length").map(String::as_str),
            Some(single_bytes.len().to_string().as_str())
        );
        assert!(s_head.body.is_empty());

        // Full GET without Range on non-chunked -> 200 (not 206)
        let s_full = send_raw(
            port,
            "GET",
            &format!("/media/{single_id}?t={token}"),
            None,
            &[("Origin", "tauri://localhost")],
        )
        .await;
        assert_eq!(s_full.status, 200);
        assert_eq!(
            s_full.headers.get("content-type").map(String::as_str),
            Some("video/mp4")
        );
        assert_eq!(
            s_full
                .headers
                .get("access-control-allow-origin")
                .map(String::as_str),
            Some("tauri://localhost")
        );
        assert_eq!(s_full.body, single_bytes);

        // Full GET without Range on multi-chunk -> 200 (not 206)
        let c_full = send_raw(
            port,
            "GET",
            &format!("/media/{chunked_id}?t={token}"),
            None,
            &[],
        )
        .await;
        assert_eq!(c_full.status, 200);
        assert_eq!(
            c_full.headers.get("content-type").map(String::as_str),
            Some("video/webm")
        );
        assert_eq!(
            c_full.headers.get("content-length").map(String::as_str),
            Some(chunked_bytes.len().to_string().as_str())
        );
        assert_eq!(c_full.body, chunked_bytes);

        // HEAD on multi-chunk -> 200 empty body
        let c_head = send_raw(
            port,
            "HEAD",
            &format!("/media/{chunked_id}?t={token}"),
            None,
            &[],
        )
        .await;
        assert_eq!(c_head.status, 200);
        assert!(c_head.body.is_empty());

        // Explicit Range: bytes=0-15 -> 206
        let r_start = send_raw(
            port,
            "GET",
            &format!("/media/{chunked_id}?t={token}"),
            None,
            &[("Range", "bytes=0-15")],
        )
        .await;
        assert_eq!(r_start.status, 206);
        assert_eq!(
            r_start.headers.get("content-range").map(String::as_str),
            Some(format!("bytes 0-15/{}", chunked_bytes.len()).as_str())
        );
        assert_eq!(r_start.body, chunked_bytes[0..=15]);

        // Cross-chunk Range: bytes=4090-4110 -> 206
        let r_cross = send_raw(
            port,
            "GET",
            &format!("/media/{chunked_id}?t={token}"),
            None,
            &[("Range", "bytes=4090-4110")],
        )
        .await;
        assert_eq!(r_cross.status, 206);
        assert_eq!(r_cross.body, chunked_bytes[4090..=4110]);

        // Open-ended Range: bytes=4090- -> 206 through EOF
        let r_open = send_raw(
            port,
            "GET",
            &format!("/media/{chunked_id}?t={token}"),
            None,
            &[("Range", "bytes=4090-")],
        )
        .await;
        assert_eq!(r_open.status, 206);
        assert_eq!(r_open.body, chunked_bytes[4090..]);

        // Suffix Range: bytes=-20 -> 206 last 20 bytes
        let r_suffix = send_raw(
            port,
            "GET",
            &format!("/media/{chunked_id}?t={token}"),
            None,
            &[("Range", "bytes=-20")],
        )
        .await;
        assert_eq!(r_suffix.status, 206);
        assert_eq!(r_suffix.body, chunked_bytes[chunked_bytes.len() - 20..]);

        // Out-of-bounds Range -> 416
        let r_oob = send_raw(
            port,
            "GET",
            &format!("/media/{chunked_id}?t={token}"),
            None,
            &[("Range", "bytes=999999-")],
        )
        .await;
        assert_eq!(r_oob.status, 416);
        assert_eq!(
            r_oob.headers.get("content-range").map(String::as_str),
            Some(format!("bytes */{}", chunked_bytes.len()).as_str())
        );

        // After lock_vault() -> 403
        state.lock_vault();
        let after_lock = send_raw(
            port,
            "GET",
            &format!("/media/{single_id}?t={token}"),
            None,
            &[],
        )
        .await;
        assert_eq!(after_lock.status, 403);

        handle.stop();
    }
}
