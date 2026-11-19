use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::{is_allowed_lan_peer_ip, LanServerRuntime, LAN_BROWSER_CSP};

pub(super) fn check_lan_peer_addr(
    addr: SocketAddr,
    allow_tailscale: bool,
) -> Result<(), StatusCode> {
    if is_allowed_lan_peer_ip(addr.ip(), allow_tailscale) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

pub(crate) fn is_valid_tailscale_magic_dns_host(host_header: &str, expected_port: u16) -> bool {
    let Some((host_part, port_part)) = host_header.rsplit_once(':') else {
        return false;
    };
    if port_part != expected_port.to_string() {
        return false;
    }
    let lower = host_part.to_ascii_lowercase();
    if lower.len() <= ".ts.net".len()
        || !lower.ends_with(".ts.net")
        || lower.starts_with('.')
        || lower.contains("..")
    {
        return false;
    }
    lower
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

pub(crate) fn is_allowed_host_header(
    host_header: Option<&str>,
    selected_ip: &str,
    port: u16,
    peer_ip: IpAddr,
    allow_tailscale: bool,
) -> bool {
    let Some(raw) = host_header.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let expected_selected = format!("{selected_ip}:{port}");
    if raw.eq_ignore_ascii_case(&expected_selected) {
        return true;
    }
    if peer_ip.to_canonical().is_loopback() {
        let loop_hosts = [
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ];
        if loop_hosts.iter().any(|h| raw.eq_ignore_ascii_case(h)) {
            return true;
        }
    }
    allow_tailscale && is_valid_tailscale_magic_dns_host(raw, port)
}

pub(super) async fn lan_ip_and_host_middleware(
    State(runtime): State<Arc<LanServerRuntime>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Err(status) = check_lan_peer_addr(addr, runtime.allow_tailscale) {
        return status.into_response();
    }
    let host_hdr = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if !is_allowed_host_header(
        host_hdr,
        &runtime.selected_ip,
        runtime.port,
        addr.ip(),
        runtime.allow_tailscale,
    ) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

pub(super) async fn security_headers_middleware(req: Request<Body>, next: Next) -> Response {
    let is_api = req.uri().path().starts_with("/api/");
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(LAN_BROWSER_CSP),
    );
    if is_api {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-cache, must-revalidate"),
        );
        headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    }
    res
}
