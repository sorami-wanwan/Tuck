use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
#[cfg(target_os = "macos")]
use std::sync::atomic::Ordering;

#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
#[cfg(any(target_os = "macos", test))]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", test))]
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
#[cfg(any(target_os = "macos", test))]
use pbkdf2::pbkdf2_hmac;
#[cfg(any(target_os = "macos", test))]
use rusqlite::{params_from_iter, types::Value, Connection, OpenFlags};
#[cfg(any(target_os = "macos", test))]
use sha1::Sha1;
#[cfg(any(target_os = "macos", test))]
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "macos", test))]
use uuid::Uuid;
#[cfg(any(target_os = "macos", test))]
use zeroize::Zeroizing;

use crate::error::VaultError;

#[cfg(any(target_os = "macos", test))]
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecryptedCookie {
    pub host_key: String,
    pub name: String,
    pub value: Zeroizing<String>,
    pub path: String,
    pub is_secure: bool,
    pub expires_utc: i64,
    pub last_access_utc: i64,
}

pub struct EphemeralCookieFile {
    path: PathBuf,
}

impl EphemeralCookieFile {
    #[cfg(any(target_os = "macos", test))]
    pub fn create(contents: &str) -> Result<Self, VaultError> {
        let path = std::env::temp_dir().join(format!("tuck-ck-{}.txt", Uuid::new_v4()));
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?
        };
        #[cfg(not(unix))]
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;

