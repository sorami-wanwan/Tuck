use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LockSettings {
    pub shortcut: String,
    pub idle_timeout_minutes: u32,
}

impl Default for LockSettings {
    fn default() -> Self {
        Self {
            shortcut: "CommandOrControl+Shift+L".to_string(),
            idle_timeout_minutes: 5,
        }
    }
}

fn default_max_video_size_mb() -> u32 {
    2048
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UrlImportSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub ytdlp_path: String,
    #[serde(default)]
    pub ffmpeg_path: String,
    #[serde(default)]
    pub cookie_browser: String,
    #[serde(default)]
    pub cookies_file: String,
    #[serde(default = "default_max_video_size_mb")]
    pub max_video_size_mb: u32,
    #[serde(default)]
    pub max_video_height: u32,
}

impl Default for UrlImportSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ytdlp_path: String::new(),
            ffmpeg_path: String::new(),
            cookie_browser: String::new(),
            cookies_file: String::new(),
            max_video_size_mb: default_max_video_size_mb(),
            max_video_height: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtdlpStatus {
    pub available: bool,
    pub resolved_path: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
    pub ffmpeg_available: bool,
    pub ffmpeg_path: Option<String>,
    pub ffmpeg_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlImportProgressEvent {
    pub stage: String,
    pub percent: Option<f64>,
    pub speed: String,
    pub eta: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub message: String,
    pub item_index: Option<usize>,
    pub item_total: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub is_open: bool,
    pub can_unlock: bool,
    pub item_count: i64,
    pub uncategorized_count: i64,
    pub untagged_count: i64,
    pub trash_count: i64,
    pub total_bytes: i64,
    pub clipboard_watch_enabled: bool,
    pub skip_plain_export_confirm: bool,
    pub lock_settings: LockSettings,
    pub url_import_settings: UrlImportSettings,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    Image,
    Group,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EntryRef {
    pub kind: EntryKind,
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub color: String,
    pub sort_order: i32,
    pub item_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryEntry {
    pub kind: EntryKind,
    pub id: String,
    pub cover_item_id: String,
    pub cover_mime_type: String,
    pub cover_has_thumbnail: bool,
    pub page_count: i32,
    pub title: String,
    pub width: i32,
    pub height: i32,
    pub byte_size: i64,
    pub duration_ms: i64,
    pub source_url: String,
    pub rating: i32,
    pub memo_preview: String,
    pub tags: Vec<String>,
    pub folder_ids: Vec<String>,
    pub palette: Vec<String>,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetail {
    pub id: String,
    pub title: String,
    pub sha256: String,
    pub mime_type: String,
    pub width: i32,
    pub height: i32,
    pub byte_size: i64,
    pub duration_ms: i64,
    pub source_url: String,
    pub rating: i32,
    pub memo: String,
    pub tags: Vec<String>,
    pub folder_ids: Vec<String>,
    pub palette: Vec<String>,
    pub has_thumbnail: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPage {
    pub item_id: String,
    pub page_order: i32,
    pub title: String,
    pub mime_type: String,
    pub width: i32,
    pub height: i32,
    pub byte_size: i64,
    pub has_thumbnail: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupDetail {
    pub id: String,
    pub title: String,
    pub rating: i32,
    pub memo: String,
    pub reading_dir: String,
    pub spread_mode: String,
    pub cover_alone: bool,
    pub tags: Vec<String>,
    pub folder_ids: Vec<String>,
    pub palette: Vec<String>,
    pub total_bytes: i64,
    pub page_count: i32,
    pub pages: Vec<GroupPage>,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagWithCount {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SortBy {
    #[default]
    CreatedDesc,
    CreatedAsc,
    RatingDesc,
    SizeDesc,
    TitleAsc,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QueryScope {
    #[default]
    All,
    Uncategorized,
    Untagged,
    Trash,
    Folder {
        #[serde(rename = "folderId")]
        folder_id: String,
    },
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    #[serde(default)]
    pub scope: QueryScope,
    #[serde(default)]
    pub tag_names: Vec<String>,
    #[serde(default)]
    pub min_rating: Option<i32>,
    #[serde(default)]
    pub mime_types: Vec<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub sort_by: SortBy,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported_ids: Vec<String>,
    pub avif_pending_ids: Vec<String>,
    pub video_pending_ids: Vec<String>,
    pub duplicate_count: usize,
    pub skipped_unsupported_count: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardImportEvent {
    pub imported_id: Option<String>,
    pub avif_pending: bool,
    pub duplicate: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultHealthInfo {
    pub file_size_bytes: u64,
    pub page_count: i64,
    pub freelist_count: i64,
    pub page_size: i64,
    pub reclaimable_bytes: u64,
    pub integrity_ok: bool,
    pub integrity_detail: String,
    pub journal_mode: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizeReport {
    pub size_before_bytes: u64,
    pub size_after_bytes: u64,
    pub reclaimed_bytes: u64,
    pub integrity_ok: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupReport {
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageExportRequest {
    pub dest_path: String,
    pub passphrase: String,
    pub overwrite: bool,
    #[serde(default)]
    pub folder_ids: Vec<String>,
    #[serde(default = "default_true")]
    pub include_subfolders: bool,
    #[serde(default)]
    pub tag_names: Vec<String>,
    #[serde(default)]
    pub entries: Vec<EntryRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageExportReport {
    pub item_count: usize,
    pub group_count: usize,
    pub page_count: usize,
    pub tag_count: usize,
    pub folder_count: usize,
    pub file_size_bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageImportRequest {
    pub source_path: String,
    pub passphrase: String,
    pub target_folder_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageImportReport {
    pub imported_items: usize,
    pub merged_duplicate_items: usize,
    pub imported_groups: usize,
    pub skipped_duplicate_groups: usize,
    pub imported_folders: usize,
    pub merged_tag_links: usize,
    pub avif_pending_ids: Vec<String>,
    pub video_pending_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum LanAccessMode {
    #[default]
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LanInterfaceInfo {
    pub name: String,
    pub ip: String,
    pub is_primary: bool,
    #[serde(default)]
    pub is_tailscale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LanClientInfo {
    pub id: String,
    pub ip: String,
    pub device_label: String,
    pub connected_at_ms: i64,
    pub last_seen_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartLanShareRequest {
    #[serde(default)]
    pub mode: LanAccessMode,
    #[serde(default)]
    pub duration_minutes: u32,
    #[serde(default)]
    pub bind_ip: Option<String>,
    #[serde(default = "default_true")]
    pub pause_idle_lock: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LanShareStatus {
    pub running: bool,
    pub mode: LanAccessMode,
    pub url: String,
    pub qr_url: String,
    pub qr_svg: String,
    pub pin: String,
    pub port: u16,
    pub selected_ip: String,
    pub interfaces: Vec<LanInterfaceInfo>,
    pub started_at_ms: Option<i64>,
    pub expires_at_ms: Option<i64>,
    pub pause_idle_lock: bool,
    pub clients: Vec<LanClientInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanAuthStatus {
    pub authenticated: bool,
    pub mode: LanAccessMode,
    pub expires_at_ms: Option<i64>,
    pub media_token: Option<String>,
    pub vault_open: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlainExportItemRequest {
    pub item_id: String,
    pub dest_path: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlainExportBatchRequest {
    pub dest_dir: String,
    #[serde(default)]
    pub entries: Vec<EntryRef>,
    #[serde(default)]
    pub folder_ids: Vec<String>,
    #[serde(default = "default_true")]
    pub include_subfolders: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlainExportReport {
    pub file_count: usize,
    pub group_count: usize,
    pub folder_count: usize,
    pub total_bytes: u64,
    pub dest_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebBatchExportRequest {
    #[serde(default)]
    pub entries: Vec<EntryRef>,
    #[serde(default)]
    pub folder_ids: Vec<String>,
    #[serde(default = "default_true")]
    pub include_subfolders: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebBatchExportTicketResponse {
    pub ticket: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub filename: String,
}

/// Compares strings with embedded ASCII numbers sorted numerically (e.g. "p2.png" < "p10.png").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ia = a.chars().peekable();
    let mut ib = b.chars().peekable();

    loop {
        match (ia.peek(), ib.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&ca), Some(&cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut sa = String::new();
                    while let Some(&c) = ia.peek() {
                        if c.is_ascii_digit() {
                            sa.push(c);
                            ia.next();
                        } else {
                            break;
                        }
                    }
                    let mut sb = String::new();
                    while let Some(&c) = ib.peek() {
                        if c.is_ascii_digit() {
                            sb.push(c);
                            ib.next();
                        } else {
                            break;
                        }
                    }
                    let ta = sa.trim_start_matches('0');
                    let tb = sb.trim_start_matches('0');
                    let ord = ta
                        .len()
                        .cmp(&tb.len())
                        .then_with(|| ta.cmp(tb))
                        .then_with(|| sa.len().cmp(&sb.len()));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    ia.next();
                    ib.next();
                    let la = ca.to_lowercase();
                    let lb = cb.to_lowercase();
                    let ord = la.cmp(lb);
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
            }
        }
    }
}
