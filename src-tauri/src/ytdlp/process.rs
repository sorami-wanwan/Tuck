use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::models::UrlImportProgressEvent;

enum ChildStreamMsg {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    StdoutDone,
    StderrDone,
}

pub(super) struct CapturedCommandOutput {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

pub(super) enum ChildRunFailure {
    Cancelled,
    MemoryLimitExceeded,
    StagingLimitExceeded,
    StallTimeout,
    TotalTimeout,
    Io(std::io::Error),
}

fn dir_size_bytes(dir: &Path) -> u64 {
    let mut sum = 0u64;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Ok(m) = entry.metadata() {
                    sum = sum.saturating_add(m.len());
                }
            }
        }
    }
    sum
}

pub(crate) fn parse_progress_line(
    line: &str,
    item_total_hint: Option<usize>,
) -> Option<UrlImportProgressEvent> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("TUCK_PROG|") {
        let parts: Vec<&str> = rest.split('|').collect();
        if parts.len() < 5 {
            return None;
        }
        let pct_str = parts[0].trim().trim_end_matches('%').trim();
        let percent = pct_str
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 100.0));

        let clean_metric = |s: &str| -> String {
            let t = s.trim();
            if t.is_empty()
                || t.eq_ignore_ascii_case("na")
                || t.eq_ignore_ascii_case("n/a")
                || t.eq_ignore_ascii_case("unknown")
            {
                String::new()
            } else {
                t.to_string()
            }
        };

        let speed = clean_metric(parts[1]);
        let eta = clean_metric(parts[2]);
        let downloaded_bytes = parts[3]
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|v| v.round() as u64)
            .unwrap_or(0);

        let parse_opt_bytes = |s: &str| -> Option<u64> {
            s.trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| v.round() as u64)
        };
        let total_bytes =
            parse_opt_bytes(parts[4]).or_else(|| parts.get(5).and_then(|s| parse_opt_bytes(s)));

        let parse_opt_usize = |opt: Option<&&str>| -> Option<usize> {
            opt.and_then(|s| s.trim().parse::<usize>().ok())
                .filter(|&n| n > 0)
        };
        let parsed_idx = parse_opt_usize(parts.get(6));
        let parsed_total = parse_opt_usize(parts.get(7)).or(item_total_hint);
        let (item_index, item_total) = if parsed_total.is_some_and(|t| t > 1) {
            (parsed_idx.or(Some(1)), parsed_total)
        } else {
            (None, None)
        };

        return Some(UrlImportProgressEvent {
            stage: "downloading".to_string(),
            percent,
            speed,
            eta,
            downloaded_bytes,
            total_bytes,
            message: "動画をダウンロード中...".to_string(),
            item_index,
            item_total,
        });
    }

    let hint_multi = item_total_hint.filter(|&t| t > 1);
    if let Some(rest) = trimmed.strip_prefix("TUCK_PP|") {
        let parts: Vec<&str> = rest.split('|').collect();
        let pp_name = parts.get(1).copied().unwrap_or("").trim();
        let message = if pp_name.contains("Merger") {
            "ffmpeg で映像と音声を MP4 に結合中..."
        } else if pp_name.contains("Remux") || pp_name.contains("Convert") {
            "ffmpeg で MP4 形式へ変換中..."
        } else {
            "ffmpeg で後処理を実行中..."
        };
        return Some(UrlImportProgressEvent {
            stage: "muxing".to_string(),
            percent: None,
            speed: String::new(),
            eta: String::new(),
            downloaded_bytes: 0,
            total_bytes: None,
            message: message.to_string(),
            item_index: None,
            item_total: hint_multi,
        });
    }

    if trimmed.contains("[Merger]")
        || trimmed.contains("[VideoRemuxer]")
        || trimmed.contains("[FixupM3u8]")
    {
        return Some(UrlImportProgressEvent {
            stage: "muxing".to_string(),
            percent: None,
            speed: String::new(),
            eta: String::new(),
            downloaded_bytes: 0,
            total_bytes: None,
            message: "ffmpeg で MP4 に変換・結合中...".to_string(),
            item_index: None,
            item_total: hint_multi,
        });
    }

    None
}

fn spawn_pipe_reader<R: Read + Send + 'static>(
    mut reader: R,
    tx: mpsc::Sender<ChildStreamMsg>,
    is_stdout: bool,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(if is_stdout {
                        ChildStreamMsg::StdoutDone
                    } else {
                        ChildStreamMsg::StderrDone
                    });
                    break;
                }
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    let msg = if is_stdout {
                        ChildStreamMsg::Stdout(chunk)
                    } else {
                        ChildStreamMsg::Stderr(chunk)
                    };
                    if tx.send(msg).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

