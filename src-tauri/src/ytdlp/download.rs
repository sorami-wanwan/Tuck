use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::Value;

use crate::error::VaultError;
use crate::imaging::detect_mime;
use crate::models::{UrlImportProgressEvent, UrlImportSettings};

use super::binary::{
    build_augmented_path, configure_tool_command, prepare_cookie_args, resolve_ffmpeg_binary,
    resolve_ytdlp_binary,
};
use super::process::{
    format_size_limit_label, format_ytdlp_failure, run_child_monitored, ChildRunFailure,
    MonitorOptions,
};
use super::{
    normalize_twitter_url, validate_http_url, DownloadedItemPayload, DownloadedMediaItem,
    DownloadedUrlBatch, EphemeralStagingDir, ExtractedVideoMeta, DOWNLOAD_PROGRESS_TEMPLATE,
    DOWNLOAD_STALL_TIMEOUT_SECS, DOWNLOAD_TOTAL_TIMEOUT_SECS, MAX_MULTI_MEDIA_ITEMS,
    MEMORY_PIPE_MAX_BYTES, META_TIMEOUT_SECS, PLAYLIST_RANGE_SPEC, POSTPROCESS_PROGRESS_TEMPLATE,
};

fn progressive_format_selector(max_height: u32) -> String {
    if max_height == 0 {
        "b[ext=mp4][protocol^=http]/b[ext=webm][protocol^=http]/bv*[ext=mp4][protocol^=http]/bv*[ext=webm][protocol^=http]/b[ext=mp4]/b[ext=webm]/b/bv*"
            .to_string()
    } else {
        format!(
            "b[height<={h}][ext=mp4][protocol^=http]/b[height<={h}][ext=webm][protocol^=http]/bv*[height<={h}][ext=mp4][protocol^=http]/bv*[height<={h}][ext=webm][protocol^=http]/b[height<={h}][ext=mp4]/b[height<={h}][ext=webm]/b[height<={h}]/bv*[height<={h}]/b[ext=mp4][protocol^=http]/b/bv*",
            h = max_height
        )
    }
}

fn muxed_format_selector(max_height: u32) -> String {
    if max_height == 0 {
        "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b/bv*".to_string()
    } else {
        format!(
            "bv*[height<={h}][ext=mp4]+ba[ext=m4a]/b[height<={h}][ext=mp4]/bv*[height<={h}]+ba/b[height<={h}]/bv*[height<={h}]/b[ext=mp4]/b/bv*",
            h = max_height
        )
    }
}

fn is_segmented_protocol(proto: &str) -> bool {
    let p = proto.trim().to_ascii_lowercase();
    p.starts_with("m3u8") || p == "http_dash_segments"
}

fn format_filesize_opt(v: &Value) -> Option<u64> {
    v.get("filesize")
        .and_then(|x| x.as_u64())
        .or_else(|| v.get("filesize_approx").and_then(|x| x.as_u64()))
}

