use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::arc_cookies::{self, EphemeralCookieFile};
use crate::error::VaultError;
use crate::models::{UrlImportSettings, YtdlpStatus};

use super::is_valid_cookie_browser;
use super::process::{extract_ytdlp_error_line, run_child_monitored, MonitorOptions};

pub(super) fn build_augmented_path() -> String {
    let existing = env::var("PATH").unwrap_or_default();
    #[cfg(unix)]
    {
        let mut dirs: Vec<String> = vec![
            "/opt/homebrew/bin".to_string(),
            "/usr/local/bin".to_string(),
        ];
        if let Ok(home) = env::var("HOME") {
            if !home.trim().is_empty() {
                dirs.push(format!("{}/.local/bin", home.trim_end_matches('/')));
            }
        }
        if !existing.is_empty() {
            dirs.push(existing);
        }
        dirs.join(":")
    }
    #[cfg(windows)]
    {
        let mut dirs: Vec<PathBuf> = env::split_paths(&existing).collect();
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            let trimmed = local_app_data.trim();
            if !trimmed.is_empty() {
                dirs.push(
                    PathBuf::from(trimmed)
                        .join("Microsoft")
                        .join("WinGet")
                        .join("Links"),
                );
            }
        }
        if let Ok(user_profile) = env::var("USERPROFILE") {
            let trimmed = user_profile.trim();
            if !trimmed.is_empty() {
                dirs.push(PathBuf::from(trimmed).join("scoop").join("shims"));
            }
        }
        env::join_paths(dirs)
            .ok()
            .and_then(|s| s.into_string().ok())
            .unwrap_or(existing)
    }
}

