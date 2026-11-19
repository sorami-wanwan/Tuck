use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use super::{AssetSource, LanServerRuntime};

pub(super) async fn handle_serve_index(State(runtime): State<Arc<LanServerRuntime>>) -> Response {
    serve_asset_path(&runtime.assets, "index.html")
}

pub(super) async fn handle_serve_static(
    State(runtime): State<Arc<LanServerRuntime>>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    if path == "api" || path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_asset_path(&runtime.assets, &path)
}

fn sanitize_rel_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim_start_matches('/');
    let rel = if trimmed.is_empty() {
        "index.html"
    } else {
        trimmed
    };
    let mut clean = PathBuf::new();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(seg) => clean.push(seg),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if clean.as_os_str().is_empty() {
        clean.push("index.html");
    }
    Some(clean)
}

fn content_type_for_path(rel: &Path) -> &'static str {
    match rel
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("json") => "application/json; charset=utf-8",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn try_read_from_dir(base_dir: &Path, clean_rel: &Path) -> Option<(Vec<u8>, &'static str)> {
    let Ok(canonical_base) = fs::canonicalize(base_dir) else {
        return None;
    };
    let candidate = canonical_base.join(clean_rel);
    if let Ok(canonical_target) = fs::canonicalize(&candidate) {
        if canonical_target.starts_with(&canonical_base) && canonical_target.is_file() {
            if let Ok(bytes) = fs::read(&canonical_target) {
                return Some((bytes, content_type_for_path(&canonical_target)));
            }
        }
    }
    if clean_rel.extension().is_none() {
        let index_candidate = canonical_base.join("index.html");
        if let Ok(canonical_index) = fs::canonicalize(&index_candidate) {
            if canonical_index.starts_with(&canonical_base) && canonical_index.is_file() {
                if let Ok(bytes) = fs::read(&canonical_index) {
                    return Some((bytes, "text/html; charset=utf-8"));
                }
            }
        }
    }
    None
}

fn serve_asset_path(assets: &AssetSource, raw_path: &str) -> Response {
    let Some(clean_rel) = sanitize_rel_path(raw_path) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let rel_str = clean_rel.to_string_lossy().replace('\\', "/");

    match assets {
        AssetSource::Tauri(app) => {
            if let Some(asset) = app.asset_resolver().get(rel_str.clone()) {
                return build_static_response(asset.bytes, &asset.mime_type);
            }
            if clean_rel.extension().is_none() {
                if let Some(asset) = app.asset_resolver().get("index.html".to_string()) {
                    return build_static_response(asset.bytes, &asset.mime_type);
                }
            }
            #[cfg(debug_assertions)]
            {
                let dev_dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist");
                if let Some((bytes, mime)) = try_read_from_dir(&dev_dist, &clean_rel) {
                    return build_static_response(bytes, mime);
                }
                if clean_rel.extension().is_none() || rel_str == "index.html" {
                    let msg = "<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\"><title>Tuck LAN</title><body style=\"background:#14171e;color:#e6eaf2;font-family:sans-serif;padding:24px\"><h2>開発モード: フロントエンド未ビルド</h2><p>開発中に LAN 公開を利用する場合は、ホスト側で <code>bun run build</code> を実行して <code>dist/</code> を生成してから再読み込みしてください。</p></body></html>";
                    return build_static_response(
                        msg.as_bytes().to_vec(),
                        "text/html; charset=utf-8",
                    );
                }
            }
            StatusCode::NOT_FOUND.into_response()
        }
        AssetSource::Dir(dir) => {
            if let Some((bytes, mime)) = try_read_from_dir(dir, &clean_rel) {
                return build_static_response(bytes, mime);
            }
            StatusCode::NOT_FOUND.into_response()
        }
        AssetSource::None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn build_static_response(bytes: Vec<u8>, mime_type: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
