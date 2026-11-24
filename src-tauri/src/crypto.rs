use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::error::VaultError;

/// Validates a vault or package passphrase before touching disk.
pub fn validate_passphrase(passphrase: &str) -> Result<(), VaultError> {
    if passphrase.is_empty() {
        return Err(VaultError::InvalidPassphrase(
            "パスワードを入力してください".to_string(),
        ));
    }
    if passphrase.contains('\0') {
        return Err(VaultError::InvalidPassphrase(
            "パスワードに使用できない文字が含まれています".to_string(),
        ));
    }
    Ok(())
}

/// Applies the SQLCipher passphrase inside a zeroizing buffer before any other PRAGMA.
pub fn apply_sqlcipher_key(conn: &Connection, passphrase: &str) -> Result<(), VaultError> {
    validate_passphrase(passphrase)?;

    let escaped = Zeroizing::new(passphrase.replace('\'', "''"));
    let pragma = Zeroizing::new(format!("PRAGMA key = '{}';", *escaped));
    drop(escaped);

    conn.execute_batch(&pragma).map_err(|_| {
        VaultError::InvalidPassphrase("暗号鍵の適用に失敗しました".to_string())
    })?;
    drop(pragma);

    conn.execute_batch("PRAGMA cipher_compatibility = 4;")?;
    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn sha256_file_hex(path: &Path) -> Result<String, VaultError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Normalizes text to Unicode NFC and trims surrounding whitespace for cross-OS consistency.
pub fn normalize_nfc_trimmed(s: &str) -> String {
    s.trim().nfc().collect()
}
