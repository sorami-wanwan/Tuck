use rusqlite::{params, OptionalExtension};

use crate::error::VaultError;
use crate::models::{LockSettings, UrlImportSettings};

use super::VaultDb;

impl VaultDb {
    pub fn get_lock_settings(&self) -> LockSettings {
        let mut settings = LockSettings::default();
        if let Ok(Some(shortcut)) = self
            .conn
            .query_row(
                "SELECT value FROM vault_meta WHERE key = 'lock_shortcut'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
        {
            if !shortcut.trim().is_empty() {
                settings.shortcut = shortcut;
            }
        }
        if let Ok(Some(mins_str)) = self
            .conn
            .query_row(
                "SELECT value FROM vault_meta WHERE key = 'auto_lock_minutes'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
        {
            if let Ok(mins) = mins_str.parse::<u32>() {
                settings.idle_timeout_minutes = mins;
            }
        }
        settings
    }

    pub fn set_lock_settings(
        &mut self,
        settings: &LockSettings,
    ) -> Result<LockSettings, VaultError> {
        let shortcut = settings.shortcut.trim().to_string();
        if shortcut.is_empty() {
            return Err(VaultError::InvalidInput(
                "即時ロックのショートカットキーを設定してください".to_string(),
            ));
        }
        let mins = settings.idle_timeout_minutes.min(120);
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO vault_meta (key, value) VALUES ('lock_shortcut', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![shortcut],
        )?;
        tx.execute(
            "INSERT INTO vault_meta (key, value) VALUES ('auto_lock_minutes', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![mins.to_string()],
        )?;
        tx.commit()?;
        Ok(LockSettings {
            shortcut,
            idle_timeout_minutes: mins,
        })
    }

    fn get_meta_opt(&self, key: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT value FROM vault_meta WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn get_url_import_settings(&self) -> UrlImportSettings {
        let mut settings = UrlImportSettings::default();
        if let Some(enabled_str) = self.get_meta_opt("url_import_enabled") {
            settings.enabled =
                enabled_str.trim() == "1" || enabled_str.eq_ignore_ascii_case("true");
        }
        if let Some(path_str) = self.get_meta_opt("ytdlp_path") {
            settings.ytdlp_path = path_str.trim().to_string();
        }
        if let Some(ffmpeg_str) = self.get_meta_opt("ffmpeg_path") {
            settings.ffmpeg_path = ffmpeg_str.trim().to_string();
        }
        if let Some(browser_str) = self.get_meta_opt("ytdlp_cookie_browser") {
            let clean = browser_str.trim().to_ascii_lowercase();
            if crate::ytdlp::is_valid_cookie_browser(&clean) {
                settings.cookie_browser = clean;
            }
        }
        if let Some(cookies_str) = self.get_meta_opt("ytdlp_cookies_file") {
            settings.cookies_file = cookies_str.trim().to_string();
        }
        if let Some(size_str) = self.get_meta_opt("ytdlp_max_video_size_mb") {
            if let Ok(mb) = size_str.trim().parse::<u32>() {
                settings.max_video_size_mb = if mb == 0 { 0 } else { mb.clamp(64, 16384) };
            }
        }
        if let Some(height_str) = self.get_meta_opt("ytdlp_max_video_height") {
            if let Ok(h) = height_str.trim().parse::<u32>() {
                settings.max_video_height = match h {
                    480 | 720 | 1080 | 1440 | 2160 => h,
                    _ => 0,
                };
            }
        }
        settings
    }

    pub fn set_url_import_settings(
        &mut self,
        settings: &UrlImportSettings,
    ) -> Result<UrlImportSettings, VaultError> {
        let clean_path = settings.ytdlp_path.trim().to_string();
        let clean_ffmpeg = settings.ffmpeg_path.trim().to_string();
        let clean_browser = settings.cookie_browser.trim().to_ascii_lowercase();
        if !crate::ytdlp::is_valid_cookie_browser(&clean_browser) {
            return Err(VaultError::InvalidInput(
                "非対応の Cookie 連携先が指定されました".to_string(),
            ));
        }
        let clean_cookies_file = settings.cookies_file.trim().to_string();
        if clean_cookies_file.chars().any(|c| c.is_control()) {
            return Err(VaultError::InvalidInput(
                "cookies.txt ファイルのパスが不正です".to_string(),
            ));
        }
        if clean_browser == "file" && clean_cookies_file.is_empty() {
            return Err(VaultError::InvalidInput(
                "cookies.txt ファイルのパスを指定してください".to_string(),
            ));
        }
        let max_video_size_mb = if settings.max_video_size_mb == 0 {
            0
        } else {
            settings.max_video_size_mb.clamp(64, 16384)
        };
        let max_video_height = match settings.max_video_height {
            480 | 720 | 1080 | 1440 | 2160 => settings.max_video_height,
            _ => 0,
        };

        let tx = self.conn.transaction()?;
        let pairs = [
            (
                "url_import_enabled",
                if settings.enabled { "1" } else { "0" }.to_string(),
            ),
            ("ytdlp_path", clean_path.clone()),
            ("ffmpeg_path", clean_ffmpeg.clone()),
            ("ytdlp_cookie_browser", clean_browser.clone()),
            ("ytdlp_cookies_file", clean_cookies_file.clone()),
            ("ytdlp_max_video_size_mb", max_video_size_mb.to_string()),
            ("ytdlp_max_video_height", max_video_height.to_string()),
        ];
        for (k, v) in pairs {
            tx.execute(
                "INSERT INTO vault_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v],
            )?;
        }
        tx.commit()?;
        Ok(UrlImportSettings {
            enabled: settings.enabled,
            ytdlp_path: clean_path,
            ffmpeg_path: clean_ffmpeg,
            cookie_browser: clean_browser,
            cookies_file: clean_cookies_file,
            max_video_size_mb,
            max_video_height,
        })
    }

    pub fn get_skip_plain_export_confirm(&self) -> bool {
        self.get_meta_opt("skip_plain_export_confirm")
            .is_some_and(|v| v.trim() == "1" || v.eq_ignore_ascii_case("true"))
    }

    pub fn set_skip_plain_export_confirm(&mut self, skip: bool) -> Result<bool, VaultError> {
        self.conn.execute(
            "INSERT INTO vault_meta (key, value) VALUES ('skip_plain_export_confirm', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![if skip { "1" } else { "0" }],
        )?;
        Ok(skip)
    }
}
