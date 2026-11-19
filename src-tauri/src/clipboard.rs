use crate::crypto::sha256_hex;
use crate::error::VaultError;
use crate::imaging::rgba_to_png_bytes;

pub fn read_clipboard_image_png() -> Result<Vec<u8>, VaultError> {
    let (_, _, rgba, width, height) = read_clipboard_raw()?;
    rgba_to_png_bytes(width, height, &rgba)
}

pub fn read_clipboard_raw_hash_and_image() -> Result<(String, u32, u32, Vec<u8>), VaultError> {
    let (hash, _, rgba, width, height) = read_clipboard_raw()?;
    Ok((hash, width, height, rgba))
}

pub fn read_clipboard_hash_only() -> Option<String> {
    read_clipboard_raw().ok().map(|(hash, _, _, _, _)| hash)
}

fn read_clipboard_raw() -> Result<(String, usize, Vec<u8>, u32, u32), VaultError> {
    let mut clipboard = arboard::Clipboard::new().map_err(|_| VaultError::ClipboardEmpty)?;
    let image = clipboard
        .get_image()
        .map_err(|_| VaultError::ClipboardEmpty)?;
    let bytes = image.bytes.into_owned();
    let mut material = Vec::with_capacity(8 + bytes.len());
    material.extend_from_slice(&(image.width as u32).to_le_bytes());
    material.extend_from_slice(&(image.height as u32).to_le_bytes());
    material.extend_from_slice(&bytes);
    let hash = sha256_hex(&material);
    let len = bytes.len();
    Ok((hash, len, bytes, image.width as u32, image.height as u32))
}
