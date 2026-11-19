mod binary;
mod download;
mod process;
#[cfg(test)]
mod tests;

use std::fs::{self, DirBuilder};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::db::{ImportOutcome, VaultDb};
use crate::error::VaultError;
use crate::imaging::is_video_mime;
use crate::models::{EntryKind, EntryRef, ImportReport, UrlImportProgressEvent};

pub use binary::check_ytdlp_status;
pub(crate) use binary::{build_augmented_path, configure_tool_command, resolve_ffmpeg_binary};
pub use download::download_from_url;
pub(crate) use process::{run_child_monitored, ChildRunFailure, MonitorOptions};

pub(super) const MEMORY_PIPE_MAX_BYTES: usize = 128 * 1024 * 1024;
pub(super) const META_TIMEOUT_SECS: u64 = 60;
pub(super) const DOWNLOAD_TOTAL_TIMEOUT_SECS: u64 = 1800;
pub(super) const DOWNLOAD_STALL_TIMEOUT_SECS: u64 = 120;
pub(super) const MAX_MULTI_MEDIA_ITEMS: usize = 20;
pub(super) const PLAYLIST_RANGE_SPEC: &str = "1:20";
pub(super) const DOWNLOAD_PROGRESS_TEMPLATE: &str = "download:TUCK_PROG|%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(info.playlist_index)s|%(info.n_entries)s";
pub(super) const POSTPROCESS_PROGRESS_TEMPLATE: &str =
    "postprocess:TUCK_PP|%(progress.status)s|%(progress.postprocessor)s";

pub const ALLOWED_COOKIE_BROWSERS: &[&str] = &[
    "", "arc", "chrome", "safari", "firefox", "brave", "edge", "vivaldi", "opera", "chromium",
    "whale", "file",
];

pub fn is_valid_cookie_browser(val: &str) -> bool {
    let clean = val.trim().to_ascii_lowercase();
    ALLOWED_COOKIE_BROWSERS.contains(&clean.as_str())
}

pub struct EphemeralStagingDir {
    path: PathBuf,
}