        let guard = Self { path: path.clone() };
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        let _ = file.sync_all();
        Ok(guard)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for EphemeralCookieFile {
    fn drop(&mut self) {
        if let Ok(meta) = fs::metadata(&self.path) {
            let len = meta.len().min(4 * 1024 * 1024) as usize;
            if len > 0 {
                if let Ok(mut f) = OpenOptions::new().write(true).open(&self.path) {
                    let zeros = vec![0u8; len];
                    let _ = f.write_all(&zeros);
                    let _ = f.flush();
                    let _ = f.sync_all();
                }
            }
        }
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(any(target_os = "macos", test))]
mod macos_impl {
    use super::*;

    struct TempSqliteCopyDir {
        dir: PathBuf,
    }

    impl TempSqliteCopyDir {
        fn new() -> Result<Self, VaultError> {
            let dir = std::env::temp_dir().join(format!("tuck-arc-db-{}", Uuid::new_v4()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder.create(&dir)?;
            }
            #[cfg(not(unix))]
            {
                fs::create_dir(&dir)?;
            }
            Ok(Self { dir })
        }

        fn path(&self) -> &Path {
            &self.dir
        }
    }

    impl Drop for TempSqliteCopyDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn extract_url_host(raw_url: &str) -> String {
        let trimmed = raw_url.trim();
        let after_scheme = trimmed
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(trimmed);
        let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
        let host_port = authority
            .rsplit_once('@')
            .map(|(_, h)| h)
            .unwrap_or(authority);
        let host = if host_port.starts_with('[') {
            host_port
                .split(']')
                .next()
                .unwrap_or(host_port)
                .trim_start_matches('[')
        } else {
            host_port.split(':').next().unwrap_or(host_port)
        };
        host.trim().trim_end_matches('.').to_ascii_lowercase()
    }

    fn registrable_base_domain(host: &str) -> String {
        if host.is_empty() || host.parse::<std::net::IpAddr>().is_ok() {
            return String::new();
        }
        let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
        if parts.len() <= 2 {
            return parts.join(".");
        }
        let last_two = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        let second_level_tlds = [
            "co.jp", "ne.jp", "or.jp", "ac.jp", "go.jp", "co.uk", "org.uk", "ac.uk", "com.au",
            "net.au", "org.au", "co.kr", "com.tw", "com.cn", "com.br", "com.mx", "co.nz", "co.in",
            "com.sg", "com.hk", "co.id", "com.th", "com.vn",
        ];
        if second_level_tlds.contains(&last_two.as_str()) && parts.len() >= 3 {
            format!(
                "{}.{}.{}",
                parts[parts.len() - 3],
                parts[parts.len() - 2],
                parts[parts.len() - 1]
            )
        } else {
            last_two
        }
    }

    fn host_matches(host: &str, domain: &str) -> bool {
        host == domain || host.ends_with(&format!(".{domain}"))
    }

    pub(super) fn target_cookie_domains(raw_url: &str) -> Vec<String> {
        let host = extract_url_host(raw_url);
        if host.is_empty() || host.parse::<std::net::IpAddr>().is_ok() {
            return Vec::new();
        }
        if host_matches(&host, "x.com")
            || host_matches(&host, "twitter.com")
            || host_matches(&host, "twimg.com")
            || host_matches(&host, "t.co")
        {
            return vec![
                "x.com".to_string(),
                "twitter.com".to_string(),
                "twimg.com".to_string(),
            ];
        }
        if host_matches(&host, "tiktok.com") || host_matches(&host, "tiktokv.com") {
            return vec!["tiktok.com".to_string(), "tiktokv.com".to_string()];
        }
        let base = registrable_base_domain(&host);
        if base.is_empty() {
            vec![host]
        } else {
            vec![base]
        }
    }

    pub(super) fn derive_chromium_mac_key(password: &[u8]) -> Zeroizing<[u8; 16]> {
        let mut key = Zeroizing::new([0u8; 16]);
        pbkdf2_hmac::<Sha1>(password, b"saltysalt", 1003, &mut *key);
        key
    }

    pub(super) fn decrypt_chromium_v10_cookie(
        encrypted_value: &[u8],
        key: &[u8; 16],
        meta_version: i64,
        host_key: &str,
    ) -> Option<Zeroizing<String>> {
        if encrypted_value.is_empty() {
            return None;
        }
        if !encrypted_value.starts_with(b"v10") {
            let s = std::str::from_utf8(encrypted_value).ok()?;
            if s.chars().any(|c| c == '\r' || c == '\n' || c == '\0') {
                return None;
            }
            return Some(Zeroizing::new(s.to_string()));
        }

        let ciphertext = &encrypted_value[3..];
        if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(16) {
            return None;
        }

        let iv = [0x20u8; 16];
        let mut buf = Zeroizing::new(ciphertext.to_vec());
        let dec = Aes128CbcDec::new_from_slices(key, &iv).ok()?;
        let plain = dec.decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?;

        let payload = if meta_version >= 24 {
            if plain.len() < 32 {
                return None;
            }
            let expected_hash = Sha256::digest(host_key.as_bytes());
            if plain[..32] != expected_hash[..] {
                return None;
            }
            &plain[32..]
        } else {
            plain
        };

        let s = std::str::from_utf8(payload).ok()?;
        if s.chars().any(|c| c == '\r' || c == '\n' || c == '\0') {
            return None;
        }
        Some(Zeroizing::new(s.to_string()))
    }

    fn current_chromium_utc_micros() -> i64 {
        let unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        (unix_secs + 11_644_473_600) * 1_000_000
    }

    fn chromium_expires_to_unix(expires_utc: i64) -> i64 {
        if expires_utc <= 0 {
            0
        } else {
            ((expires_utc / 1_000_000) - 11_644_473_600).max(0)
        }
    }

    fn has_forbidden_netscape_chars(s: &str) -> bool {
        s.chars()
            .any(|c| c == '\r' || c == '\n' || c == '\t' || c == '\0')
    }

    pub(super) fn format_netscape_cookies(cookies: &[DecryptedCookie]) -> Zeroizing<String> {
        let mut out = Zeroizing::new(String::from("# Netscape HTTP Cookie File\n"));
        for c in cookies {
            let domain = c.host_key.trim();
            let name = c.name.trim();
            let path = if c.path.is_empty() {
                "/"
            } else {
                c.path.as_str()
            };
            if domain.is_empty()
                || name.is_empty()
                || has_forbidden_netscape_chars(domain)
                || has_forbidden_netscape_chars(name)
                || has_forbidden_netscape_chars(&c.value)
                || has_forbidden_netscape_chars(path)
            {
                continue;
            }
            let include_subdomains = if domain.starts_with('.') {
                "TRUE"
            } else {
                "FALSE"
            };
            let secure = if c.is_secure { "TRUE" } else { "FALSE" };
            let exp = chromium_expires_to_unix(c.expires_utc);
            out.push_str(&format!(
                "{domain}\t{include_subdomains}\t{path}\t{secure}\t{exp}\t{name}\t{}\n",
                c.value.as_str()
            ));
        }
        out
    }

    fn escape_like_literal(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            match ch {
                '\\' | '%' | '_' => {
                    out.push('\\');
                    out.push(ch);
                }
                _ => out.push(ch),
            }
        }
        out
    }

    fn collect_cookie_db_candidates(user_data_dir: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for rel in ["Cookies", "Network/Cookies"] {
            let p = user_data_dir.join(rel);
            if p.is_file() {
                paths.push(p);
            }
        }
        if let Ok(entries) = fs::read_dir(user_data_dir) {
            let mut subdirs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            subdirs.sort();
            for sub in subdirs {
                for rel in ["Cookies", "Network/Cookies"] {
                    let p = sub.join(rel);
                    if p.is_file() {
                        paths.push(p);
                    }
                }
            }
        }
        paths
    }

    fn copy_sqlite_with_sidecars(src_db: &Path, dest_dir: &Path) -> Result<PathBuf, VaultError> {
        let dest_db = dest_dir.join("Cookies");
        fs::copy(src_db, &dest_db)?;
        for suffix in ["-wal", "-shm"] {
            let src_sidecar = PathBuf::from(format!("{}{}", src_db.display(), suffix));
            if src_sidecar.is_file() {
                let dest_sidecar = dest_dir.join(format!("Cookies{suffix}"));
                let _ = fs::copy(&src_sidecar, &dest_sidecar);
            }
        }
        Ok(dest_db)
    }

    fn value_as_string(val: Value) -> String {
        match val {
            Value::Text(s) => s,
            Value::Blob(b) => String::from_utf8_lossy(&b).into_owned(),
            Value::Integer(n) => n.to_string(),
            Value::Real(f) => f.to_string(),
            Value::Null => String::new(),
        }
    }

    fn value_as_bytes(val: Value) -> Vec<u8> {
        match val {
            Value::Blob(b) => b,
            Value::Text(s) => s.into_bytes(),
            _ => Vec::new(),
        }
    }

    fn read_profile_cookies(
        db_path: &Path,
        domains: &[String],
        key: &[u8; 16],
    ) -> Result<(Vec<DecryptedCookie>, usize), VaultError> {
        if domains.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let tmp_dir = TempSqliteCopyDir::new()?;
        let copied_db = copy_sqlite_with_sidecars(db_path, tmp_dir.path())?;
        let conn = Connection::open_with_flags(
            &copied_db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        let meta_version: i64 = conn
            .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
                r.get::<_, Value>(0)
            })
            .ok()
            .and_then(|v| value_as_string(v).trim().parse::<i64>().ok())
            .unwrap_or(24);

        let mut has_is_secure = false;
        let mut has_secure = false;
        let mut has_last_access = false;
        {
            let mut pragma = conn.prepare("PRAGMA table_info(cookies)")?;
            let cols = pragma.query_map([], |r| r.get::<_, String>(1))?;
            for col in cols.flatten() {
                match col.as_str() {
                    "is_secure" => has_is_secure = true,
                    "secure" => has_secure = true,
                    "last_access_utc" => has_last_access = true,
                    _ => {}
                }
            }
        }

        let secure_expr = if has_is_secure {
            "is_secure"
        } else if has_secure {
            "secure"
        } else {
            "0"
        };
        let last_access_expr = if has_last_access {
            "last_access_utc"
        } else {
            "0"
        };

        let mut clauses = Vec::with_capacity(domains.len());
        let mut params_vec: Vec<Value> = Vec::with_capacity(domains.len() * 3);
        for d in domains {
            clauses
                .push("(host_key = ? OR host_key = ? OR host_key LIKE ? ESCAPE '\\')".to_string());
            params_vec.push(Value::Text(d.clone()));
            params_vec.push(Value::Text(format!(".{d}")));
            params_vec.push(Value::Text(format!("%.{}", escape_like_literal(d))));
        }

        let sql = format!(
            "SELECT host_key, name, value, encrypted_value, path, expires_utc, {secure_expr}, {last_access_expr}
             FROM cookies
             WHERE {}",
            clauses.join(" OR ")
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params_vec))?;

        let now_chromium = current_chromium_utc_micros();
        let mut out = Vec::new();
        let mut decrypt_failures = 0usize;
        while let Some(row) = rows.next()? {
            let host_key = value_as_string(row.get::<_, Value>(0)?);
            let name = value_as_string(row.get::<_, Value>(1)?);
            let plain_val = value_as_string(row.get::<_, Value>(2)?);
            let enc_val = value_as_bytes(row.get::<_, Value>(3)?);
            let path = value_as_string(row.get::<_, Value>(4)?);
            let expires_utc = row.get::<_, i64>(5).unwrap_or(0);
            let is_secure = row.get::<_, i64>(6).unwrap_or(0) != 0;
            let last_access_utc = row.get::<_, i64>(7).unwrap_or(0);

            if expires_utc > 0 && expires_utc <= now_chromium {
                continue;
            }

            let value = if !plain_val.is_empty() {
                Zeroizing::new(plain_val)
            } else if !enc_val.is_empty() {
                match decrypt_chromium_v10_cookie(&enc_val, key, meta_version, &host_key) {
                    Some(v) => v,
                    None => {
                        decrypt_failures += 1;
                        continue;
                    }
                }
            } else {
                Zeroizing::new(String::new())
            };

            if value.is_empty() {
                continue;
            }

            out.push(DecryptedCookie {
                host_key,
                name,
                value,
                path,
                is_secure,
                expires_utc,
                last_access_utc,
            });
        }

        Ok((out, decrypt_failures))
    }

