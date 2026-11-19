use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use image::ImageFormat;

use crate::error::VaultError;
use crate::imaging::{
    encode_webp_thumbnail_with_quality, is_video_mime, FrameQuality, FrameQualityKind,
    GeneratedThumbnail,
};
use crate::models::RegenerateThumbnailResult;
use crate::state::AppState;
use crate::ytdlp::{
    build_augmented_path, configure_tool_command, resolve_ffmpeg_binary, run_child_monitored,
    ChildRunFailure, MonitorOptions,
};

#[derive(Debug)]
struct ExtractedVideoFrame {
    thumb: GeneratedThumbnail,
    orig_width: i32,
    orig_height: i32,
    duration_ms: i64,
    quality: FrameQuality,
}

struct FfmpegFrameAttempt {
    frame: Option<ExtractedVideoFrame>,
    parsed_duration_ms: Option<i64>,
}

pub fn parse_ffmpeg_duration_ms(stderr: &[u8]) -> Option<i64> {
    let text = String::from_utf8_lossy(stderr);
    for line in text.lines() {
        let Some(pos) = line.find("Duration:") else {
            continue;
        };
        let rest = line[pos + "Duration:".len()..].trim();
        let raw_dur = rest.split(',').next()?.trim();
        if raw_dur.eq_ignore_ascii_case("N/A") {
            continue;
        }
        let parts: Vec<&str> = raw_dur.split(':').collect();
        if parts.len() != 3 {
            continue;
        }
        let hours: f64 = parts[0].trim().parse().ok()?;
        let mins: f64 = parts[1].trim().parse().ok()?;
        let secs: f64 = parts[2].trim().parse().ok()?;
        if !hours.is_finite()
            || !mins.is_finite()
            || !secs.is_finite()
            || hours < 0.0
            || mins < 0.0
            || secs < 0.0
        {
            continue;
        }
        let total_sec = hours * 3600.0 + mins * 60.0 + secs;
        if total_sec > 0.0 {
            return Some((total_sec * 1000.0).round() as i64);
        }
    }
    None
}

pub fn build_candidate_timestamps_sec(duration_ms: i64) -> Vec<f64> {
    if duration_ms <= 0 {
        return vec![0.0];
    }
    let dur = duration_ms as f64 / 1000.0;
    let max_ts = (dur - 0.1).max(0.0);

    let (raw, min_gap) = if dur >= 5.0 {
        (
            vec![(dur * 0.12).min(15.0), dur * 0.25, dur * 0.50, 1.0, 0.0],
            0.4,
        )
    } else if dur >= 0.5 {
        (vec![dur * 0.25, dur * 0.50, 0.0], 0.2)
    } else {
        return vec![0.0];
    };

    let mut out = Vec::with_capacity(raw.len());
    for ts in raw {
        let clamped = ts.clamp(0.0, max_ts);
        if !out.iter().any(|&prev: &f64| (prev - clamped).abs() < min_gap) {
            out.push(clamped);
        }
    }
    if out.is_empty() {
        out.push(0.0);
    }
    out
}

