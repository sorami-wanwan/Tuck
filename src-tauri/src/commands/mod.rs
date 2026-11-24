mod entries;
mod import;
mod lan;
mod maintenance;
mod organize;
mod vault;

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::db::{ImportOutcome, VaultDb};
use crate::error::VaultError;
use crate::imaging::is_video_mime;
use crate::models::{GroupDetail, ImportReport};

pub use entries::*;
pub use import::*;
pub use lan::*;
pub use maintenance::*;
pub use organize::*;
pub use vault::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportGroupReport {
    pub report: ImportReport,
    pub group: Option<GroupDetail>,
}

pub(super) fn with_open_db<F, T>(
    db_arc: &Arc<Mutex<Option<VaultDb>>>,
    f: F,
) -> Result<T, VaultError>
where
    F: FnOnce(&mut VaultDb) -> Result<T, VaultError>,
{
    let mut guard = db_arc
        .lock()
        .map_err(|_| VaultError::InvalidInput("内部状態ロックの取得に失敗しました".to_string()))?;
    let db = guard.as_mut().ok_or(VaultError::VaultNotOpen)?;
    f(db)
}

pub(super) async fn run_blocking<F, T>(f: F) -> Result<T, VaultError>
where
    F: FnOnce() -> Result<T, VaultError> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| VaultError::InvalidInput(format!("バックグラウンド処理に失敗しました: {e}")))?
}

pub(super) fn title_from_path(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
}

pub(crate) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(super) fn record_import_outcome(
    report: &mut ImportReport,
    outcome: Result<ImportOutcome, VaultError>,
    label: &str,
) {
    match outcome {
        Ok(ImportOutcome::Imported {
            id,
            mime_type,
            thumb_pending,
        }) => {
            if thumb_pending {
                if is_video_mime(&mime_type) {
                    report.video_pending_ids.push(id.clone());
                } else {
                    report.avif_pending_ids.push(id.clone());
                }
            }
            report.imported_ids.push(id);
        }
        Ok(ImportOutcome::Duplicate) => {
            report.duplicate_count += 1;
        }
        Err(VaultError::UnsupportedFormat(_)) => {
            report.skipped_unsupported_count += 1;
        }
        Err(e) => {
            report.errors.push(format!("{label}: {e}"));
        }
    }
}