fn extract_single_entry_meta(
    entry: &Value,
    parent: &Value,
    fallback_url: &str,
    default_index: usize,
) -> ExtractedVideoMeta {
    let get_nonempty_str = |key: &str| -> Option<String> {
        entry
            .get(key)
            .and_then(|v| v.as_str())
            .or_else(|| parent.get(key).and_then(|v| v.as_str()))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let title = get_nonempty_str("title").unwrap_or_else(|| "無題の動画".to_string());
    let source_url = get_nonempty_str("webpage_url")
        .or_else(|| get_nonempty_str("original_url"))
        .unwrap_or_else(|| fallback_url.to_string());

    let duration_ms = entry
        .get("duration")
        .and_then(|v| v.as_f64())
        .or_else(|| parent.get("duration").and_then(|v| v.as_f64()))
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| (d * 1000.0).round() as i64)
        .unwrap_or(0);

    let req_formats = entry.get("requested_formats").and_then(|v| v.as_array());
    let filesize_estimate = format_filesize_opt(entry).or_else(|| {
        req_formats.and_then(|arr| {
            let mut sum = 0u64;
            let mut any = false;
            for f in arr {
                if let Some(sz) = format_filesize_opt(f) {
                    sum = sum.saturating_add(sz);
                    any = true;
                }
            }
            if any {
                Some(sum)
            } else {
                None
            }
        })
    });

    let split_streams = req_formats.is_some_and(|arr| arr.len() > 1);
    let main_proto = entry.get("protocol").and_then(|v| v.as_str()).unwrap_or("");
    let segmented = is_segmented_protocol(main_proto)
        || req_formats.is_some_and(|arr| {
            arr.iter().any(|f| {
                f.get("protocol")
                    .and_then(|v| v.as_str())
                    .is_some_and(is_segmented_protocol)
            })
        });

    let playlist_index = entry
        .get("playlist_index")
        .and_then(|v| v.as_u64())
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n > 0)
        .unwrap_or(default_index);

    ExtractedVideoMeta {
        title,
        source_url,
        duration_ms,
        filesize_estimate,
        needs_ffmpeg: split_streams || segmented,
        playlist_index,
    }
}

pub(crate) fn extract_video_metadata_list(
    meta_json: &Value,
    fallback_url: &str,
) -> Vec<ExtractedVideoMeta> {
    if meta_json.get("_type").and_then(|v| v.as_str()) == Some("playlist") {
        if let Some(entries) = meta_json.get("entries").and_then(|v| v.as_array()) {
            let mut list = Vec::new();
            for entry in entries
                .iter()
                .filter(|e| !e.is_null())
                .take(MAX_MULTI_MEDIA_ITEMS)
            {
                let idx = list.len() + 1;
                list.push(extract_single_entry_meta(
                    entry,
                    meta_json,
                    fallback_url,
                    idx,
                ));
            }
            if !list.is_empty() {
                return list;
            }
        }
    }
    vec![extract_single_entry_meta(
        meta_json,
        meta_json,
        fallback_url,
        1,
    )]
}

fn parse_leading_playlist_index(file_name: &str) -> Option<usize> {
    let digits: String = file_name
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<usize>().ok().filter(|&n| n > 0)
}

pub(crate) fn collect_multi_staged_files(
    dir: &Path,
    metas: &[ExtractedVideoMeta],
    max_bytes: Option<u64>,
    max_video_size_mb: u32,
) -> (Vec<DownloadedMediaItem>, Vec<String>) {
    let mut raw_files: Vec<(Option<usize>, String, PathBuf, u64)> = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let fname = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let lower = fname.to_ascii_lowercase();
            if lower.ends_with(".part") || lower.ends_with(".ytdl") || lower.ends_with(".tmp") {
                continue;
            }
            let sz = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if sz == 0 {
                continue;
            }
            let idx = parse_leading_playlist_index(&fname);
            raw_files.push((idx, fname, p, sz));
        }
    }

    raw_files.sort_by(|a, b| {
        a.0.unwrap_or(usize::MAX)
            .cmp(&b.0.unwrap_or(usize::MAX))
            .then_with(|| a.1.cmp(&b.1))
    });

    let mut items = Vec::new();
    let mut warnings = Vec::new();
    let fallback_meta = ExtractedVideoMeta {
        title: "無題の動画".to_string(),
        source_url: String::new(),
        duration_ms: 0,
        filesize_estimate: None,
        needs_ffmpeg: false,
        playlist_index: 1,
    };

    for (pos, (parsed_idx, _fname, file_path, sz)) in raw_files.into_iter().enumerate() {
        let meta = parsed_idx
            .and_then(|idx| metas.iter().find(|m| m.playlist_index == idx))
            .or_else(|| metas.get(pos))
            .or_else(|| metas.first())
            .unwrap_or(&fallback_meta);

        if let Some(limit) = max_bytes {
            if sz > limit {
                let label = format_size_limit_label(max_video_size_mb);
                warnings.push(format!(
                    "{}: サイズ上限（{label}）を超えているためスキップしました",
                    meta.title
                ));
                continue;
            }
        }

        let Ok(head) = read_file_head(&file_path) else {
            warnings.push(format!("{}: ファイルの読み取りに失敗しました", meta.title));
            continue;
        };
        if detect_mime(&head).is_err() {
            warnings.push(format!(
                "{}: 対応動画形式（MP4 / WebM / MOV）ではないためスキップしました",
                meta.title
            ));
            continue;
        }

        items.push(DownloadedMediaItem {
            payload: DownloadedItemPayload::File(file_path),
            title: meta.title.clone(),
            source_url: meta.source_url.clone(),
            duration_ms: meta.duration_ms,
        });
    }

    (items, warnings)
}