impl EphemeralStagingDir {
    pub fn create() -> Result<Self, VaultError> {
        let path = std::env::temp_dir().join(format!("tuck-dl-{}", Uuid::new_v4()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder.create(&path)?;
        }
        #[cfg(not(unix))]
        {
            DirBuilder::new().create(&path)?;
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for EphemeralStagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub enum DownloadedItemPayload {
    Memory(Vec<u8>),
    File(PathBuf),
}

pub struct DownloadedMediaItem {
    pub payload: DownloadedItemPayload,
    pub title: String,
    pub source_url: String,
    pub duration_ms: i64,
}

pub struct DownloadedUrlBatch {
    pub items: Vec<DownloadedMediaItem>,
    pub warnings: Vec<String>,
    pub(super) _staging_dir: Option<EphemeralStagingDir>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtractedVideoMeta {
    pub title: String,
    pub source_url: String,
    pub duration_ms: i64,
    pub filesize_estimate: Option<u64>,
    pub needs_ffmpeg: bool,
    pub playlist_index: usize,
}

pub fn validate_http_url(raw: &str) -> Result<String, VaultError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(VaultError::InvalidInput(
            "URLを入力してください".to_string(),
        ));
    }
    if trimmed.len() > 2048 {
        return Err(VaultError::InvalidInput(
            "URLが長すぎます（2048文字以内）".to_string(),
        ));
    }
    if trimmed.starts_with('-') {
        return Err(VaultError::InvalidInput(
            "URLの形式が正しくありません".to_string(),
        ));
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(VaultError::InvalidInput(
            "URLに空白や改行を含めることはできません".to_string(),
        ));
    }

    let lower = trimmed.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        r
    } else if let Some(r) = lower.strip_prefix("http://") {
        r
    } else {
        return Err(VaultError::InvalidInput(
            "http:// または https:// で始まるURLのみ対応しています".to_string(),
        ));
    };

    let host_part = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host_part.is_empty() {
        return Err(VaultError::InvalidInput(
            "URLにホスト名が含まれていません".to_string(),
        ));
    }

    Ok(trimmed.to_string())
}

fn is_twitter_host(host: &str) -> bool {
    matches!(
        host,
        "x.com"
            | "www.x.com"
            | "mobile.x.com"
            | "twitter.com"
            | "www.twitter.com"
            | "mobile.twitter.com"
            | "m.twitter.com"
    )
}

fn strip_twitter_status_suffix(url: &str) -> Option<String> {
    let scheme_end = url.find("://")?;
    let after_scheme = &url[scheme_end + 3..];
    let rel_path_start = after_scheme.find('/')?;
    let host_part = &after_scheme[..rel_path_start];
    let host_only = host_part
        .split(':')
        .next()
        .unwrap_or(host_part)
        .to_ascii_lowercase();
    if !is_twitter_host(&host_only) {
        return None;
    }

    let path_start = scheme_end + 3 + rel_path_start;
    let tail = &url[path_start..];
    let query_or_frag = tail.find(['?', '#']).unwrap_or(tail.len());
    let path_only = &tail[..query_or_frag];
    let rest_suffix = &tail[query_or_frag..];

    let status_marker = path_only.find("/status/")?;
    let after_status = &path_only[status_marker + "/status/".len()..];
    let id_len = after_status
        .bytes()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if id_len == 0 {
        return None;
    }

    let after_id = &after_status[id_len..];
    let rest_after_kind = after_id
        .strip_prefix("/photo/")
        .or_else(|| after_id.strip_prefix("/video/"))?;

    let num_len = rest_after_kind
        .bytes()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if num_len == 0 {
        return None;
    }
    let after_num = &rest_after_kind[num_len..];
    if !after_num.is_empty() && !after_num.starts_with('/') {
        return None;
    }

    let base_end = path_start + status_marker + "/status/".len() + id_len;
    Some(format!("{}{}{}", &url[..base_end], after_num, rest_suffix))
}

pub(crate) fn normalize_twitter_url(url: &str) -> String {
    strip_twitter_status_suffix(url).unwrap_or_else(|| url.to_string())
}

pub fn import_downloaded_url_batch<F>(
    db: &mut VaultDb,
    batch: &DownloadedUrlBatch,
    folder_id: Option<&str>,
    mut on_progress: F,
) -> Result<ImportReport, VaultError>
where
    F: FnMut(UrlImportProgressEvent),
{
    let item_total = batch.items.len();
    let mut report = ImportReport::default();

    for (idx, item) in batch.items.iter().enumerate() {
        on_progress(UrlImportProgressEvent {
            stage: "saving".to_string(),
            percent: Some(100.0),
            speed: String::new(),
            eta: String::new(),
            downloaded_bytes: 0,
            total_bytes: None,
            message: "暗号化保管庫へ保存しています...".to_string(),
            item_index: (item_total > 1).then_some(idx + 1),
            item_total: (item_total > 1).then_some(item_total),
        });

        let outcome = match &item.payload {
            DownloadedItemPayload::Memory(bytes) => db.import_media_bytes(
                bytes,
                Some(&item.title),
                Some(&item.source_url),
                item.duration_ms,
            )?,
            DownloadedItemPayload::File(file_path) => db.import_media_file(
                file_path,
                Some(&item.title),
                Some(&item.source_url),
                item.duration_ms,
            )?,
        };

        match outcome {
            ImportOutcome::Imported {
                id,
                mime_type,
                thumb_pending,
            } => {
                if let Some(fid) = folder_id {
                    db.add_entries_to_folder(
                        fid,
                        &[EntryRef {
                            kind: EntryKind::Image,
                            id: id.clone(),
                        }],
                    )?;
                }
                if thumb_pending {
                    if is_video_mime(&mime_type) {
                        report.video_pending_ids.push(id.clone());
                    } else {
                        report.avif_pending_ids.push(id.clone());
                    }
                }
                report.imported_ids.push(id);
            }
            ImportOutcome::Duplicate => {
                report.duplicate_count += 1;
            }
        }
    }

    report.errors.extend(batch.warnings.iter().cloned());
    Ok(report)
}