    fn has_site_auth_cookie(cookies: &[DecryptedCookie], domains: &[String]) -> bool {
        if domains.iter().any(|d| d == "x.com" || d == "twitter.com") {
            return cookies
                .iter()
                .any(|c| c.name == "auth_token" && !c.value.is_empty());
        }
        if domains.iter().any(|d| d == "tiktok.com") {
            return cookies
                .iter()
                .any(|c| (c.name == "sessionid" || c.name == "sid_tt") && !c.value.is_empty());
        }
        !cookies.is_empty()
    }

    pub(super) fn extract_arc_cookies_from_user_data_dir(
        user_data_dir: &Path,
        target_url: &str,
        key: &[u8; 16],
    ) -> Result<Vec<DecryptedCookie>, VaultError> {
        let domains = target_cookie_domains(target_url);
        if domains.is_empty() {
            return Err(VaultError::UrlImport(
                "Cookie の抽出対象ドメインをURLから特定できませんでした".to_string(),
            ));
        }
        if !user_data_dir.is_dir() {
            return Err(VaultError::UrlImport(
                "Arc Browser のデータフォルダが見つかりません（設定画面で cookies.txt 指定もお試しください）"
                    .to_string(),
            ));
        }

        let candidates = collect_cookie_db_candidates(user_data_dir);
        if candidates.is_empty() {
            return Err(VaultError::UrlImport(
                "Arc Browser の Cookie データベースが見つかりません（設定画面で cookies.txt 指定もお試しください）"
                    .to_string(),
            ));
        }

        let mut best_score: Option<(bool, i64, u128, usize)> = None;
        let mut best_cookies: Vec<DecryptedCookie> = Vec::new();
        let mut total_decrypt_failures = 0usize;

        for db_path in candidates {
            let mtime_ms = fs::metadata(&db_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0);

            let Ok((cookies, failures)) = read_profile_cookies(&db_path, &domains, key) else {
                continue;
            };
            total_decrypt_failures += failures;
            if cookies.is_empty() {
                continue;
            }

            let is_auth = has_site_auth_cookie(&cookies, &domains);
            let max_access = cookies.iter().map(|c| c.last_access_utc).max().unwrap_or(0);
            let score = (is_auth, max_access, mtime_ms, cookies.len());
            if best_score.is_none_or(|cur| score > cur) {
                best_score = Some(score);
                best_cookies = cookies;
            }
        }

        if best_cookies.is_empty() {
            if total_decrypt_failures > 0 {
                return Err(VaultError::UrlImport(
                    "Arc Browser の Cookie 復号に失敗しました。\nキーチェーンアクセスが許可されているか確認するか、設定画面で cookies.txt 指定をお試しください"
                        .to_string(),
                ));
            }
            return Err(VaultError::UrlImport(format!(
                "Arc Browser 内に {} 用の Cookie が見つかりません。\nArc で該当サイトにログイン済みか確認してください",
                domains[0]
            )));
        }

        Ok(best_cookies)
    }
}

