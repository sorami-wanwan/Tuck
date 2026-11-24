use super::middleware::{check_lan_peer_addr, is_allowed_host_header};
use super::*;
use crate::db::{ImportOutcome, VaultDb};
use crate::models::UrlImportSettings;
use axum::http::StatusCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[test]
fn test_is_private_lan_ip_and_tailscale_rules() {
    assert!(is_private_lan_ip("127.0.0.1".parse().unwrap()));
    assert!(is_private_lan_ip("10.0.0.1".parse().unwrap()));
    assert!(is_private_lan_ip("172.16.0.1".parse().unwrap()));
    assert!(is_private_lan_ip("172.31.255.254".parse().unwrap()));
    assert!(is_private_lan_ip("192.168.1.50".parse().unwrap()));
    assert!(is_private_lan_ip("169.254.1.1".parse().unwrap()));
    assert!(is_private_lan_ip("::1".parse().unwrap()));
    assert!(is_private_lan_ip("fd00::1".parse().unwrap()));
    assert!(is_private_lan_ip("fe80::1".parse().unwrap()));
    assert!(is_private_lan_ip("::ffff:192.168.1.5".parse().unwrap()));

    assert!(!is_private_lan_ip("8.8.8.8".parse().unwrap()));
    assert!(!is_private_lan_ip("1.1.1.1".parse().unwrap()));
    assert!(!is_private_lan_ip("100.64.0.1".parse().unwrap()));
    assert!(!is_private_lan_ip("172.32.0.1".parse().unwrap()));
    assert!(!is_private_lan_ip("203.0.113.1".parse().unwrap()));
    assert!(!is_private_lan_ip("2001:db8::1".parse().unwrap()));
    assert!(!is_private_lan_ip("::ffff:8.8.8.8".parse().unwrap()));

    // Tailscale CGNAT (100.64.0.0/10 = 100.64.0.0 - 100.127.255.255)
    assert!(is_tailscale_ipv4("100.64.0.1".parse().unwrap()));
    assert!(is_tailscale_ipv4("100.100.100.100".parse().unwrap()));
    assert!(is_tailscale_ipv4("100.127.255.254".parse().unwrap()));
    assert!(!is_tailscale_ipv4("100.63.255.255".parse().unwrap()));
    assert!(!is_tailscale_ipv4("100.128.0.1".parse().unwrap()));

    let ts_peer: IpAddr = "100.101.20.30".parse().unwrap();
    assert!(!is_allowed_lan_peer_ip(ts_peer, false));
    assert!(is_allowed_lan_peer_ip(ts_peer, true));
    assert!(!is_allowed_lan_peer_ip("8.8.8.8".parse().unwrap(), true));

    // DNS Rebinding Host header rules
    let loop_peer: IpAddr = "127.0.0.1".parse().unwrap();
    let lan_peer: IpAddr = "192.168.1.25".parse().unwrap();
    assert!(is_allowed_host_header(
        Some("192.168.1.10:8080"),
        "192.168.1.10",
        8080,
        lan_peer,
        false
    ));
    assert!(!is_allowed_host_header(
        Some("evil.example:8080"),
        "192.168.1.10",
        8080,
        lan_peer,
        false
    ));
    assert!(!is_allowed_host_header(
        Some("my-mac.tailnet.ts.net:8080"),
        "192.168.1.10",
        8080,
        lan_peer,
        false
    ));
    assert!(is_allowed_host_header(
        Some("my-mac.tailnet.ts.net:8080"),
        "100.80.1.2",
        8080,
        ts_peer,
        true
    ));
    assert!(!is_allowed_host_header(
        Some("evil.com@my-mac.tailnet.ts.net:8080"),
        "100.80.1.2",
        8080,
        ts_peer,
        true
    ));
    assert!(is_allowed_host_header(
        Some("localhost:8080"),
        "192.168.1.10",
        8080,
        loop_peer,
        false
    ));
    assert!(!is_allowed_host_header(
        Some("localhost:8080"),
        "192.168.1.10",
        8080,
        lan_peer,
        false
    ));
}

