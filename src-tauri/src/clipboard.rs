use sha2::{Digest, Sha256};

use crate::error::VaultError;
use crate::imaging::rgba_to_png_bytes;

pub fn read_clipboard_image_png() -> Result<Vec<u8>, VaultError> {
    let (_, rgba, width, height) = read_clipboard_raw()?;
    rgba_to_png_bytes(width, height, &rgba)
}

pub fn read_clipboard_raw_hash_and_image() -> Result<(String, u32, u32, Vec<u8>), VaultError> {
    let (hash, rgba, width, height) = read_clipboard_raw()?;
    Ok((hash, width, height, rgba))
}

pub fn read_clipboard_hash_only() -> Option<String> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let image = clipboard.get_image().ok()?;
    let hash = hash_clipboard_rgba(image.width as u32, image.height as u32, &image.bytes);
    Some(hash)
}

fn hash_clipboard_rgba(width: u32, height: u32, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(width.to_le_bytes());
    hasher.update(height.to_le_bytes());
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn read_clipboard_raw() -> Result<(String, Vec<u8>, u32, u32), VaultError> {
    let mut clipboard = arboard::Clipboard::new().map_err(|_| VaultError::ClipboardEmpty)?;
    let image = clipboard
        .get_image()
        .map_err(|_| VaultError::ClipboardEmpty)?;
    let width = image.width as u32;
    let height = image.height as u32;
    let bytes = image.bytes.into_owned();
    let hash = hash_clipboard_rgba(width, height, &bytes);
    Ok((hash, bytes, width, height))
}