pub(super) fn configure_tool_command(cmd: &mut Command, aug_path: &str) {
    cmd.env("PATH", aug_path)
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

fn resolve_binary(custom_path: &str, unix_name: &str, win_name: &str) -> Option<PathBuf> {
    let trimmed = custom_path.trim();
    if !trimmed.is_empty() {
        let p = PathBuf::from(trimmed);
        return if p.is_file() { Some(p) } else { None };
    }

    let aug_path = build_augmented_path();
    let bin_name = if cfg!(windows) { win_name } else { unix_name };

    for dir in env::split_paths(&aug_path) {
        let candidate = dir.join(bin_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    #[cfg(unix)]
    {
        for fallback_dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
            let p = Path::new(fallback_dir).join(unix_name);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    None
}

pub fn resolve_ytdlp_binary(custom_path: &str) -> Result<PathBuf, VaultError> {
    let trimmed = custom_path.trim();
    if let Some(p) = resolve_binary(trimmed, "yt-dlp", "yt-dlp.exe") {
        return Ok(p);
    }
    if !trimmed.is_empty() {
        return Err(VaultError::UrlImport(format!(
            "指定された yt-dlp が見つかりません: {trimmed}"
        )));
    }
    Err(VaultError::UrlImport(
        "yt-dlp が見つかりません。yt-dlp をインストール（macOS: brew install yt-dlp / Windows: winget install yt-dlp.yt-dlp / Linux: apt や公式サイト）し、設定画面でパスを確認してください"
            .to_string(),
    ))
}

pub fn resolve_ffmpeg_binary(custom_path: &str) -> Option<PathBuf> {
    resolve_binary(custom_path, "ffmpeg", "ffmpeg.exe")
}

pub(super) struct ResolvedCookies {
    pub(super) args: Vec<String>,
    pub(super) has_cookie: bool,
    pub(super) _ephemeral: Option<EphemeralCookieFile>,
}

pub(super) fn prepare_cookie_args(
    settings: &UrlImportSettings,
    target_url: &str,
    cancel: &AtomicBool,
) -> Result<ResolvedCookies, VaultError> {
    let clean = settings.cookie_browser.trim().to_ascii_lowercase();
    if !is_valid_cookie_browser(&clean) {
        return Err(VaultError::UrlImport(
            "非対応の Cookie 連携先が指定されています".to_string(),
        ));
    }
    if clean.is_empty() {
        return Ok(ResolvedCookies {
            args: Vec::new(),
            has_cookie: false,
            _ephemeral: None,
        });
    }
    if clean == "arc" {
        let ephemeral = arc_cookies::prepare_arc_cookie_file(target_url, cancel)?;
        let path_str = ephemeral.path().display().to_string();
        return Ok(ResolvedCookies {
            args: vec!["--cookies".to_string(), path_str],
            has_cookie: true,
            _ephemeral: Some(ephemeral),
        });
    }
    if clean == "file" {
        let cf = settings.cookies_file.trim();
        if cf.is_empty() {
            return Err(VaultError::UrlImport(
                "cookies.txt ファイルが設定されていません。設定画面から選択してください"
                    .to_string(),
            ));
        }
        if !Path::new(cf).is_file() {
            return Err(VaultError::UrlImport(format!(
                "指定された cookies.txt ファイルが見つかりません: {cf}"
            )));
        }
        return Ok(ResolvedCookies {
            args: vec!["--cookies".to_string(), cf.to_string()],
            has_cookie: true,
            _ephemeral: None,
        });
    }

    Ok(ResolvedCookies {
        args: vec!["--cookies-from-browser".to_string(), clean],
        has_cookie: true,
        _ephemeral: None,
    })
}

fn detect_tool_version(bin: &Path, args: &[&str]) -> (bool, Option<String>, Option<String>) {
    let aug = build_augmented_path();
    let mut cmd = Command::new(bin);
    configure_tool_command(&mut cmd, &aug);
    let child_res = cmd
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let Ok(child) = child_res else {
        return (
            false,
            None,
            Some("実行ファイルの起動に失敗しました".to_string()),
        );
    };

    let cancel = AtomicBool::new(false);
    let noop = |_| {};
    match run_child_monitored(
        child,
        MonitorOptions {
            total_timeout: Duration::from_secs(10),
            stall_timeout: Duration::from_secs(10),
            capture_stdout: true,
            stdout_cap: 16 * 1024,
            cancel: &cancel,
            staging_watch: None,
            item_total_hint: None,
        },
        &noop,
    ) {
        Ok(out) if out.status.success() => {
            let first_line = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            (
                true,
                if first_line.is_empty() {
                    None
                } else {
                    Some(first_line)
                },
                None,
            )
        }
        Ok(out) => (false, None, Some(extract_ytdlp_error_line(&out.stderr))),
        Err(_) => (false, None, Some("確認がタイムアウトしました".to_string())),
    }
}

pub fn check_ytdlp_status(custom_path: &str, custom_ffmpeg_path: &str) -> YtdlpStatus {
    let (ffmpeg_available, ffmpeg_path, ffmpeg_version) =
        match resolve_ffmpeg_binary(custom_ffmpeg_path) {
            Some(ff_bin) => {
                let (ok, raw_ver, _) = detect_tool_version(&ff_bin, &["-version"]);
                let ver = raw_ver.map(|line| {
                    line.strip_prefix("ffmpeg version ")
                        .and_then(|s| s.split_whitespace().next())
                        .unwrap_or(&line)
                        .to_string()
                });
                (ok, Some(ff_bin.display().to_string()), ver)
            }
            None => (false, None, None),
        };

    match resolve_ytdlp_binary(custom_path) {
        Ok(bin) => {
            let (available, version, error) = detect_tool_version(&bin, &["--version"]);
            YtdlpStatus {
                available,
                resolved_path: Some(bin.display().to_string()),
                version,
                error,
                ffmpeg_available,
                ffmpeg_path,
                ffmpeg_version,
            }
        }
        Err(e) => YtdlpStatus {
            available: false,
            resolved_path: None,
            version: None,
            error: Some(e.to_string()),
            ffmpeg_available,
            ffmpeg_path,
            ffmpeg_version,
        },
    }
}