#[test]
fn test_ephemeral_upload_file_permissions_and_drop() {
    let mut up = EphemeralUploadFile::create().unwrap();
    let path = up.path().to_path_buf();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    up.write_all(b"sensitive upload payload").unwrap();
    up.finish_writing().unwrap();
    assert!(path.exists());
    drop(up);
    assert!(!path.exists());
}

struct RawHttpResp {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn send_raw_http_with_host(
    port: u16,
    method: &str,
    path: &str,
    host_header: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> RawHttpResp {
    let mut stream = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .expect("connect to test lan server");
    let host_line = match host_header {
        Some(h) => format!("Host: {h}\r\n"),
        None => format!("Host: 127.0.0.1:{port}\r\n"),
    };
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\n{host_line}Connection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    if !body.is_empty() {
        stream.write_all(body).await.unwrap();
    }
    stream.flush().await.unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let split_pos = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| {
            panic!(
                "expected valid HTTP response for {method} {path}, got {} bytes: {:?}",
                raw.len(),
                String::from_utf8_lossy(&raw)
            )
        });
    let head_str = String::from_utf8_lossy(&raw[..split_pos]);
    let body_bytes = raw[split_pos + 4..].to_vec();

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
    RawHttpResp {
        status,
        headers,
        body: body_bytes,
    }
}

async fn send_raw_http(
    port: u16,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> RawHttpResp {
    send_raw_http_with_host(port, method, path, None, extra_headers, body).await
}

fn minimal_png_bytes() -> Vec<u8> {
    let mut img = image::RgbaImage::new(2, 2);
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn sample_mp4_bytes() -> Vec<u8> {
    let mut b = vec![
        0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2', 0x00, 0x00, 0x00,
        0x00, b'm', b'p', b'4', b'2', b'i', b's', b'o', b'm',
    ];
    b.extend_from_slice(&[0x5A; 128]);
    b
}

#[tokio::test]
async fn test_lan_server_auth_security_rbac_streaming_and_lock() {
    let dir = tempfile::tempdir().unwrap();
    let vault_path = dir.path().join("lan_test.vault");
    let mut db = VaultDb::create(&vault_path, "vault-pass-123", true).unwrap();
    db.set_url_import_settings(&UrlImportSettings {
        enabled: true,
        ytdlp_path: "/opt/homebrew/bin/yt-dlp".to_string(),
        ffmpeg_path: "/opt/homebrew/bin/ffmpeg".to_string(),
        cookie_browser: "arc".to_string(),
        cookies_file: "/Users/secret/cookies.txt".to_string(),
        max_video_size_mb: 1024,
        max_video_height: 1080,
    })
    .unwrap();

    let png1 = minimal_png_bytes();
    let item_id = match db.import_bytes(&png1, Some("sample_img")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected imported image"),
    };

    let mp4 = sample_mp4_bytes();
    let video_id = match db
        .import_media_bytes(&mp4, Some("sample_vid"), None, 1200)
        .unwrap()
    {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected imported video"),
    };

    let state = AppState::default();
    {
        let mut g = state.lock_db().unwrap();
        *g = Some(db);
    }

    // Write dummy dist index.html
    let dist_dir = dir.path().join("dist");
    fs::create_dir_all(&dist_dir).unwrap();
    fs::write(
        dist_dir.join("index.html"),
        "<html><body>Tuck</body></html>",
    )
    .unwrap();

    let (handle, status) = start_lan_server_on_addr(
        state.clone(),
        StartLanShareRequest {
            mode: LanAccessMode::ReadOnly,
            duration_minutes: 30,
            bind_ip: None,
            pause_idle_lock: true,
        },
        SocketAddr::from(([127, 0, 0, 1], 0)),
        "127.0.0.1".to_string(),
        vec![],
        AssetSource::Dir(dist_dir),
        LanHostNotifier::default(),
    )
    .await
    .unwrap();
    {
        let mut g = state.lan_server.lock().unwrap();
        *g = Some(handle.clone());
    }

    let port = status.port;
    assert_eq!(status.pin.len(), 6);
    assert!(status.qr_svg.contains("<svg"));

    // 0. Peer IP filter & DNS Rebinding Host filter + CSP
    assert_eq!(
        check_lan_peer_addr(SocketAddr::from(([8, 8, 8, 8], 50000)), false),
        Err(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        check_lan_peer_addr(SocketAddr::from(([192, 168, 1, 20], 50000)), false),
        Ok(())
    );
    assert_eq!(
        check_lan_peer_addr(SocketAddr::from(([100, 100, 1, 2], 50000)), false),
        Err(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        check_lan_peer_addr(SocketAddr::from(([100, 100, 1, 2], 50000)), true),
        Ok(())
    );

    let rebind_resp = send_raw_http_with_host(
        port,
        "GET",
        "/api/auth/status",
        Some(&format!("attacker.example:{port}")),
        &[],
        b"",
    )
    .await;
    assert_eq!(rebind_resp.status, 403);
    assert!(rebind_resp.headers.contains_key("content-security-policy"));

    let index_res = send_raw_http(port, "GET", "/", &[], b"").await;
    assert_eq!(index_res.status, 200);
    let csp = index_res.headers.get("content-security-policy").unwrap();
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("script-src 'self'"));

    // 1. Unauthenticated access must fail with 401
    let unauth_status = send_raw_http(port, "GET", "/api/vault/status", &[], b"").await;
    assert_eq!(unauth_status.status, 401);
    let unauth_media =
        send_raw_http(port, "GET", &format!("/api/media/{video_id}"), &[], b"").await;
    assert_eq!(unauth_media.status, 401);

    let query_body = br#"{"scope":{"kind":"all"}}"#;
    let unauth_query = send_raw_http(
        port,
        "POST",
        "/api/entries/query",
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        query_body,
    )
    .await;
    assert_eq!(unauth_query.status, 401);

    // 2. CSRF header missing -> 403
    let no_csrf = send_raw_http(
        port,
        "POST",
        "/api/auth/pin",
        &[("Content-Type", "application/json")],
        br#"{"pin":"000000"}"#,
    )
    .await;
    assert_eq!(no_csrf.status, 403);

    // 3. Wrong PIN 5 times -> 429 Too Many Requests
    let wrong_pin = if status.pin == "000000" {
        "111111"
    } else {
        "000000"
    };
    let wrong_body = format!(r#"{{"pin":"{wrong_pin}"}}"#);
    for i in 0..5 {
        let resp = send_raw_http(
            port,
            "POST",
            "/api/auth/pin",
            &[
                ("Content-Type", "application/json"),
                ("X-Tuck-Client", "web"),
            ],
            wrong_body.as_bytes(),
        )
        .await;
        if i < 4 {
            assert_eq!(resp.status, 401);
        } else {
            assert_eq!(resp.status, 429);
        }
    }

    // Regenerate credentials clears rate limiter and rotates PIN/token
    let rotated = handle.regenerate_credentials(false).unwrap();
    assert_ne!(rotated.qr_url, status.qr_url);

    // 4. Valid PIN login -> 200 + HttpOnly Cookie + mediaToken
    let good_pin_body = format!(r#"{{"pin":"{}"}}"#, rotated.pin);
    let pin_ok = send_raw_http(
        port,
        "POST",
        "/api/auth/pin",
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        good_pin_body.as_bytes(),
    )
    .await;
    assert_eq!(pin_ok.status, 200);
    let set_cookie = pin_ok.headers.get("set-cookie").unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    let cookie_pair = set_cookie.split(';').next().unwrap().to_string();

    // 5. Authenticated /api/vault/status -> no-store + sanitized host paths
    let v_status_res = send_raw_http(
        port,
        "GET",
        "/api/vault/status",
        &[("Cookie", &cookie_pair)],
        b"",
    )
    .await;
    assert_eq!(v_status_res.status, 200);
    assert!(v_status_res
        .headers
        .get("cache-control")
        .unwrap()
        .contains("no-store"));
    let v_status_json: serde_json::Value = serde_json::from_slice(&v_status_res.body).unwrap();
    assert_eq!(v_status_json["urlImportSettings"]["ytdlpPath"], "");
    assert_eq!(v_status_json["urlImportSettings"]["ffmpegPath"], "");
    assert_eq!(v_status_json["urlImportSettings"]["cookiesFile"], "");
    assert_eq!(v_status_json["urlImportSettings"]["cookieBrowser"], "");

    // 6. QR token login -> cookie + mediaToken
    let qr_token = rotated.qr_url.split("?k=").nth(1).unwrap();
    let qr_body = format!(r#"{{"token":"{qr_token}"}}"#);
    let qr_ok = send_raw_http(
        port,
        "POST",
        "/api/auth/token",
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        qr_body.as_bytes(),
    )
    .await;
    assert_eq!(qr_ok.status, 200);
    let auth_json: serde_json::Value = serde_json::from_slice(&qr_ok.body).unwrap();
    let media_token = auth_json["mediaToken"].as_str().unwrap().to_string();

    // 7. mediaToken works ONLY on /api/media/{id} (Range 206), NOT on other endpoints
    let media_range = send_raw_http(
        port,
        "GET",
        &format!("/api/media/{video_id}?mt={media_token}"),
        &[("Range", "bytes=0-15")],
        b"",
    )
    .await;
    assert_eq!(media_range.status, 206);
    assert_eq!(media_range.body, mp4[0..=15]);

    let mt_on_status = send_raw_http(
        port,
        "GET",
        &format!("/api/vault/status?mt={media_token}"),
        &[],
        b"",
    )
    .await;
    assert_eq!(mt_on_status.status, 401);

    // 8. ReadOnly blocks writes with 403 LAN_READ_ONLY
    let ro_title = send_raw_http(
        port,
        "PATCH",
        &format!("/api/items/{item_id}/title"),
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
            ("Cookie", &cookie_pair),
        ],
        br#"{"title":"hacked"}"#,
    )
    .await;
    assert_eq!(ro_title.status, 403);
    let ro_err: serde_json::Value = serde_json::from_slice(&ro_title.body).unwrap();
    assert_eq!(ro_err["code"], "LAN_READ_ONLY");

    // 9. Switch live to ReadWrite -> mutation & upload succeed
    handle.set_mode(LanAccessMode::ReadWrite);
    let rw_title = send_raw_http(
        port,
        "PATCH",
        &format!("/api/items/{item_id}/title"),
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
            ("Cookie", &cookie_pair),
        ],
        br#"{"title":"updated_from_lan"}"#,
    )
    .await;
    assert_eq!(rw_title.status, 200);
    let updated_item: serde_json::Value = serde_json::from_slice(&rw_title.body).unwrap();
    assert_eq!(updated_item["title"], "updated_from_lan");

    // Upload a second unique image via /api/import/bytes
    let mut img2 = image::RgbaImage::new(3, 3);
    img2.put_pixel(1, 1, image::Rgba([0, 255, 128, 255]));
    let mut png2 = Vec::new();
    image::DynamicImage::ImageRgba8(img2)
        .write_to(
            &mut std::io::Cursor::new(&mut png2),
            image::ImageFormat::Png,
        )
        .unwrap();
    let upload_res = send_raw_http(
        port,
        "POST",
        "/api/import/bytes",
        &[
            ("X-Tuck-Client", "web"),
            ("X-Title", "mobile_upload"),
            ("Cookie", &cookie_pair),
        ],
        &png2,
    )
    .await;
    assert_eq!(upload_res.status, 200);
    let upload_json: serde_json::Value = serde_json::from_slice(&upload_res.body).unwrap();
    assert_eq!(upload_json["importedIds"].as_array().unwrap().len(), 1);

    // 10. Host Instant Lock stops LAN server immediately and rejects further connections
    state.lock_vault();
    assert!(!state.current_lan_status().running);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .is_err());
}

#[tokio::test]
async fn test_lan_server_auto_expire_timer() {
    let dir = tempfile::tempdir().unwrap();
    let vault_path = dir.path().join("lan_timer.vault");
    let db = VaultDb::create(&vault_path, "timer-pass", true).unwrap();
    let state = AppState::default();
    {
        let mut g = state.lock_db().unwrap();
        *g = Some(db);
    }

    let (handle, status) = start_lan_server_on_addr(
        state.clone(),
        StartLanShareRequest {
            mode: LanAccessMode::ReadOnly,
            duration_minutes: 10,
            bind_ip: None,
            pause_idle_lock: true,
        },
        SocketAddr::from(([127, 0, 0, 1], 0)),
        "127.0.0.1".to_string(),
        vec![],
        AssetSource::None,
        LanHostNotifier::default(),
    )
    .await
    .unwrap();
    {
        let mut g = state.lan_server.lock().unwrap();
        *g = Some(handle.clone());
    }
    assert!(status.running);

    // Set deadline to 40ms from now and wake the timer task
    {
        let mut g = handle.runtime.store.write().unwrap();
        g.expires_at_ms = Some(now_ms() + 40);
    }
    handle.runtime.timer_notify.notify_waiters();

    tokio::time::sleep(Duration::from_millis(160)).await;
    assert!(!state.current_lan_status().running);
    state.lock_vault();
}

#[tokio::test]
async fn test_lan_server_plain_export_and_zip_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let vault_path = dir.path().join("lan_export.vault");
    let mut db = VaultDb::create(&vault_path, "lan-export-pass", false).unwrap();
    db.set_skip_plain_export_confirm(true).unwrap();

    let mut img = image::RgbaImage::new(6, 6);
    img.put_pixel(0, 0, image::Rgba([20, 140, 250, 255]));
    let mut png1 = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut png1),
            image::ImageFormat::Png,
        )
        .unwrap();

    let mut img2 = image::RgbaImage::new(5, 5);
    img2.put_pixel(1, 1, image::Rgba([250, 80, 40, 255]));
    let mut png2 = Vec::new();
    image::DynamicImage::ImageRgba8(img2)
        .write_to(
            &mut std::io::Cursor::new(&mut png2),
            image::ImageFormat::Png,
        )
        .unwrap();

    let mut img3 = image::RgbaImage::new(4, 4);
    img3.put_pixel(2, 2, image::Rgba([80, 220, 90, 255]));
    let mut png3 = Vec::new();
    image::DynamicImage::ImageRgba8(img3)
        .write_to(
            &mut std::io::Cursor::new(&mut png3),
            image::ImageFormat::Png,
        )
        .unwrap();

    let id1 = match db.import_bytes(&png1, Some("イラストA")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected import 1"),
    };
    let id2 = match db.import_bytes(&png2, Some("ページ1")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected import 2"),
    };
    let id3 = match db.import_bytes(&png3, Some("ページ2")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected import 3"),
    };
    let group = db
        .create_group_from_items(&[id2, id3], Some("テスト漫画"), None)
        .unwrap();
    let root_folder = db.create_folder(None, "親フォルダ", "").unwrap();
    let child_folder = db
        .create_folder(Some(&root_folder.id), "子フォルダ", "")
        .unwrap();
    db.add_entries_to_folder(
        &root_folder.id,
        &[crate::models::EntryRef {
            kind: crate::models::EntryKind::Image,
            id: id1.clone(),
        }],
    )
    .unwrap();
    db.add_entries_to_folder(
        &child_folder.id,
        &[crate::models::EntryRef {
            kind: crate::models::EntryKind::Group,
            id: group.id.clone(),
        }],
    )
    .unwrap();

    let state = AppState::default();
    {
        let mut g = state.lock_db().unwrap();
        *g = Some(db);
    }

    let (handle, status) = start_lan_server_on_addr(
        state.clone(),
        StartLanShareRequest {
            mode: LanAccessMode::ReadOnly,
            duration_minutes: 30,
            bind_ip: None,
            pause_idle_lock: true,
        },
        SocketAddr::from(([127, 0, 0, 1], 0)),
        "127.0.0.1".to_string(),
        vec![],
        AssetSource::None,
        LanHostNotifier::default(),
    )
    .await
    .unwrap();
    {
        let mut g = state.lan_server.lock().unwrap();
        *g = Some(handle.clone());
    }
    let port = status.port;

    // 1. Unauthenticated must fail with 401
    let unauth = send_raw_http(port, "GET", &format!("/api/export/item/{id1}"), &[], b"").await;
    assert_eq!(unauth.status, 401);

    // 2. Authenticate session A via PIN (readOnly mode)
    let pin_body = format!(r#"{{"pin":"{}"}}"#, status.pin);
    let pin_res = send_raw_http(
        port,
        "POST",
        "/api/auth/pin",
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        pin_body.as_bytes(),
    )
    .await;
    assert_eq!(pin_res.status, 200);
    let cookie_a = pin_res
        .headers
        .get("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Host skipPlainExportConfirm must be sanitized to false for web status
    let st_res = send_raw_http(
        port,
        "GET",
        "/api/vault/status",
        &[("Cookie", &cookie_a)],
        b"",
    )
    .await;
    let st_json: serde_json::Value = serde_json::from_slice(&st_res.body).unwrap();
    assert_eq!(st_json["skipPlainExportConfirm"], false);

    // 3. HEAD & GET single item succeed even in readOnly mode
    let head_item = send_raw_http(
        port,
        "HEAD",
        &format!("/api/export/item/{id1}"),
        &[("Cookie", &cookie_a)],
        b"",
    )
    .await;
    assert_eq!(head_item.status, 200);
    assert_eq!(
        head_item.headers.get("content-length").unwrap(),
        &png1.len().to_string()
    );
    assert!(head_item
        .headers
        .get("content-disposition")
        .unwrap()
        .starts_with("attachment;"));

    let get_item = send_raw_http(
        port,
        "GET",
        &format!("/api/export/item/{id1}"),
        &[("Cookie", &cookie_a)],
        b"",
    )
    .await;
    assert_eq!(get_item.status, 200);
    assert_eq!(get_item.body, png1);

    // 4. Manga group & folder ZIP exports return application/zip with valid PK headers
    let get_group = send_raw_http(
        port,
        "GET",
        &format!("/api/export/group/{}", group.id),
        &[("Cookie", &cookie_a)],
        b"",
    )
    .await;
    assert_eq!(get_group.status, 200);
    assert_eq!(
        get_group.headers.get("content-type").unwrap(),
        "application/zip"
    );
    assert!(get_group.body.starts_with(b"PK\x03\x04"));
    assert!(get_group.body.windows(4).any(|w| w == b"PK\x05\x06"));

    let folder_nosub = send_raw_http(
        port,
        "GET",
        &format!(
            "/api/export/folder/{}?includeSubfolders=false",
            root_folder.id
        ),
        &[("Cookie", &cookie_a)],
        b"",
    )
    .await;
    let folder_sub = send_raw_http(
        port,
        "GET",
        &format!(
            "/api/export/folder/{}?includeSubfolders=true",
            root_folder.id
        ),
        &[("Cookie", &cookie_a)],
        b"",
    )
    .await;
    assert_eq!(folder_nosub.status, 200);
    assert_eq!(folder_sub.status, 200);
    assert!(folder_sub.body.len() > folder_nosub.body.len());

    // 5. Batch ticket requires CSRF, binds to session, and is single-use
    let batch_req = format!(
        r#"{{"entries":[{{"kind":"image","id":"{id1}"}},{{"kind":"group","id":"{}"}}],"folderIds":[],"includeSubfolders":true}}"#,
        group.id
    );
    let no_csrf = send_raw_http(
        port,
        "POST",
        "/api/export/batch-ticket",
        &[("Content-Type", "application/json"), ("Cookie", &cookie_a)],
        batch_req.as_bytes(),
    )
    .await;
    assert_eq!(no_csrf.status, 403);

    let ticket_res = send_raw_http(
        port,
        "POST",
        "/api/export/batch-ticket",
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
            ("Cookie", &cookie_a),
        ],
        batch_req.as_bytes(),
    )
    .await;
    assert_eq!(ticket_res.status, 200);
    let ticket_json: serde_json::Value = serde_json::from_slice(&ticket_res.body).unwrap();
    let ticket = ticket_json["ticket"].as_str().unwrap().to_string();
    assert_eq!(ticket_json["fileCount"], 3);

    // Second session cannot redeem session A's ticket
    let pin_res_b = send_raw_http(
        port,
        "POST",
        "/api/auth/pin",
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        pin_body.as_bytes(),
    )
    .await;
    let cookie_b = pin_res_b
        .headers
        .get("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let cross_res = send_raw_http(
        port,
        "GET",
        &format!("/api/export/batch/{ticket}"),
        &[("Cookie", &cookie_b)],
        b"",
    )
    .await;
    assert_eq!(cross_res.status, 404);

    // Session A redeems once -> 200 ZIP, second attempt -> 404
    let redeem1 = send_raw_http(
        port,
        "GET",
        &format!("/api/export/batch/{ticket}"),
        &[("Cookie", &cookie_a)],
        b"",
    )
    .await;
    assert_eq!(redeem1.status, 200);
    assert!(redeem1.body.starts_with(b"PK\x03\x04"));
    let redeem2 = send_raw_http(
        port,
        "GET",
        &format!("/api/export/batch/{ticket}"),
        &[("Cookie", &cookie_a)],
        b"",
    )
    .await;
    assert_eq!(redeem2.status, 404);

    // 6. Locked DB returns 403 VAULT_NOT_OPEN
    {
        let mut g = state.lock_db().unwrap();
        *g = None;
    }
    let locked_res = send_raw_http(
        port,
        "GET",
        &format!("/api/export/item/{id1}"),
        &[("Cookie", &cookie_a)],
        b"",
    )
    .await;
    assert_eq!(locked_res.status, 403);
    handle.stop("test_done");
}

#[test]
fn test_interface_priority_prefers_wifi_across_oses() {
    let linux_wifi = interface_priority("wlp2s0", "192.168.1.20".parse().unwrap(), false);
    let linux_eth = interface_priority("eth0", "192.168.1.21".parse().unwrap(), false);
    let docker = interface_priority("docker0", "172.17.0.1".parse().unwrap(), false);
    let tailscale = interface_priority("tailscale0", "100.64.0.5".parse().unwrap(), true);
    let mac_wifi = interface_priority("en0", "192.168.1.10".parse().unwrap(), false);
    let win_wifi = interface_priority("Wi-Fi", "192.168.1.30".parse().unwrap(), false);

    assert!(linux_wifi < linux_eth);
    assert!(linux_eth < docker);
    assert!(docker < tailscale);
    assert_eq!(mac_wifi.1, 0);
    assert_eq!(linux_wifi.1, 0);
    assert_eq!(win_wifi.1, 0);
}

#[tokio::test]
async fn test_lan_server_batch_entry_tags_rbac_and_sync() {
    let dir = tempfile::tempdir().unwrap();
    let vault_path = dir.path().join("lan_batch_tags.vault");
    let mut db = VaultDb::create(&vault_path, "vault-pass-tag", true).unwrap();

    let png1 = minimal_png_bytes();
    let mut png2 = minimal_png_bytes();
    png2.push(0); // distinct sha256 while still valid PNG bytes
    let mut png3 = minimal_png_bytes();
    png3.extend_from_slice(&[1, 2]);

    let img_id = match db.import_bytes(&png1, Some("standalone")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected image"),
    };
    let p1 = match db.import_bytes(&png2, Some("p1")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected p1"),
    };
    let p2 = match db.import_bytes(&png3, Some("p2")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected p2"),
    };
    let grp = db
        .create_group_from_items(&[p1, p2], Some("comic"), None)
        .unwrap();

    let state = AppState::default();
    {
        let mut g = state.lock_db().unwrap();
        *g = Some(db);
    }

    let dist_dir = dir.path().join("dist");
    fs::create_dir_all(&dist_dir).unwrap();
    fs::write(
        dist_dir.join("index.html"),
        "<html><body>Tuck</body></html>",
    )
    .unwrap();

    let (handle, status) = start_lan_server_on_addr(
        state.clone(),
        StartLanShareRequest {
            mode: LanAccessMode::ReadOnly,
            duration_minutes: 30,
            bind_ip: None,
            pause_idle_lock: true,
        },
        SocketAddr::from(([127, 0, 0, 1], 0)),
        "127.0.0.1".to_string(),
        vec![],
        AssetSource::Dir(dist_dir),
        LanHostNotifier::default(),
    )
    .await
    .unwrap();
    {
        let mut g = state.lan_server.lock().unwrap();
        *g = Some(handle.clone());
    }

    let port = status.port;
    let payload = serde_json::json!({
        "entries": [
            { "kind": "image", "id": img_id },
            { "kind": "group", "id": grp.id }
        ],
        "addTags": ["風景", "お気に入り"],
        "removeTags": []
    })
    .to_string();

    // 1. Unauthenticated -> 401
    let unauth = send_raw_http(
        port,
        "POST",
        "/api/entries/tags/batch",
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        payload.as_bytes(),
    )
    .await;
    assert_eq!(unauth.status, 401);

    // Login with PIN
    let pin_body = format!(r#"{{"pin":"{}"}}"#, status.pin);
    let pin_ok = send_raw_http(
        port,
        "POST",
        "/api/auth/pin",
        &[
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        pin_body.as_bytes(),
    )
    .await;
    assert_eq!(pin_ok.status, 200);
    let cookie = pin_ok
        .headers
        .get("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // 2. ReadOnly mode -> 403
    let ro_res = send_raw_http(
        port,
        "POST",
        "/api/entries/tags/batch",
        &[
            ("Cookie", &cookie),
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        payload.as_bytes(),
    )
    .await;
    assert_eq!(ro_res.status, 403);

    // Switch to ReadWrite
    handle.set_mode(LanAccessMode::ReadWrite);

    // 3. Missing X-Tuck-Client header -> 403
    let no_csrf = send_raw_http(
        port,
        "POST",
        "/api/entries/tags/batch",
        &[("Cookie", &cookie), ("Content-Type", "application/json")],
        payload.as_bytes(),
    )
    .await;
    assert_eq!(no_csrf.status, 403);

    // 4. ReadWrite + Cookie + CSRF -> 200
    let ok_res = send_raw_http(
        port,
        "POST",
        "/api/entries/tags/batch",
        &[
            ("Cookie", &cookie),
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        payload.as_bytes(),
    )
    .await;
    assert_eq!(ok_res.status, 200);
    let report: serde_json::Value = serde_json::from_slice(&ok_res.body).unwrap();
    assert_eq!(report["targetCount"], 2);
    assert_eq!(report["updatedCount"], 2);

    // Verify via GET /api/tags and DB details
    let tags_res = send_raw_http(port, "GET", "/api/tags", &[("Cookie", &cookie)], b"").await;
    assert_eq!(tags_res.status, 200);
    let tags_json: Vec<serde_json::Value> = serde_json::from_slice(&tags_res.body).unwrap();
    assert_eq!(tags_json.len(), 2);
    assert!(tags_json.iter().all(|t| t["count"] == 2));

    {
        let g = state.lock_db().unwrap();
        let db_ref = g.as_ref().unwrap();
        let item_detail = db_ref.get_item_detail(&img_id).unwrap();
        let grp_detail = db_ref.get_group_detail(&grp.id).unwrap();
        assert_eq!(item_detail.tags, vec!["お気に入り", "風景"]);
        assert_eq!(grp_detail.tags, vec!["お気に入り", "風景"]);
    }

    // 5. Remove one tag and verify orphan pruning via GET /api/tags
    let remove_payload = serde_json::json!({
        "entries": [
            { "kind": "image", "id": img_id },
            { "kind": "group", "id": grp.id }
        ],
        "addTags": [],
        "removeTags": ["風景"]
    })
    .to_string();
    let rem_res = send_raw_http(
        port,
        "POST",
        "/api/entries/tags/batch",
        &[
            ("Cookie", &cookie),
            ("Content-Type", "application/json"),
            ("X-Tuck-Client", "web"),
        ],
        remove_payload.as_bytes(),
    )
    .await;
    assert_eq!(rem_res.status, 200);
    let rem_report: serde_json::Value = serde_json::from_slice(&rem_res.body).unwrap();
    assert_eq!(rem_report["targetCount"], 2);
    assert_eq!(rem_report["updatedCount"], 2);

    let tags_after = send_raw_http(port, "GET", "/api/tags", &[("Cookie", &cookie)], b"").await;
    assert_eq!(tags_after.status, 200);
    let tags_after_json: Vec<serde_json::Value> = serde_json::from_slice(&tags_after.body).unwrap();
    assert_eq!(tags_after_json.len(), 1);
    assert_eq!(tags_after_json[0]["name"], "お気に入り");

    handle.stop("test_done");
}
