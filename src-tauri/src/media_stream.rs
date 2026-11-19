use std::io;

use axum::body::Body;
use axum::http::{header as axum_header, StatusCode as AxumStatusCode};
use axum::response::Response as AxumResponse;
use bytes::Bytes;
use futures_util::stream::unfold;
use tauri::http::StatusCode;
use tokio::sync::mpsc;

use crate::error::VaultError;
use crate::state::AppState;

pub const MAX_STREAM_CHUNK_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_EXPLICIT_RANGE_SLICE_BYTES: usize = 16 * 1024 * 1024;
const INLINE_CHUNKED_THRESHOLD: usize = 64 * 1024 * 1024;
const HTTP_SUBCHUNK_BYTES: usize = 256 * 1024;

pub struct MediaStreamSlice {
    pub status: StatusCode,
    pub mime_type: String,
    pub content_range: Option<String>,
    pub content_length: usize,
    pub body: Vec<u8>,
}

fn parse_byte_range_with_cap(
    range_header: &str,
    total: usize,
    open_end_cap: Option<usize>,
) -> Option<(usize, usize)> {
    if total == 0 {
        return None;
    }
    let trimmed = range_header.trim();
    let spec = trimmed
        .strip_prefix("bytes=")
        .or_else(|| trimmed.strip_prefix("BYTES="))?;
    let first_range = spec.split(',').next()?.trim();
    let (start_str, end_str) = first_range.split_once('-')?;

    if start_str.is_empty() {
        let suffix_len: usize = end_str.trim().parse().ok()?;
        if suffix_len == 0 {
            return None;
        }
        let start = total.saturating_sub(suffix_len);
        return Some((start, total - 1));
    }

    let start: usize = start_str.trim().parse().ok()?;
    if start >= total {
        return None;
    }

    let end = if end_str.is_empty() {
        match open_end_cap {
            Some(cap) => start.saturating_add(cap.saturating_sub(1)).min(total - 1),
            None => total - 1,
        }
    } else {
        let requested_end: usize = end_str.trim().parse().ok()?;
        if requested_end < start {
            return None;
        }
        requested_end.min(total - 1)
    };

    Some((start, end))
}

pub(crate) fn parse_byte_range(range_header: &str, total: usize) -> Option<(usize, usize)> {
    parse_byte_range_with_cap(range_header, total, Some(MAX_STREAM_CHUNK_BYTES))
}

pub(crate) fn parse_http_byte_range(range_header: &str, total: usize) -> Option<(usize, usize)> {
    parse_byte_range_with_cap(range_header, total, None)
}

fn stream_media_range_to_channel(
    state: &AppState,
    item_id: &str,
    start: usize,
    end: usize,
    chunk_size: usize,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) {
    if start > end {
        return;
    }

    if chunk_size == 0 {
        let (_, bytes) = match state.get_or_load_media(item_id) {
            Ok(res) => res,
            Err(e) => {
                let _ = tx.blocking_send(Err(io::Error::other(e.to_string())));
                return;
            }
        };
        if start >= bytes.len() {
            return;
        }
        let clamped_end = end.min(bytes.len() - 1);
        for sub in bytes[start..=clamped_end].chunks(HTTP_SUBCHUNK_BYTES) {
            if !state.is_open() {
                let _ = tx.blocking_send(Err(io::Error::other("vault closed")));
                return;
            }
            if tx.blocking_send(Ok(Bytes::copy_from_slice(sub))).is_err() {
                return;
            }
        }
        return;
    }

    let first_chunk = start / chunk_size;
    let last_chunk = end / chunk_size;
    for chunk_idx in first_chunk..=last_chunk {
        if !state.is_open() {
            let _ = tx.blocking_send(Err(io::Error::other("vault closed")));
            return;
        }
        let data = match state.read_blob_chunk(item_id, chunk_idx as i64) {
            Ok(Some(d)) => d,
            Ok(None) => break,
            Err(e) => {
                let _ = tx.blocking_send(Err(io::Error::other(e.to_string())));
                return;
            }
        };
        let chunk_start = chunk_idx.saturating_mul(chunk_size);
        let local_start = start.saturating_sub(chunk_start);
        let local_end_excl = end
            .saturating_add(1)
            .saturating_sub(chunk_start)
            .min(data.len());
        if local_start < local_end_excl {
            for sub in data[local_start..local_end_excl].chunks(HTTP_SUBCHUNK_BYTES) {
                if !state.is_open() {
                    let _ = tx.blocking_send(Err(io::Error::other("vault closed")));
                    return;
                }
                if tx.blocking_send(Ok(Bytes::copy_from_slice(sub))).is_err() {
                    return;
                }
            }
        }
    }
}