#[cfg(any(target_os = "macos", test))]
use macos_impl::*;

#[cfg(target_os = "macos")]
enum KeychainLookupError {
    ItemNotFound(VaultError),
    Abort(VaultError),
}

#[cfg(target_os = "macos")]
fn run_security_find_password(
    args: &[&str],
    cancel: &AtomicBool,
) -> Result<Zeroizing<Vec<u8>>, KeychainLookupError> {
    let mut child = Command::new("/usr/bin/security")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            KeychainLookupError::Abort(VaultError::UrlImport(format!(
                "macOS キーチェーン照会（/usr/bin/security）の起動に失敗しました: {e}"
            )))
        })?;

    let started = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(KeychainLookupError::Abort(VaultError::UrlImport(
                "取り込みをキャンセルしました".to_string(),
            )));
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|e| KeychainLookupError::Abort(VaultError::Io(e)))?
        {
            if !status.success() {
                let mut stderr_buf = Vec::new();
                if let Some(mut stderr) = child.stderr.take() {
                    use std::io::Read;
                    let _ = stderr.read_to_end(&mut stderr_buf);
                }
                let stderr_text = String::from_utf8_lossy(&stderr_buf).to_ascii_lowercase();
                let err = VaultError::UrlImport(
                    "Arc Browser のキーチェーン（Arc Safe Storage）へのアクセスに失敗しました。\nmacOS の許可ダイアログで「常に許可」を選択するか、設定画面で cookies.txt をお試しください"
                        .to_string(),
                );
                if status.code() == Some(44)
                    || stderr_text.contains("could not be found in the keychain")
                {
                    return Err(KeychainLookupError::ItemNotFound(err));
                }
                return Err(KeychainLookupError::Abort(err));
            }
            let mut out = Zeroizing::new(Vec::new());
            if let Some(mut stdout) = child.stdout.take() {
                use std::io::Read;
                let _ = stdout.read_to_end(&mut out);
            }
            while matches!(out.last(), Some(b'\n' | b'\r')) {
                out.pop();
            }
            if out.is_empty() {
                return Err(KeychainLookupError::Abort(VaultError::UrlImport(
                    "Arc Browser のキーチェーン（Arc Safe Storage）から鍵を取得できませんでした"
                        .to_string(),
                )));
            }
            return Ok(out);
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(KeychainLookupError::Abort(VaultError::UrlImport(
                "macOS キーチェーンの承認待ちがタイムアウトしました（60秒）".to_string(),
            )));
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

#[cfg(target_os = "macos")]
fn fetch_arc_safe_storage_password(cancel: &AtomicBool) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    match run_security_find_password(
        &[
            "find-generic-password",
            "-w",
            "-a",
            "Arc",
            "-s",
            "Arc Safe Storage",
        ],
        cancel,
    ) {
        Ok(pw) => Ok(pw),
        Err(KeychainLookupError::Abort(err)) => Err(err),
        Err(KeychainLookupError::ItemNotFound(err)) => {
            if cancel.load(Ordering::SeqCst) {
                return Err(err);
            }
            run_security_find_password(
                &["find-generic-password", "-w", "-s", "Arc Safe Storage"],
                cancel,
            )
            .map_err(|_| err)
        }
    }
}

