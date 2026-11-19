use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::error::VaultError;

/// Applies the SQLCipher passphrase inside a zeroizing buffer before any other PRAGMA.
pub fn apply_sqlcipher_key(conn: &Connection, passphrase: &str) -> Result<(), VaultError> {
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

    let escaped = Zeroizing::new(passphrase.replace('\'', "''"));
    let pragma = Zeroizing::new(format!("PRAGMA key = '{}';", *escaped));
    drop(escaped);

    conn.execute_batch(&pragma)?;
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
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Normalizes text to Unicode NFC and trims surrounding whitespace for cross-OS consistency.
pub fn normalize_nfc_trimmed(s: &str) -> String {
    s.trim().nfc().collect()
}