fn find_staged_media_file(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if name.ends_with(".part") || name.ends_with(".ytdl") || name.ends_with(".tmp") {
                continue;
            }
            let sz = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if sz == 0 {
                continue;
            }
            if best.as_ref().is_none_or(|(cur_sz, _)| sz > *cur_sz) {
                best = Some((sz, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

fn read_file_head(path: &Path) -> Result<Vec<u8>, VaultError> {
    crate::imaging::read_file_head(path, crate::imaging::HEADER_SNIFF_BYTES)
        .map_err(VaultError::from)
}

struct YtDlpExecContext<'a> {
    bin: &'a Path,
    aug_path: &'a str,
    url: &'a str,
    cookie_args: &'a [String],
    format_spec: &'a str,
    ffmpeg_bin: Option<&'a Path>,
    max_video_size_mb: u32,
    has_cookie: bool,
    cancel: &'a AtomicBool,
}

fn run_metadata_probe<F>(ctx: &YtDlpExecContext<'_>, on_progress: &F) -> Result<Value, VaultError>
where
    F: Fn(UrlImportProgressEvent),
{
    let mut cmd = Command::new(ctx.bin);
    configure_tool_command(&mut cmd, ctx.aug_path);
    cmd.args([
        "--no-playlist",
        "--playlist-items",
        PLAYLIST_RANGE_SPEC,
        "--no-warnings",
        "--dump-single-json",
        "-f",
        ctx.format_spec,
    ]);
    if let Some(ff) = ctx.ffmpeg_bin {
        cmd.arg("--ffmpeg-location").arg(ff);
    }
    cmd.args(ctx.cookie_args).arg("--").arg(ctx.url);

    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| VaultError::UrlImport(format!("yt-dlp の起動に失敗しました: {e}")))?;

    let out = match run_child_monitored(
        child,
        MonitorOptions {
            total_timeout: Duration::from_secs(META_TIMEOUT_SECS),
            stall_timeout: Duration::from_secs(META_TIMEOUT_SECS),
            capture_stdout: true,
            stdout_cap: 16 * 1024 * 1024,
            cancel: ctx.cancel,
            staging_watch: None,
            item_total_hint: None,
        },
        on_progress,
    ) {
        Ok(o) => o,
        Err(ChildRunFailure::Cancelled) => {
            return Err(VaultError::UrlImport(
                "取り込みをキャンセルしました".to_string(),
            ));
        }
        Err(ChildRunFailure::TotalTimeout | ChildRunFailure::StallTimeout) => {
            return Err(VaultError::UrlImport(
                "URLの解析がタイムアウトしました（ブラウザの Cookie アクセス許可ダイアログが出ていないかご確認ください）"
                    .to_string(),
            ));
        }
        Err(ChildRunFailure::MemoryLimitExceeded | ChildRunFailure::StagingLimitExceeded) => {
            return Err(VaultError::UrlImport(
                "動画メタデータが大きすぎます".to_string(),
            ));
        }
        Err(ChildRunFailure::Io(e)) => return Err(VaultError::Io(e)),
    };

    if !out.status.success() {
        return Err(VaultError::UrlImport(format_ytdlp_failure(
            "meta",
            &out.stderr,
            ctx.has_cookie,
            ctx.ffmpeg_bin.is_some(),
        )));
    }

    serde_json::from_slice(&out.stdout)
        .map_err(|_| VaultError::UrlImport("動画メタデータの解析に失敗しました".to_string()))
}

fn download_memory_pipe<F>(
    ctx: &YtDlpExecContext<'_>,
    playlist_item: usize,
    max_pipe_bytes: usize,
    on_progress: &F,
) -> Result<Vec<u8>, Result<(), VaultError>>
where
    F: Fn(UrlImportProgressEvent),
{
    let item_spec = playlist_item.max(1).to_string();
    let mut cmd = Command::new(ctx.bin);
    configure_tool_command(&mut cmd, ctx.aug_path);
    cmd.args([
        "--no-playlist",
        "--playlist-items",
        &item_spec,
        "--no-warnings",
        "--newline",
        "--progress",
        "--progress-template",
        DOWNLOAD_PROGRESS_TEMPLATE,
        "-f",
        ctx.format_spec,
        "-o",
        "-",
    ]);
    if let Some(ff) = ctx.ffmpeg_bin {
        cmd.arg("--ffmpeg-location").arg(ff);
    }
    cmd.args(ctx.cookie_args).arg("--").arg(ctx.url);

    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            Err(VaultError::UrlImport(format!(
                "yt-dlp の起動に失敗しました: {e}"
            )))
        })?;

    let out = match run_child_monitored(
        child,
        MonitorOptions {
            total_timeout: Duration::from_secs(DOWNLOAD_TOTAL_TIMEOUT_SECS),
            stall_timeout: Duration::from_secs(DOWNLOAD_STALL_TIMEOUT_SECS),
            capture_stdout: true,
            stdout_cap: max_pipe_bytes,
            cancel: ctx.cancel,
            staging_watch: None,
            item_total_hint: Some(1),
        },
        on_progress,
    ) {
        Ok(o) => o,
        Err(ChildRunFailure::MemoryLimitExceeded) => return Err(Ok(())),
        Err(ChildRunFailure::Cancelled) => {
            return Err(Err(VaultError::UrlImport(
                "取り込みをキャンセルしました".to_string(),
            )));
        }
        Err(ChildRunFailure::StallTimeout) => {
            return Err(Err(VaultError::UrlImport(
                "通信が一定時間（120秒）応答しなかったため中断しました".to_string(),
            )));
        }
        Err(ChildRunFailure::TotalTimeout) => {
            return Err(Err(VaultError::UrlImport(
                "ダウンロードが全体の上限時間（30分）を超えたため中断しました".to_string(),
            )));
        }
        Err(ChildRunFailure::StagingLimitExceeded) => unreachable!(),
        Err(ChildRunFailure::Io(e)) => return Err(Err(VaultError::Io(e))),
    };

    if !out.status.success() {
        return Err(Err(VaultError::UrlImport(format_ytdlp_failure(
            "download",
            &out.stderr,
            ctx.has_cookie,
            ctx.ffmpeg_bin.is_some(),
        ))));
    }
    if out.stdout.is_empty() {
        return Err(Err(VaultError::UrlImport(
            "取得した動画データが空でした（HLS 専用動画の可能性があります。ffmpeg の導入をご検討ください）"
                .to_string(),
        )));
    }
    detect_mime(&out.stdout).map_err(|_| {
        Err(VaultError::UrlImport(
            "取得したファイルは対応動画形式（MP4 / WebM / MOV）ではありませんでした".to_string(),
        ))
    })?;

    Ok(out.stdout)
}

fn download_staged_file<F>(
    ctx: &YtDlpExecContext<'_>,
    playlist_item: usize,
    on_progress: &F,
) -> Result<(PathBuf, EphemeralStagingDir), VaultError>
where
    F: Fn(UrlImportProgressEvent),
{
    let staging = EphemeralStagingDir::create()?;
    let output_tmpl = staging.path().join("video.%(ext)s");
    let item_spec = playlist_item.max(1).to_string();

    let mut cmd = Command::new(ctx.bin);
    configure_tool_command(&mut cmd, ctx.aug_path);
    cmd.args([
        "--no-playlist",
        "--playlist-items",
        &item_spec,
        "--no-warnings",
        "--newline",
        "--progress",
        "--progress-template",
        DOWNLOAD_PROGRESS_TEMPLATE,
        "--progress-template",
        POSTPROCESS_PROGRESS_TEMPLATE,
        "--concurrent-fragments",
        "4",
        "-f",
        ctx.format_spec,
    ]);
    if let Some(ff) = ctx.ffmpeg_bin {
        cmd.arg("--ffmpeg-location").arg(ff).args([
            "--merge-output-format",
            "mp4",
            "--remux-video",
            "mp4",
            "--postprocessor-args",
            "Merger+ffmpeg_o:-movflags +faststart",
        ]);
    }
    if ctx.max_video_size_mb > 0 {
        cmd.arg("--max-filesize")
            .arg(format!("{}M", ctx.max_video_size_mb));
    }
    cmd.arg("-o").arg(&output_tmpl);
    cmd.args(ctx.cookie_args).arg("--").arg(ctx.url);

    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| VaultError::UrlImport(format!("yt-dlp の起動に失敗しました: {e}")))?;

    let max_bytes = if ctx.max_video_size_mb == 0 {
        None
    } else {
        Some((ctx.max_video_size_mb as u64) * 1024 * 1024)
    };
    let staging_limit = max_bytes.map(|b| b.saturating_mul(2).saturating_add(128 * 1024 * 1024));
    let staging_watch = staging_limit.map(|lim| (staging.path(), lim));

    let out = match run_child_monitored(
        child,
        MonitorOptions {
            total_timeout: Duration::from_secs(DOWNLOAD_TOTAL_TIMEOUT_SECS),
            stall_timeout: Duration::from_secs(DOWNLOAD_STALL_TIMEOUT_SECS),
            capture_stdout: false,
            stdout_cap: 0,
            cancel: ctx.cancel,
            staging_watch,
            item_total_hint: Some(1),
        },
        on_progress,
    ) {
        Ok(o) => o,
        Err(ChildRunFailure::Cancelled) => {
            return Err(VaultError::UrlImport(
                "取り込みをキャンセルしました".to_string(),
            ));
        }
        Err(ChildRunFailure::StagingLimitExceeded) => {
            let label = format_size_limit_label(ctx.max_video_size_mb);
            return Err(VaultError::UrlImport(format!(
                "動画サイズが設定上限（{label}）を超えたためダウンロードを中断しました（設定画面から上限を変更できます）"
            )));
        }
        Err(ChildRunFailure::StallTimeout) => {
            return Err(VaultError::UrlImport(
                "通信が一定時間（120秒）応答しなかったため中断しました".to_string(),
            ));
        }
        Err(ChildRunFailure::TotalTimeout) => {
            return Err(VaultError::UrlImport(
                "ダウンロードが全体の上限時間（30分）を超えたため中断しました".to_string(),
            ));
        }
        Err(ChildRunFailure::MemoryLimitExceeded) => unreachable!(),
        Err(ChildRunFailure::Io(e)) => return Err(VaultError::Io(e)),
    };

    if !out.status.success() {
        let mut combined = out.stderr;
        combined.extend_from_slice(&out.stdout);
        return Err(VaultError::UrlImport(format_ytdlp_failure(
            "download",
            &combined,
            ctx.has_cookie,
            ctx.ffmpeg_bin.is_some(),
        )));
    }

    let Some(file_path) = find_staged_media_file(staging.path()) else {
        let mut combined = out.stderr;
        combined.extend_from_slice(&out.stdout);
        return Err(VaultError::UrlImport(format_ytdlp_failure(
            "download",
            &combined,
            ctx.has_cookie,
            ctx.ffmpeg_bin.is_some(),
        )));
    };

    if let Some(limit) = max_bytes {
        let final_len = fs::metadata(&file_path)?.len();
        if final_len > limit {
            let label = format_size_limit_label(ctx.max_video_size_mb);
            return Err(VaultError::UrlImport(format!(
                "取得後の動画サイズが上限（{label}）を超えているため取り込めません（設定画面から上限を変更できます）"
            )));
        }
    }

    let head = read_file_head(&file_path)?;
    detect_mime(&head).map_err(|_| {
        VaultError::UrlImport(
            "取得したファイルは対応動画形式（MP4 / WebM / MOV）ではありませんでした".to_string(),
        )
    })?;

    Ok((file_path, staging))
}