#[cfg(target_os = "macos")]
pub fn prepare_arc_cookie_file(
    target_url: &str,
    cancel: &AtomicBool,
) -> Result<EphemeralCookieFile, VaultError> {
    let home = std::env::var("HOME").map_err(|_| {
        VaultError::UrlImport(
            "ホームディレクトリを特定できないため Arc Cookie を読み込めません".to_string(),
        )
    })?;
    let user_data_dir = PathBuf::from(home).join("Library/Application Support/Arc/User Data");
    if !user_data_dir.is_dir() {
        return Err(VaultError::UrlImport(
            "Arc Browser のデータフォルダが見つかりません（設定画面で他のブラウザや cookies.txt をお試しください）"
                .to_string(),
        ));
    }

    let password = fetch_arc_safe_storage_password(cancel)?;
    let key = derive_chromium_mac_key(&password);
    drop(password);

    let cookies = extract_arc_cookies_from_user_data_dir(&user_data_dir, target_url, &key)?;
    drop(key);

    let netscape = format_netscape_cookies(&cookies);
    EphemeralCookieFile::create(&netscape)
}

#[cfg(not(target_os = "macos"))]
pub fn prepare_arc_cookie_file(
    _target_url: &str,
    _cancel: &AtomicBool,
) -> Result<EphemeralCookieFile, VaultError> {
    Err(VaultError::UrlImport(
        "Arc Browser 直接連携は macOS のみ対応しています。設定画面で cookies.txt ファイル指定をご利用ください"
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    use tempfile::TempDir;

    type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

    fn make_synthetic_v10_cookie(
        key: &[u8; 16],
        host_sha256_prefix: bool,
        host_key: &str,
        plaintext_val: &str,
    ) -> Vec<u8> {
        let mut plain = Vec::new();
        if host_sha256_prefix {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(host_key.as_bytes());
            plain.extend_from_slice(&h.finalize());
        }
        plain.extend_from_slice(plaintext_val.as_bytes());

        let iv = [0x20u8; 16];
        let msg_len = plain.len();
        let mut buf = vec![0u8; ((msg_len / 16) + 1) * 16];
        buf[..msg_len].copy_from_slice(&plain);
        let enc_len = Aes128CbcEnc::new_from_slices(key, &iv)
            .unwrap()
            .encrypt_padded_mut::<Pkcs7>(&mut buf, msg_len)
            .unwrap()
            .len();

        let mut out = b"v10".to_vec();
        out.extend_from_slice(&buf[..enc_len]);
        out
    }

    type CookieFixtureRow<'a> = (&'a str, &'a str, Vec<u8>, &'a str, i64, i64, i64);

    fn create_synthetic_cookies_db(
        db_path: &Path,
        meta_version: i64,
        rows: &[CookieFixtureRow<'_>],
    ) {
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
             CREATE TABLE cookies (
                creation_utc INTEGER NOT NULL,
                host_key TEXT NOT NULL,
                name TEXT NOT NULL,
                value TEXT NOT NULL,
                path TEXT NOT NULL,
                expires_utc INTEGER NOT NULL,
                is_secure INTEGER NOT NULL,
                last_access_utc INTEGER NOT NULL,
                encrypted_value BLOB
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('version', ?1)",
            rusqlite::params![meta_version.to_string()],
        )
        .unwrap();
        for (host, name, enc, path, exp, sec, access) in rows {
            conn.execute(
                "INSERT INTO cookies (creation_utc, host_key, name, value, path, expires_utc, is_secure, last_access_utc, encrypted_value)
                 VALUES (1, ?1, ?2, '', ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![host, name, path, exp, sec, access, enc],
            )
            .unwrap();
        }
    }

    #[test]
    fn test_target_cookie_domains() {
        assert_eq!(
            target_cookie_domains("https://x.com/user/status/12345"),
            vec!["x.com", "twitter.com", "twimg.com"]
        );
        assert_eq!(
            target_cookie_domains("https://mobile.twitter.com/i/web/status/1"),
            vec!["x.com", "twitter.com", "twimg.com"]
        );
        assert_eq!(
            target_cookie_domains("https://www.tiktok.com/@creator/video/99"),
            vec!["tiktok.com", "tiktokv.com"]
        );
        assert_eq!(
            target_cookie_domains("https://video.example.co.jp/watch/1"),
            vec!["example.co.jp"]
        );
        assert!(target_cookie_domains("http://192.168.1.10:8080/video.mp4").is_empty());
        assert!(target_cookie_domains("http://[::1]:8080/video.mp4").is_empty());
    }

    #[test]
    fn test_chromium_v10_decrypt_v24_and_legacy_and_ephemeral_zeroize() {
        let key = derive_chromium_mac_key(b"synthetic-arc-safe-storage-secret");

        let enc_v24 = make_synthetic_v10_cookie(&key, true, ".x.com", "token_xyz_123");
        assert_eq!(
            decrypt_chromium_v10_cookie(&enc_v24, &key, 24, ".x.com")
                .as_deref()
                .map(String::as_str),
            Some("token_xyz_123")
        );
        assert_eq!(
            decrypt_chromium_v10_cookie(&enc_v24, &key, 24, ".evil.example"),
            None
        );

        let enc_legacy = make_synthetic_v10_cookie(&key, false, ".x.com", "legacy_val");
        assert_eq!(
            decrypt_chromium_v10_cookie(&enc_legacy, &key, 23, ".x.com")
                .as_deref()
                .map(String::as_str),
            Some("legacy_val")
        );

        let netscape = format_netscape_cookies(&[DecryptedCookie {
            host_key: ".x.com".to_string(),
            name: "auth_token".to_string(),
            value: Zeroizing::new("token_xyz_123".to_string()),
            path: "/".to_string(),
            is_secure: true,
            expires_utc: 13_800_000_000_000_000, // 2038-ish in WebKit microseconds
            last_access_utc: 100,
        }]);
        assert!(netscape.contains(".x.com\tTRUE\t/\tTRUE\t"));
        assert!(netscape.contains("\tauth_token\ttoken_xyz_123\n"));

        let saved_path;
        {
            let ef = EphemeralCookieFile::create(&netscape).unwrap();
            saved_path = ef.path().to_path_buf();
            assert!(saved_path.is_file());
            assert_eq!(fs::read_to_string(&saved_path).unwrap(), netscape.as_str());
        }
        assert!(!saved_path.exists());
    }

    #[test]
    fn test_arc_multi_profile_selection_prefers_authenticated() {
        let dir = TempDir::new().unwrap();
        let user_data = dir.path().join("User Data");
        let key = derive_chromium_mac_key(b"test-profile-key");

        // Profile 1 (Default): only guest cookie + unrelated other-site cookie
        create_synthetic_cookies_db(
            &user_data.join("Default/Cookies"),
            24,
            &[
                (
                    ".x.com",
                    "guest_id",
                    make_synthetic_v10_cookie(&key, true, ".x.com", "guest_only"),
                    "/",
                    13_800_000_000_000_000,
                    1,
                    9_000_000,
                ),
                (
                    ".otherbank.com",
                    "secret_session",
                    make_synthetic_v10_cookie(&key, true, ".otherbank.com", "MUST_NOT_LEAK"),
                    "/",
                    13_800_000_000_000_000,
                    1,
                    9_500_000,
                ),
            ],
        );

        // Profile 2: authenticated X session (auth_token + ct0 + expired session cookie)
        create_synthetic_cookies_db(
            &user_data.join("Profile 2/Cookies"),
            24,
            &[
                (
                    ".x.com",
                    "auth_token",
                    make_synthetic_v10_cookie(&key, true, ".x.com", "real_auth_token_abc"),
                    "/",
                    13_800_000_000_000_000,
                    1,
                    500,
                ),
                (
                    ".twitter.com",
                    "ct0",
                    make_synthetic_v10_cookie(&key, true, ".twitter.com", "csrf_123"),
                    "/",
                    13_800_000_000_000_000,
                    1,
                    500,
                ),
                (
                    ".x.com",
                    "expired_token",
                    make_synthetic_v10_cookie(&key, true, ".x.com", "should_be_skipped"),
                    "/",
                    12_000_000_000_000_000,
                    1,
                    500,
                ),
                (
                    ".unrelated.example",
                    "other",
                    make_synthetic_v10_cookie(&key, true, ".unrelated.example", "DO_NOT_READ"),
                    "/",
                    13_800_000_000_000_000,
                    1,
                    500,
                ),
            ],
        );

        let extracted = extract_arc_cookies_from_user_data_dir(
            &user_data,
            "https://x.com/someuser/status/999/photo/1",
            &key,
        )
        .unwrap();

        assert_eq!(extracted.len(), 2);
        assert!(extracted
            .iter()
            .any(|c| c.name == "auth_token" && c.value.as_str() == "real_auth_token_abc"));
        assert!(extracted
            .iter()
            .any(|c| c.name == "ct0" && c.value.as_str() == "csrf_123"));
        assert!(!extracted.iter().any(|c| c.host_key.contains("otherbank")));
        assert!(!extracted.iter().any(|c| c.host_key.contains("unrelated")));
    }
}