pub async fn build_http_media_response(
    state: AppState,
    item_id: String,
    is_head: bool,
    range_header: Option<String>,
) -> Result<AxumResponse, VaultError> {
    let state_info = state.clone();
    let id_info = item_id.clone();
    let (mime_type, total, chunk_size) =
        tokio::task::spawn_blocking(move || state_info.get_media_info(&id_info))
            .await
            .map_err(|e| VaultError::Io(io::Error::other(e.to_string())))??;

    if total == 0 {
        if range_header.is_some() {
            return AxumResponse::builder()
                .status(AxumStatusCode::RANGE_NOT_SATISFIABLE)
                .header(axum_header::ACCEPT_RANGES, "bytes")
                .header(axum_header::CONTENT_RANGE, "bytes */0")
                .header(axum_header::CACHE_CONTROL, "no-store")
                .header(axum_header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                .body(Body::empty())
                .map_err(|e| VaultError::Io(io::Error::other(e.to_string())));
        }
        return AxumResponse::builder()
            .status(AxumStatusCode::OK)
            .header(axum_header::CONTENT_TYPE, mime_type)
            .header(axum_header::ACCEPT_RANGES, "bytes")
            .header(axum_header::CONTENT_LENGTH, "0")
            .header(axum_header::CACHE_CONTROL, "no-store")
            .header(axum_header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .body(Body::empty())
            .map_err(|e| VaultError::Io(io::Error::other(e.to_string())));
    }

    let (status, start, end, content_range) = if let Some(rh) = range_header.as_deref() {
        let Some((s, e)) = parse_http_byte_range(rh, total) else {
            return AxumResponse::builder()
                .status(AxumStatusCode::RANGE_NOT_SATISFIABLE)
                .header(axum_header::ACCEPT_RANGES, "bytes")
                .header(axum_header::CONTENT_RANGE, format!("bytes */{total}"))
                .header(axum_header::CACHE_CONTROL, "no-store")
                .header(axum_header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                .body(Body::empty())
                .map_err(|e| VaultError::Io(io::Error::other(e.to_string())));
        };
        (
            AxumStatusCode::PARTIAL_CONTENT,
            s,
            e,
            Some(format!("bytes {s}-{e}/{total}")),
        )
    } else {
        (AxumStatusCode::OK, 0, total - 1, None)
    };

    let content_length = end.saturating_sub(start).saturating_add(1);
    let mut builder = AxumResponse::builder()
        .status(status)
        .header(axum_header::CONTENT_TYPE, mime_type)
        .header(axum_header::ACCEPT_RANGES, "bytes")
        .header(axum_header::CONTENT_LENGTH, content_length.to_string())
        .header(axum_header::CACHE_CONTROL, "no-store")
        .header(axum_header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(cr) = content_range {
        builder = builder.header(axum_header::CONTENT_RANGE, cr);
    }

    if is_head {
        return builder
            .body(Body::empty())
            .map_err(|e| VaultError::Io(io::Error::other(e.to_string())));
    }

    let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(2);
    tokio::task::spawn_blocking(move || {
        stream_media_range_to_channel(&state, &item_id, start, end, chunk_size, tx);
    });
    let stream = unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    builder
        .body(Body::from_stream(stream))
        .map_err(|e| VaultError::Io(io::Error::other(e.to_string())))
}

pub fn resolve_media_slice(
    state: &AppState,
    item_id: &str,
    is_head: bool,
    range_header: Option<&str>,
) -> Result<MediaStreamSlice, VaultError> {
    let (mime_type, total, chunk_size) = state.get_media_info(item_id)?;

    if total == 0 {
        return Ok(MediaStreamSlice {
            status: StatusCode::OK,
            mime_type,
            content_range: None,
            content_length: 0,
            body: Vec::new(),
        });
    }

    if chunk_size > 0 {
        if let Some(rh) = range_header {
            let Some((start, raw_end)) = parse_byte_range(rh, total) else {
                return Ok(MediaStreamSlice {
                    status: StatusCode::RANGE_NOT_SATISFIABLE,
                    mime_type,
                    content_range: Some(format!("bytes */{total}")),
                    content_length: 0,
                    body: Vec::new(),
                });
            };
            let end = raw_end.min(start.saturating_add(MAX_EXPLICIT_RANGE_SLICE_BYTES.saturating_sub(1)));
            let len = end.saturating_sub(start).saturating_add(1);
            let body = if is_head {
                Vec::new()
            } else {
                state.read_chunked_media_range(item_id, start, end, chunk_size)?
            };
            return Ok(MediaStreamSlice {
                status: StatusCode::PARTIAL_CONTENT,
                mime_type,
                content_range: Some(format!("bytes {start}-{end}/{total}")),
                content_length: len,
                body,
            });
        }

        if is_head {
            return Ok(MediaStreamSlice {
                status: StatusCode::OK,
                mime_type,
                content_range: None,
                content_length: total,
                body: Vec::new(),
            });
        }

        if total <= INLINE_CHUNKED_THRESHOLD {
            let body = state.read_chunked_media_range(item_id, 0, total - 1, chunk_size)?;
            return Ok(MediaStreamSlice {
                status: StatusCode::OK,
                mime_type,
                content_range: None,
                content_length: total,
                body,
            });
        }

        let end = (MAX_STREAM_CHUNK_BYTES - 1).min(total - 1);
        let len = end + 1;
        let body = state.read_chunked_media_range(item_id, 0, end, chunk_size)?;
        return Ok(MediaStreamSlice {
            status: StatusCode::PARTIAL_CONTENT,
            mime_type,
            content_range: Some(format!("bytes 0-{end}/{total}")),
            content_length: len,
            body,
        });
    }

    let (cached_mime, bytes) = state.get_or_load_media(item_id)?;
    let full_len = bytes.len();

    if let Some(rh) = range_header {
        let Some((start, raw_end)) = parse_byte_range(rh, full_len) else {
            return Ok(MediaStreamSlice {
                status: StatusCode::RANGE_NOT_SATISFIABLE,
                mime_type: cached_mime,
                content_range: Some(format!("bytes */{full_len}")),
                content_length: 0,
                body: Vec::new(),
            });
        };
        let end = raw_end.min(start.saturating_add(MAX_EXPLICIT_RANGE_SLICE_BYTES.saturating_sub(1)));
        let slice = &bytes[start..=end];
        let body = if is_head { Vec::new() } else { slice.to_vec() };
        return Ok(MediaStreamSlice {
            status: StatusCode::PARTIAL_CONTENT,
            mime_type: cached_mime,
            content_range: Some(format!("bytes {start}-{end}/{full_len}")),
            content_length: slice.len(),
            body,
        });
    }

    let body = if is_head {
        Vec::new()
    } else {
        bytes.as_ref().clone()
    };
    Ok(MediaStreamSlice {
        status: StatusCode::OK,
        mime_type: cached_mime,
        content_range: None,
        content_length: full_len,
        body,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;
    use crate::db::{ImportOutcome, VaultDb};

    #[test]
    fn test_parse_byte_range() {
        let total = 10_000_000usize;
        assert_eq!(parse_byte_range("bytes=0-99", total), Some((0, 99)));
        assert_eq!(
            parse_byte_range("bytes=0-", total),
            Some((0, MAX_STREAM_CHUNK_BYTES - 1))
        );
        assert_eq!(
            parse_byte_range("bytes=9000000-", total),
            Some((9_000_000, total - 1))
        );
        assert_eq!(
            parse_byte_range("bytes=-500", total),
            Some((total - 500, total - 1))
        );
        assert_eq!(parse_byte_range("bytes=10000000-", total), None);
        assert_eq!(parse_byte_range("bytes=200-100", total), None);
        assert_eq!(parse_byte_range("bytes=-0", total), None);
        assert_eq!(parse_byte_range("bytes=0-0", 0), None);
    }

    #[test]
    fn test_parse_http_byte_range() {
        let total = 10_000_000usize;
        assert_eq!(parse_http_byte_range("bytes=0-99", total), Some((0, 99)));
        assert_eq!(
            parse_http_byte_range("bytes=0-", total),
            Some((0, total - 1))
        );
        assert_eq!(
            parse_http_byte_range("bytes=4096-", total),
            Some((4096, total - 1))
        );
        assert_eq!(
            parse_http_byte_range("bytes=-500", total),
            Some((total - 500, total - 1))
        );
        assert_eq!(parse_http_byte_range("bytes=10000000-", total), None);
        assert_eq!(parse_http_byte_range("bytes=200-100", total), None);
        assert_eq!(parse_http_byte_range("bytes=-0", total), None);
        assert_eq!(parse_http_byte_range("bytes=0-0", 0), None);
    }

    #[test]
    fn test_resolve_media_slice_non_chunked_and_chunked() {
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("stream_test.vault");
        let mut db = VaultDb::create(&vault_path, "secret-pass", true).unwrap();

        // 1. Non-chunked MP4-like payload
        let mut mp4_bytes = vec![
            0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2', 0x00, 0x00,
            0x00, 0x00, b'm', b'p', b'4', b'2', b'i', b's', b'o', b'm',
        ];
        mp4_bytes.extend_from_slice(&[0xAB; 200]);

        let single_id = match db
            .import_media_bytes(&mp4_bytes, Some("clip"), None, 1000)
            .unwrap()
        {
            ImportOutcome::Imported { id, .. } => id,
            _ => panic!("expected imported"),
        };

        // 2. Chunked MP4-like payload (force small chunk threshold/size)
        let mut chunked_file = NamedTempFile::new().unwrap();
        let mut chunked_bytes = vec![
            0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2', 0x00, 0x00,
            0x00, 0x00, b'm', b'p', b'4', b'2', b'i', b's', b'o', b'm',
        ];
        chunked_bytes.extend((0..10_000usize).map(|i| (i % 251) as u8));
        chunked_file.write_all(&chunked_bytes).unwrap();
        chunked_file.flush().unwrap();

        let chunked_id = match db
            .import_media_file_with_chunk_params(
                chunked_file.path(),
                Some("chunked_clip"),
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

        // Full non-chunked 200
        let full = resolve_media_slice(&state, &single_id, false, None).unwrap();
        assert_eq!(full.status, StatusCode::OK);
        assert_eq!(full.mime_type, "video/mp4");
        assert_eq!(full.content_length, mp4_bytes.len());
        assert_eq!(full.body, mp4_bytes);

        // HEAD non-chunked 200
        let head = resolve_media_slice(&state, &single_id, true, None).unwrap();
        assert_eq!(head.status, StatusCode::OK);
        assert_eq!(head.content_length, mp4_bytes.len());
        assert!(head.body.is_empty());

        // Range 206 non-chunked
        let part = resolve_media_slice(&state, &single_id, false, Some("bytes=4-15")).unwrap();
        assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.content_range.as_deref(), Some("bytes 4-15/224"));
        assert_eq!(part.body, mp4_bytes[4..=15]);

        // Unsatisfiable 416
        let bad = resolve_media_slice(&state, &single_id, false, Some("bytes=9999-")).unwrap();
        assert_eq!(bad.status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(bad.content_range.as_deref(), Some("bytes */224"));

        // Chunked Range across chunk boundary (4096)
        let c_part =
            resolve_media_slice(&state, &chunked_id, false, Some("bytes=4090-4110")).unwrap();
        assert_eq!(c_part.status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(c_part.body, chunked_bytes[4090..=4110]);
        assert_eq!(
            c_part.content_range.as_deref(),
            Some(format!("bytes 4090-4110/{}", chunked_bytes.len()).as_str())
        );

        // Chunked HEAD
        let c_head = resolve_media_slice(&state, &chunked_id, true, None).unwrap();
        assert_eq!(c_head.status, StatusCode::OK);
        assert_eq!(c_head.content_length, chunked_bytes.len());
        assert!(c_head.body.is_empty());
    }

    #[test]
    fn test_resolve_media_slice_large_explicit_range() {
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("range_large.vault");
        let mut db = VaultDb::create(&vault_path, "secret-pass", true).unwrap();

        let mut big_mp4 = vec![
            0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2', 0x00, 0x00,
            0x00, 0x00, b'm', b'p', b'4', b'2', b'i', b's', b'o', b'm',
        ];
        big_mp4.resize(7 * 1024 * 1024, 0x55);
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&big_mp4).unwrap();
        f.flush().unwrap();

        let id = match db
            .import_media_file_with_chunk_params(f.path(), Some("big"), None, 1000, 100, 1024 * 1024)
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

        // Open-ended range stays bounded to MAX_STREAM_CHUNK_BYTES (4 MiB)
        let open_slice = resolve_media_slice(&state, &id, false, Some("bytes=0-")).unwrap();
        assert_eq!(open_slice.body.len(), MAX_STREAM_CHUNK_BYTES);

        // Explicit range up to 6 MiB honors the full requested range (<= 16 MiB cap)
        let explicit_end = 6 * 1024 * 1024 - 1;
        let explicit_slice = resolve_media_slice(
            &state,
            &id,
            false,
            Some(&format!("bytes=0-{explicit_end}")),
        )
        .unwrap();
        assert_eq!(explicit_slice.body.len(), 6 * 1024 * 1024);
    }
}