fn download_multi_staged_files<F>(
    ctx: &YtDlpExecContext<'_>,
    metas: &[ExtractedVideoMeta],
    on_progress: &F,
) -> Result<DownloadedUrlBatch, VaultError>
where
    F: Fn(UrlImportProgressEvent),
{
    let staging = EphemeralStagingDir::create()?;
    let output_tmpl = staging.path().join("%(playlist_index)03d_video.%(ext)s");

    let mut cmd = Command::new(ctx.bin);
    configure_tool_command(&mut cmd, ctx.aug_path);
    cmd.args([
        "--no-playlist",
        "--playlist-items",
        PLAYLIST_RANGE_SPEC,
        "--ignore-errors",
        "--no-warnings",
        "--newline",
        "--progress",
        "--progress-template",
        DOWNLOAD_PROGRESS_TEMPLATE,
        "--progress-template",
        POSTPROCESS_PROGRESS_TEMPLATE,
        "--concurrent-fragments",
        "4",
        "-f",
        ctx.format_spec,
    ]);
    if let Some(ff) = ctx.ffmpeg_bin {
        cmd.arg("--ffmpeg-location").arg(ff).args([
            "--merge-output-format",
            "mp4",
            "--remux-video",
            "mp4",
            "--postprocessor-args",
            "Merger+ffmpeg_o:-movflags +faststart",
        ]);
    }
    if ctx.max_video_size_mb > 0 {
        cmd.arg("--max-filesize")
            .arg(format!("{}M", ctx.max_video_size_mb));
    }
    cmd.arg("-o").arg(&output_tmpl);
    cmd.args(ctx.cookie_args).arg("--").arg(ctx.url);

    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| VaultError::UrlImport(format!("yt-dlp の起動に失敗しました: {e}")))?;

    let max_bytes = if ctx.max_video_size_mb == 0 {
        None
    } else {
        Some((ctx.max_video_size_mb as u64) * 1024 * 1024)
    };
    let staging_limit = max_bytes.map(|b| {
        b.saturating_mul((metas.len() as u64).saturating_add(1))
            .saturating_add(128 * 1024 * 1024)
    });
    let staging_watch = staging_limit.map(|lim| (staging.path(), lim));

    let out = match run_child_monitored(
        child,
        MonitorOptions {
            total_timeout: Duration::from_secs(DOWNLOAD_TOTAL_TIMEOUT_SECS),
            stall_timeout: Duration::from_secs(DOWNLOAD_STALL_TIMEOUT_SECS),
            capture_stdout: false,
            stdout_cap: 0,
            cancel: ctx.cancel,
            staging_watch,
            item_total_hint: Some(metas.len()),
        },
        on_progress,
    ) {
        Ok(o) => o,
        Err(ChildRunFailure::Cancelled) => {
            return Err(VaultError::UrlImport(
                "取り込みをキャンセルしました".to_string(),
            ));
        }
        Err(ChildRunFailure::StagingLimitExceeded) => {
            let label = format_size_limit_label(ctx.max_video_size_mb);
            return Err(VaultError::UrlImport(format!(
                "一時領域の使用量が想定上限を超えたためダウンロードを中断しました（個別上限: {label}）"
            )));
        }
        Err(ChildRunFailure::StallTimeout) => {
            return Err(VaultError::UrlImport(
                "通信が一定時間（120秒）応答しなかったため中断しました".to_string(),
            ));
        }
        Err(ChildRunFailure::TotalTimeout) => {
            return Err(VaultError::UrlImport(
                "ダウンロードが全体の上限時間（30分）を超えたため中断しました".to_string(),
            ));
        }
        Err(ChildRunFailure::MemoryLimitExceeded) => unreachable!(),
        Err(ChildRunFailure::Io(e)) => return Err(VaultError::Io(e)),
    };

    let (items, mut warnings) =
        collect_multi_staged_files(staging.path(), metas, max_bytes, ctx.max_video_size_mb);

    if items.is_empty() {
        if !warnings.is_empty() {
            return Err(VaultError::UrlImport(warnings.join("\n")));
        }
        let mut combined = out.stderr;
        combined.extend_from_slice(&out.stdout);
        return Err(VaultError::UrlImport(format_ytdlp_failure(
            "download",
            &combined,
            ctx.has_cookie,
            ctx.ffmpeg_bin.is_some(),
        )));
    }

    if (!out.status.success() || items.len() < metas.len()) && warnings.is_empty() {
        warnings.push("一部の動画・GIFは取得できなかったためスキップしました".to_string());
    }

    Ok(DownloadedUrlBatch {
        items,
        warnings,
        _staging_dir: Some(staging),
    })
}