fn push_capped_tail(buf: &mut Vec<u8>, chunk: &[u8], cap: usize) {
    buf.extend_from_slice(chunk);
    if buf.len() > cap {
        let excess = buf.len() - cap;
        buf.drain(0..excess);
    }
}

fn feed_progress_lines<F>(
    pending: &mut Vec<u8>,
    chunk: &[u8],
    item_total_hint: Option<usize>,
    on_progress: &F,
) where
    F: Fn(UrlImportProgressEvent),
{
    pending.extend_from_slice(chunk);
    while let Some(pos) = pending.iter().position(|&b| b == b'\n' || b == b'\r') {
        let line_bytes: Vec<u8> = pending.drain(..=pos).collect();
        let line = String::from_utf8_lossy(&line_bytes);
        if let Some(ev) = parse_progress_line(&line, item_total_hint) {
            on_progress(ev);
        }
    }
    if pending.len() > 8192 {
        pending.clear();
    }
}

pub(super) struct MonitorOptions<'a> {
    pub(super) total_timeout: Duration,
    pub(super) stall_timeout: Duration,
    pub(super) capture_stdout: bool,
    pub(super) stdout_cap: usize,
    pub(super) cancel: &'a AtomicBool,
    pub(super) staging_watch: Option<(&'a Path, u64)>,
    pub(super) item_total_hint: Option<usize>,
}