fn run_ffmpeg_single_frame(
    ffmpeg_bin: &Path,
    media_url: &str,
    seek_sec: Option<f64>,
) -> Result<FfmpegFrameAttempt, VaultError> {
    let aug_path = build_augmented_path();
    let mut cmd = Command::new(ffmpeg_bin);
    configure_tool_command(&mut cmd, &aug_path);
    for key in [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ] {
        cmd.env_remove(key);
    }

    cmd.args([
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "info",
        "-protocol_whitelist",
        "http,tcp",
        "-seekable",
        "1",
    ]);
    if let Some(ts) = seek_sec {
        if ts > 0.01 {
            cmd.arg("-ss").arg(format!("{ts:.3}"));
        }
    }
    cmd.args([
        "-i",
        media_url,
        "-map",
        "0:v:0",
        "-frames:v",
        "1",
        "-vf",
        "scale='max(1,round(iw*sar))':ih,setsar=1",
        "-f",
        "image2pipe",
        "-vcodec",
        "png",
        "-compression_level",
        "1",
        "pipe:1",
    ]);

    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| VaultError::Io(std::io::Error::other(format!("ffmpeg 起動失敗: {e}"))))?;

    let cancel = AtomicBool::new(false);
    let noop = |_| {};
    let out = run_child_monitored(
        child,
        MonitorOptions {
            total_timeout: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(10),
            capture_stdout: true,
            stdout_cap: 48 * 1024 * 1024,
            cancel: &cancel,
            staging_watch: None,
            item_total_hint: None,
        },
        &noop,
    )
    .map_err(|e| match e {
        ChildRunFailure::Io(ioe) => VaultError::Io(ioe),
        ChildRunFailure::TotalTimeout | ChildRunFailure::StallTimeout => {
            VaultError::InvalidInput("ffmpeg のフレーム抽出がタイムアウトしました".to_string())
        }
        ChildRunFailure::MemoryLimitExceeded => {
            VaultError::InvalidInput("ffmpeg の出力サイズが上限を超えました".to_string())
        }
        ChildRunFailure::Cancelled | ChildRunFailure::StagingLimitExceeded => {
            VaultError::InvalidInput("ffmpeg の実行が中断されました".to_string())
        }
    })?;

    let parsed_duration_ms = parse_ffmpeg_duration_ms(&out.stderr);
    if !out.status.success() || out.stdout.is_empty() {
        return Ok(FfmpegFrameAttempt {
            frame: None,
            parsed_duration_ms,
        });
    }

    let Ok(img) = image::load_from_memory_with_format(&out.stdout, ImageFormat::Png) else {
        return Ok(FfmpegFrameAttempt {
            frame: None,
            parsed_duration_ms,
        });
    };

    let orig_width = img.width() as i32;
    let orig_height = img.height() as i32;
    let (thumb, quality) = encode_webp_thumbnail_with_quality(&img)?;

    Ok(FfmpegFrameAttempt {
        frame: Some(ExtractedVideoFrame {
            thumb,
            orig_width,
            orig_height,
            duration_ms: parsed_duration_ms.unwrap_or(0),
            quality,
        }),
        parsed_duration_ms,
    })
}

fn consider_candidate(best: &mut Option<ExtractedVideoFrame>, frame: ExtractedVideoFrame) -> bool {
    match frame.quality.kind {
        FrameQualityKind::BlankTransparent => false,
        FrameQualityKind::Good => {
            *best = Some(frame);
            true
        }
        FrameQualityKind::FlatOrDark | FrameQualityKind::StaticNoise => {
            let replace = best
                .as_ref()
                .is_none_or(|cur| frame.quality.score > cur.quality.score);
            if replace {
                *best = Some(frame);
            }
            false
        }
    }
}

fn extract_video_frame_with_ffmpeg(
    ffmpeg_bin: &Path,
    media_url: &str,
    known_duration_ms: i64,
    explicit_timestamp_ms: Option<i64>,
) -> Result<ExtractedVideoFrame, VaultError> {
    if let Some(ts_ms) = explicit_timestamp_ms {
        let ts_sec = (ts_ms.max(0) as f64) / 1000.0;
        let attempt = run_ffmpeg_single_frame(ffmpeg_bin, media_url, Some(ts_sec))?;
        let chosen = if let Some(frame) = attempt
            .frame
            .filter(|f| f.quality.kind != FrameQualityKind::BlankTransparent)
        {
            Some(frame)
        } else if ts_sec > 0.01 {
            run_ffmpeg_single_frame(ffmpeg_bin, media_url, None)?
                .frame
                .filter(|f| f.quality.kind != FrameQualityKind::BlankTransparent)
        } else {
            None
        };

        return match chosen {
            Some(mut frame) => {
                if frame.duration_ms == 0 && known_duration_ms > 0 {
                    frame.duration_ms = known_duration_ms;
                }
                Ok(frame)
            }
            None => Err(VaultError::InvalidInput(
                "指定位置から有効なフレームを取得できませんでした".to_string(),
            )),
        };
    }

    let mut best: Option<ExtractedVideoFrame> = None;

    if known_duration_ms > 0 {
        for ts in build_candidate_timestamps_sec(known_duration_ms) {
            let attempt = run_ffmpeg_single_frame(ffmpeg_bin, media_url, Some(ts))?;
            if let Some(frame) = attempt.frame {
                if consider_candidate(&mut best, frame) {
                    break;
                }
            }
        }
    } else {
        let first = run_ffmpeg_single_frame(ffmpeg_bin, media_url, Some(1.0))?;
        let parsed_dur = first.parsed_duration_ms;
        let found_good = first
            .frame
            .is_some_and(|frame| consider_candidate(&mut best, frame));

        if !found_good {
            let remaining = match parsed_dur {
                Some(dur_ms) => build_candidate_timestamps_sec(dur_ms)
                    .into_iter()
                    .filter(|&ts| (ts - 1.0).abs() >= 0.4)
                    .collect::<Vec<_>>(),
                None => vec![0.0],
            };
            for ts in remaining {
                let attempt = run_ffmpeg_single_frame(ffmpeg_bin, media_url, Some(ts))?;
                if let Some(frame) = attempt.frame {
                    if consider_candidate(&mut best, frame) {
                        break;
                    }
                }
            }
        }
    }

    match best {
        Some(mut chosen) => {
            if chosen.duration_ms == 0 && known_duration_ms > 0 {
                chosen.duration_ms = known_duration_ms;
            }
            Ok(chosen)
        }
        None => Err(VaultError::InvalidInput(
            "動画から有効なサムネイルフレームを取得できませんでした".to_string(),
        )),
    }
}