pub fn download_from_url<F>(
    raw_url: &str,
    settings: &UrlImportSettings,
    cancel: &AtomicBool,
    on_progress: F,
) -> Result<DownloadedUrlBatch, VaultError>
where
    F: Fn(UrlImportProgressEvent),
{
    let validated = validate_http_url(raw_url)?;
    let active_url = normalize_twitter_url(&validated);

    let bin = resolve_ytdlp_binary(&settings.ytdlp_path)?;
    let ffmpeg_custom = settings.ffmpeg_path.trim();
    let ffmpeg_bin = resolve_ffmpeg_binary(ffmpeg_custom);
    if !ffmpeg_custom.is_empty() && ffmpeg_bin.is_none() {
        return Err(VaultError::UrlImport(format!(
            "指定された ffmpeg が見つかりません: {ffmpeg_custom}"
        )));
    }

    let aug_path = build_augmented_path();
    let cookies = prepare_cookie_args(settings, &active_url, cancel)?;

    on_progress(UrlImportProgressEvent {
        stage: "meta".to_string(),
        percent: None,
        speed: String::new(),
        eta: String::new(),
        downloaded_bytes: 0,
        total_bytes: None,
        message: "URL の動画情報を解析しています...".to_string(),
        item_index: None,
        item_total: None,
    });

    let format_spec = if ffmpeg_bin.is_some() {
        muxed_format_selector(settings.max_video_height)
    } else {
        progressive_format_selector(settings.max_video_height)
    };

    let ctx = YtDlpExecContext {
        bin: &bin,
        aug_path: &aug_path,
        url: &active_url,
        cookie_args: &cookies.args,
        format_spec: &format_spec,
        ffmpeg_bin: ffmpeg_bin.as_deref(),
        max_video_size_mb: settings.max_video_size_mb,
        has_cookie: cookies.has_cookie,
        cancel,
    };

    let meta_json = run_metadata_probe(&ctx, &on_progress)?;

    let metas = extract_video_metadata_list(&meta_json, &active_url);

    let max_bytes_opt = if settings.max_video_size_mb == 0 {
        None
    } else {
        Some((settings.max_video_size_mb as u64) * 1024 * 1024)
    };

    if metas.len() >= 2 {
        if metas.iter().all(|m| m.needs_ffmpeg) && ffmpeg_bin.is_none() {
            return Err(VaultError::UrlImport(
                "この投稿の動画はすべて HLS（m3u8）または分離形式で配信されているため、取り込みには ffmpeg が必要です。\nffmpeg をインストール（brew / winget / apt 等）してから再度お試しください"
                    .to_string(),
            ));
        }
        if let Some(limit) = max_bytes_opt {
            if metas
                .iter()
                .all(|m| m.filesize_estimate.is_some_and(|est| est > limit))
            {
                let label = format_size_limit_label(settings.max_video_size_mb);
                return Err(VaultError::UrlImport(format!(
                    "すべての動画サイズが上限（{label}）を超えているため取り込めません（設定画面から上限を変更できます）"
                )));
            }
        }
        return download_multi_staged_files(&ctx, &metas, &on_progress);
    }

    let meta = metas
        .into_iter()
        .next()
        .ok_or_else(|| VaultError::UrlImport("動画情報が見つかりませんでした".to_string()))?;

    if let (Some(est), Some(limit)) = (meta.filesize_estimate, max_bytes_opt) {
        if est > limit {
            let label = format_size_limit_label(settings.max_video_size_mb);
            return Err(VaultError::UrlImport(format!(
                "動画サイズが上限（{label}）を超えているため取り込めません（設定画面から上限を変更できます）"
            )));
        }
    }

    if meta.needs_ffmpeg && ffmpeg_bin.is_none() {
        return Err(VaultError::UrlImport(
            "この動画は HLS（m3u8）または映像・音声分離形式で配信されているため、取り込みには ffmpeg が必要です。\nffmpeg をインストール（brew / winget / apt 等）してから再度お試しください"
                .to_string(),
        ));
    }

    let prefer_staging = meta.needs_ffmpeg
        || meta
            .filesize_estimate
            .is_some_and(|s| s > MEMORY_PIPE_MAX_BYTES as u64)
        || (meta.filesize_estimate.is_none()
            && meta.duration_ms > 300_000
            && max_bytes_opt.is_none_or(|b| b > MEMORY_PIPE_MAX_BYTES as u64));

    if !prefer_staging {
        let pipe_cap = match max_bytes_opt {
            Some(limit) => limit.min(MEMORY_PIPE_MAX_BYTES as u64) as usize,
            None => MEMORY_PIPE_MAX_BYTES,
        };
        match download_memory_pipe(&ctx, meta.playlist_index, pipe_cap, &on_progress) {
            Ok(bytes) => {
                return Ok(DownloadedUrlBatch {
                    items: vec![DownloadedMediaItem {
                        payload: DownloadedItemPayload::Memory(bytes),
                        title: meta.title,
                        source_url: meta.source_url,
                        duration_ms: meta.duration_ms,
                    }],
                    warnings: Vec::new(),
                    _staging_dir: None,
                });
            }
            Err(Ok(())) => {
                if max_bytes_opt.is_some_and(|b| b <= MEMORY_PIPE_MAX_BYTES as u64) {
                    let label = format_size_limit_label(settings.max_video_size_mb);
                    return Err(VaultError::UrlImport(format!(
                        "動画サイズが上限（{label}）を超えているため取り込めません（設定画面から上限を変更できます）"
                    )));
                }
            }
            Err(Err(e)) => return Err(e),
        }
    }

    let (file_path, staging_dir) = download_staged_file(&ctx, meta.playlist_index, &on_progress)?;

    Ok(DownloadedUrlBatch {
        items: vec![DownloadedMediaItem {
            payload: DownloadedItemPayload::File(file_path),
            title: meta.title,
            source_url: meta.source_url,
            duration_ms: meta.duration_ms,
        }],
        warnings: Vec::new(),
        _staging_dir: Some(staging_dir),
    })
}