pub(super) fn run_child_monitored<F>(
    mut child: Child,
    opts: MonitorOptions<'_>,
    on_progress: &F,
) -> Result<CapturedCommandOutput, ChildRunFailure>
where
    F: Fn(UrlImportProgressEvent),
{
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| ChildRunFailure::Io(std::io::Error::other("missing stdout")))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| ChildRunFailure::Io(std::io::Error::other("missing stderr")))?;

    let (tx, rx) = mpsc::channel();
    let h_out = spawn_pipe_reader(stdout_pipe, tx.clone(), true);
    let h_err = spawn_pipe_reader(stderr_pipe, tx, false);

    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let mut out_line_buf = Vec::new();
    let mut err_line_buf = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;

    let started = Instant::now();
    let mut last_activity = Instant::now();
    let mut last_dir_check = Instant::now();

    let kill_and_cleanup =
        |child: &mut Child, h1: thread::JoinHandle<()>, h2: thread::JoinHandle<()>| {
            let _ = child.kill();
            let _ = child.wait();
            let _ = h1.join();
            let _ = h2.join();
        };

    while !stdout_done || !stderr_done {
        if opts.cancel.load(Ordering::SeqCst) {
            kill_and_cleanup(&mut child, h_out, h_err);
            return Err(ChildRunFailure::Cancelled);
        }

        if let Some((dir, limit_bytes)) = opts.staging_watch {
            if last_dir_check.elapsed() >= Duration::from_secs(1) {
                last_dir_check = Instant::now();
                if dir_size_bytes(dir) > limit_bytes {
                    kill_and_cleanup(&mut child, h_out, h_err);
                    return Err(ChildRunFailure::StagingLimitExceeded);
                }
            }
        }

        match rx.recv_timeout(Duration::from_millis(120)) {
            Ok(ChildStreamMsg::Stdout(chunk)) => {
                last_activity = Instant::now();
                if opts.capture_stdout {
                    if stdout_buf.len().saturating_add(chunk.len()) > opts.stdout_cap {
                        kill_and_cleanup(&mut child, h_out, h_err);
                        return Err(ChildRunFailure::MemoryLimitExceeded);
                    }
                    stdout_buf.extend_from_slice(&chunk);
                } else {
                    feed_progress_lines(
                        &mut out_line_buf,
                        &chunk,
                        opts.item_total_hint,
                        on_progress,
                    );
                    push_capped_tail(&mut stdout_buf, &chunk, 64 * 1024);
                }
            }
            Ok(ChildStreamMsg::Stderr(chunk)) => {
                last_activity = Instant::now();
                feed_progress_lines(&mut err_line_buf, &chunk, opts.item_total_hint, on_progress);
                push_capped_tail(&mut stderr_buf, &chunk, 256 * 1024);
            }
            Ok(ChildStreamMsg::StdoutDone) => stdout_done = true,
            Ok(ChildStreamMsg::StderrDone) => stderr_done = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if started.elapsed() > opts.total_timeout {
                    kill_and_cleanup(&mut child, h_out, h_err);
                    return Err(ChildRunFailure::TotalTimeout);
                }
                if last_activity.elapsed() > opts.stall_timeout {
                    kill_and_cleanup(&mut child, h_out, h_err);
                    return Err(ChildRunFailure::StallTimeout);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if !out_line_buf.is_empty() {
        let line = String::from_utf8_lossy(&out_line_buf);
        if let Some(ev) = parse_progress_line(&line, opts.item_total_hint) {
            on_progress(ev);
        }
    }
    if !err_line_buf.is_empty() {
        let line = String::from_utf8_lossy(&err_line_buf);
        if let Some(ev) = parse_progress_line(&line, opts.item_total_hint) {
            on_progress(ev);
        }
    }

    let status = child.wait().map_err(ChildRunFailure::Io)?;
    let _ = h_out.join();
    let _ = h_err.join();

    Ok(CapturedCommandOutput {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    })
}

pub(super) fn extract_ytdlp_error_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let last_err = text
        .lines()
        .rev()
        .find(|l| l.contains("ERROR:"))
        .or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("動画を取得できませんでした");
    let cleaned = last_err
        .trim()
        .trim_start_matches("ERROR:")
        .trim()
        .replace('\r', " ");
    let mut short: String = cleaned.chars().take(180).collect();
    if cleaned.chars().count() > 180 {
        short.push('…');
    }
    if short.is_empty() {
        "動画を取得できませんでした".to_string()
    } else {
        short
    }
}

pub(crate) fn format_ytdlp_failure(
    stage: &str,
    stderr: &[u8],
    has_cookie: bool,
    has_ffmpeg: bool,
) -> String {
    let full_lower = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    let short = extract_ytdlp_error_line(stderr);

    if full_lower.contains("no video could be found in this tweet")
        || full_lower.contains("nsfw")
        || full_lower.contains("age-restricted")
        || full_lower.contains("login required")
        || full_lower.contains("sign in to confirm")
        || full_lower.contains("this tweet is unavailable")
        || full_lower.contains("inappropriate content")
    {
        if !has_cookie {
            return format!(
                "ログイン必須または年齢制限付きの投稿のため、未ログイン状態では取得できません。\n設定画面の「認証・Cookie 連携」から普段使っているブラウザ（Arc Browser 等）または cookies.txt を設定してください。\n詳細: {short}"
            );
        }
        return format!(
            "この投稿から動画を取得できませんでした。引用元ポストのURLであるか、画像のみのポストであるか、連携中アカウントで閲覧権限がない可能性があります。\n詳細: {short}"
        );
    }

    if full_lower.contains("operation not permitted")
        || full_lower.contains("permission denied")
        || full_lower.contains("keychain")
        || full_lower.contains("safe storage")
        || full_lower.contains("cookies.binarycookies")
        || full_lower.contains("failed to decrypt")
        || full_lower.contains("cannot decrypt v10 cookies")
    {
        return format!(
            "ブラウザの Cookie 読み込みに必要な権限がありません。\nOS の認証許可ダイアログ（macOS のキーチェーンや Safari のフルディスクアクセス等）をご確認いただくか、設定画面で cookies.txt ファイル指定をお試しください。\n詳細: {short}"
        );
    }

    if full_lower.contains("database is locked")
        || full_lower.contains("could not copy")
        || (full_lower.contains("could not find") && full_lower.contains("cookies"))
    {
        return format!(
            "ブラウザの Cookie データベースを開けませんでした。\nブラウザを終了してから再度お試しいただくか、設定画面で cookies.txt ファイル指定をご検討ください。\n詳細: {short}"
        );
    }

    if !has_ffmpeg
        && (full_lower.contains("ffmpeg is not installed")
            || full_lower.contains("you have requested merging")
            || full_lower.contains("aborting because ffmpeg")
            || full_lower.contains("m3u8"))
    {
        return format!(
            "この動画（HLS / 高画質分離形式）の結合には ffmpeg が必要です。\nffmpeg をインストール（macOS: brew install ffmpeg / Windows: winget install Gyan.FFmpeg / Linux: sudo apt install ffmpeg）してください。\n詳細: {short}"
        );
    }

    if full_lower.contains("file is larger than") || full_lower.contains("max-filesize") {
        return format!(
            "動画サイズが設定された上限を超えているため取り込めません（設定画面でサイズ上限を変更できます）。\n詳細: {short}"
        );
    }

    if full_lower.contains("unsupported url") {
        return format!("この URL は yt-dlp でサポートされていない形式です。\n詳細: {short}");
    }

    if full_lower.contains("requested format is not available") {
        if !has_ffmpeg {
            return format!(
                "直接ダウンロード可能な形式が見つかりませんでした。ffmpeg を導入すると HLS / 分離形式にも対応できます。\n詳細: {short}"
            );
        }
        return format!("利用可能な動画形式が見つかりませんでした。\n詳細: {short}");
    }

    if stage == "meta" {
        format!("URL の解析に失敗しました: {short}")
    } else {
        format!("動画の取得に失敗しました: {short}")
    }
}

pub(super) fn format_size_limit_label(max_video_size_mb: u32) -> String {
    if max_video_size_mb >= 1024 && max_video_size_mb.is_multiple_of(1024) {
        format!("{} GB", max_video_size_mb / 1024)
    } else {
        format!("{max_video_size_mb} MB")
    }
}