pub async fn regenerate_item_thumbnail_on_host(
    state: AppState,
    item_id: String,
    timestamp_ms: Option<i64>,
) -> Result<RegenerateThumbnailResult, VaultError> {
    let (item, ffmpeg_custom) = {
        let guard = state.lock_db()?;
        let db = guard.as_ref().ok_or(VaultError::VaultNotOpen)?;
        let item = db.get_thumbnail_regen_item(&item_id)?;
        let ffmpeg_custom = db.get_url_import_settings().ffmpeg_path;
        (item, ffmpeg_custom)
    };

    if !is_video_mime(&item.mime_type) && item.mime_type != "image/avif" {
        let state_clone = state.clone();
        let id_clone = item_id.clone();
        let detail = tokio::task::spawn_blocking(move || {
            let mut guard = state_clone.lock_db()?;
            let db = guard.as_mut().ok_or(VaultError::VaultNotOpen)?;
            db.regenerate_standard_image_thumbnail(&id_clone)
        })
        .await
        .map_err(|e| VaultError::InvalidInput(format!("バックグラウンド処理に失敗しました: {e}")))??;

        return Ok(RegenerateThumbnailResult {
            status: "updated".to_string(),
            item_id,
            mime_type: item.mime_type,
            detail: Some(detail),
        });
    }

    if item.mime_type == "image/avif" {
        return Ok(RegenerateThumbnailResult {
            status: "clientRequired".to_string(),
            item_id,
            mime_type: item.mime_type,
            detail: None,
        });
    }

    let Some(ffmpeg_bin) = resolve_ffmpeg_binary(&ffmpeg_custom) else {
        return Ok(RegenerateThumbnailResult {
            status: "clientRequired".to_string(),
            item_id,
            mime_type: item.mime_type,
            detail: None,
        });
    };

    let handle = state.get_or_start_local_media_server().await?;
    let media_url = format!("{}/media/{}?t={}", handle.base_url(), item_id, handle.token());
    let known_dur = item.duration_ms;

    let extract_res = tokio::task::spawn_blocking(move || {
        extract_video_frame_with_ffmpeg(&ffmpeg_bin, &media_url, known_dur, timestamp_ms)
    })
    .await
    .map_err(|e| VaultError::InvalidInput(format!("バックグラウンド処理に失敗しました: {e}")))?;

    match extract_res {
        Ok(frame) => {
            if !state.is_open() {
                return Err(VaultError::VaultNotOpen);
            }
            let state_save = state.clone();
            let id_save = item_id.clone();
            let detail = tokio::task::spawn_blocking(move || {
                let mut guard = state_save.lock_db()?;
                let db = guard.as_mut().ok_or(VaultError::VaultNotOpen)?;
                db.save_generated_thumbnail(
                    &id_save,
                    &frame.thumb,
                    frame.orig_width,
                    frame.orig_height,
                    frame.duration_ms,
                )
            })
            .await
            .map_err(|e| {
                VaultError::InvalidInput(format!("バックグラウンド処理に失敗しました: {e}"))
            })??;

            Ok(RegenerateThumbnailResult {
                status: "updated".to_string(),
                item_id,
                mime_type: item.mime_type,
                detail: Some(detail),
            })
        }
        Err(_) => {
            if !state.is_open() {
                return Err(VaultError::VaultNotOpen);
            }
            Ok(RegenerateThumbnailResult {
                status: "clientRequired".to_string(),
                item_id,
                mime_type: item.mime_type,
                detail: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use tempfile::TempDir;

    use super::*;
    use crate::db::{ImportOutcome, VaultDb};
    use crate::imaging::evaluate_frame_quality;

    #[test]
    fn test_parse_ffmpeg_duration_ms() {
        let stderr = b"Input #0, mov,mp4,m4a,3gp,3g2,mj2:\n  Duration: 00:01:23.45, start: 0.000000, bitrate: 1200 kb/s\n";
        assert_eq!(parse_ffmpeg_duration_ms(stderr), Some(83_450));

        let na = b"  Duration: N/A, start: 0.000000, bitrate: N/A\n";
        assert_eq!(parse_ffmpeg_duration_ms(na), None);
    }

    #[test]
    fn test_build_candidate_timestamps_sec() {
        let short = build_candidate_timestamps_sec(300);
        assert_eq!(short, vec![0.0]);

        let medium = build_candidate_timestamps_sec(2_000);
        assert_eq!(medium, vec![0.5, 1.0, 0.0]);

        let long = build_candidate_timestamps_sec(20_000);
        assert_eq!(long, vec![2.4, 5.0, 10.0, 1.0, 0.0]);
    }

    #[tokio::test]
    async fn test_regenerate_with_ffmpeg_and_standard_image() {
        let Some(ffmpeg_bin) = resolve_ffmpeg_binary("") else {
            return;
        };

        let dir = TempDir::new().unwrap();
        let mp4_path = dir.path().join("black_then_color.mp4");
        let status = Command::new(&ffmpeg_bin)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=640x360:d=2",
                "-f",
                "lavfi",
                "-i",
                "testsrc=s=640x360:d=3",
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&mp4_path)
            .status()
            .expect("run ffmpeg to build test mp4");
        assert!(status.success());

        let vault_path = dir.path().join("regen_test.vault");
        let mut db = VaultDb::create(&vault_path, "pass-1234", true).unwrap();

        let mp4_id = match db
            .import_media_file(&mp4_path, Some("IntroBlack"), None, 0)
            .unwrap()
        {
            ImportOutcome::Imported { id, .. } => id,
            _ => panic!("expected mp4 imported"),
        };

        // Also import a standard PNG
        let mut img = RgbaImage::new(480, 320);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = Rgba([(x % 255) as u8, (y % 255) as u8, 180, 255]);
        }
        let mut png_bytes = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
            .unwrap();
        let png_id = match db.import_bytes(&png_bytes, Some("SamplePng")).unwrap() {
            ImportOutcome::Imported { id, .. } => id,
            _ => panic!("expected png imported"),
        };

        let state = AppState::default();
        {
            let mut guard = state.lock_db().unwrap();
            *guard = Some(db);
        }

        // 1. Auto video thumbnail regen should skip the 2s black intro and pick a Good frame
        let res = regenerate_item_thumbnail_on_host(state.clone(), mp4_id.clone(), None)
            .await
            .unwrap();
        assert_eq!(res.status, "updated");
        let detail = res.detail.expect("detail present");
        assert!(detail.has_thumbnail);
        assert_eq!((detail.width, detail.height), (640, 360));
        assert!(detail.duration_ms >= 4500 && detail.duration_ms <= 5500);
        assert!(!detail.palette.is_empty());

        let thumb_bytes = {
            let guard = state.lock_db().unwrap();
            guard
                .as_ref()
                .unwrap()
                .get_thumbnail_bytes(&mp4_id)
                .unwrap()
        };
        let thumb_img = image::load_from_memory(&thumb_bytes).unwrap().to_rgba8();
        assert_eq!(evaluate_frame_quality(&thumb_img).kind, FrameQualityKind::Good);

        // 2. Explicit timestamp extraction
        let res_explicit =
            regenerate_item_thumbnail_on_host(state.clone(), mp4_id.clone(), Some(3500))
                .await
                .unwrap();
        assert_eq!(res_explicit.status, "updated");
        assert!(res_explicit.detail.unwrap().has_thumbnail);

        // 3. Standard PNG regeneration
        let res_png = regenerate_item_thumbnail_on_host(state.clone(), png_id.clone(), None)
            .await
            .unwrap();
        assert_eq!(res_png.status, "updated");
        let png_detail = res_png.detail.unwrap();
        assert!(png_detail.has_thumbnail);
        assert_eq!((png_detail.width, png_detail.height), (480, 320));

        state.close_vault();
    }
}
