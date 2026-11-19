use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::db::VaultDb;
use crate::error::VaultError;
use crate::lan_server::{list_lan_interfaces, LanServerHandle};
use crate::local_media::{start_local_media_server, LocalMediaServerHandle};
use crate::models::{LanShareStatus, UrlImportProgressEvent, VaultStatus};

const MEDIA_RAM_CACHE_LIMIT: usize = 2;

#[derive(Clone)]
pub struct CachedMediaBlob {
    pub item_id: String,
    pub mime_type: String,
    pub bytes: Arc<Vec<u8>>,
}

#[derive(Clone, Default)]
pub struct AppState {
    pub db: Arc<Mutex<Option<VaultDb>>>,
    pub locked_path: Arc<Mutex<Option<PathBuf>>>,
    pub clipboard_watch_enabled: Arc<AtomicBool>,
    pub clipboard_last_hash: Arc<Mutex<Option<String>>>,
    pub media_cache: Arc<Mutex<Vec<CachedMediaBlob>>>,
    pub url_import_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    pub lan_server: Arc<Mutex<Option<LanServerHandle>>>,
    pub local_media_server: Arc<Mutex<Option<LocalMediaServerHandle>>>,
}

impl AppState {
    pub fn lock_db(&self) -> Result<MutexGuard<'_, Option<VaultDb>>, VaultError> {
        self.db
            .lock()
            .map_err(|_| VaultError::InvalidInput("内部状態ロックの取得に失敗しました".to_string()))
    }

    pub fn is_open(&self) -> bool {
        self.db.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    pub fn can_unlock(&self) -> bool {
        self.locked_path
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    pub fn stop_clipboard_watch(&self) {
        self.clipboard_watch_enabled.store(false, Ordering::SeqCst);
        if let Ok(mut h) = self.clipboard_last_hash.lock() {
            *h = None;
        }
    }

    pub fn clear_media_cache(&self) {
        if let Ok(mut cache) = self.media_cache.lock() {
            cache.clear();
        }
    }

    pub fn invalidate_media_items(&self, item_ids: &[String]) {
        if item_ids.is_empty() {
            return;
        }
        if let Ok(mut cache) = self.media_cache.lock() {
            cache.retain(|entry| !item_ids.iter().any(|id| id == &entry.item_id));
        }
    }

    pub fn get_or_load_media(&self, item_id: &str) -> Result<(String, Arc<Vec<u8>>), VaultError> {
        {
            if let Ok(mut cache) = self.media_cache.lock() {
                if let Some(pos) = cache.iter().position(|e| e.item_id == item_id) {
                    let entry = cache.remove(pos);
                    let res = (entry.mime_type.clone(), Arc::clone(&entry.bytes));
                    cache.push(entry);
                    return Ok(res);
                }
            }
        }

        let (mime_type, raw_bytes) = {
            let guard = self.lock_db()?;
            let db = guard.as_ref().ok_or(VaultError::VaultNotOpen)?;
            db.get_media_blob_with_mime(item_id)?
        };

        let bytes = Arc::new(raw_bytes);
        if let Ok(mut cache) = self.media_cache.lock() {
            if let Some(pos) = cache.iter().position(|e| e.item_id == item_id) {
                cache.remove(pos);
            }
            cache.push(CachedMediaBlob {
                item_id: item_id.to_string(),
                mime_type: mime_type.clone(),
                bytes: Arc::clone(&bytes),
            });
            while cache.len() > MEDIA_RAM_CACHE_LIMIT {
                cache.remove(0);
            }
        }

        Ok((mime_type, bytes))
    }

    pub fn get_media_info(&self, item_id: &str) -> Result<(String, usize, usize), VaultError> {
        let guard = self.lock_db()?;
        let db = guard.as_ref().ok_or(VaultError::VaultNotOpen)?;
        db.get_media_info(item_id)
    }

    pub fn read_chunked_media_range(
        &self,
        item_id: &str,
        start: usize,
        end: usize,
        chunk_size: usize,
    ) -> Result<Vec<u8>, VaultError> {
        let guard = self.lock_db()?;
        let db = guard.as_ref().ok_or(VaultError::VaultNotOpen)?;
        db.read_media_range(item_id, start, end, chunk_size)
    }

    pub fn read_blob_chunk(
        &self,
        item_id: &str,
        chunk_index: i64,
    ) -> Result<Option<Vec<u8>>, VaultError> {
        let guard = self.lock_db()?;
        let db = guard.as_ref().ok_or(VaultError::VaultNotOpen)?;
        db.read_blob_chunk(item_id, chunk_index)
    }

    pub fn stop_local_media_server(&self) {
        let handle = self
            .local_media_server
            .lock()
            .ok()
            .and_then(|mut g| g.take());
        if let Some(h) = handle {
            h.stop();
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub async fn ensure_local_media_server(&self) -> Result<(), VaultError> {
        self.stop_local_media_server();
        if !self.is_open() {
            return Ok(());
        }
        let handle = start_local_media_server(self.clone()).await?;
        let mut guard = self.local_media_server.lock().map_err(|_| {
            VaultError::InvalidInput("内部状態ロックの取得に失敗しました".to_string())
        })?;
        if !self.is_open() {
            handle.stop();
            return Ok(());
        }
        if let Some(old) = guard.replace(handle) {
            old.stop();
        }
        Ok(())
    }

    pub async fn get_or_start_local_media_server(
        &self,
    ) -> Result<LocalMediaServerHandle, VaultError> {
        if !self.is_open() {
            return Err(VaultError::VaultNotOpen);
        }
        if let Ok(guard) = self.local_media_server.lock() {
            if let Some(existing) = guard.as_ref() {
                if !existing.is_stopped() {
                    return Ok(existing.clone());
                }
            }
        }
        let handle = start_local_media_server(self.clone()).await?;
        let mut guard = self.local_media_server.lock().map_err(|_| {
            VaultError::InvalidInput("内部状態ロックの取得に失敗しました".to_string())
        })?;
        if !self.is_open() {
            handle.stop();
            return Err(VaultError::VaultNotOpen);
        }
        if let Some(existing) = guard.as_ref() {
            if !existing.is_stopped() {
                handle.stop();
                return Ok(existing.clone());
            }
        }
        *guard = Some(handle.clone());
        Ok(handle)
    }

    pub fn begin_url_import(&self) -> Result<Arc<AtomicBool>, VaultError> {
        let mut guard = self
            .url_import_cancel
            .lock()
            .map_err(|_| VaultError::UrlImport("内部状態ロックの取得に失敗しました".to_string()))?;
        if guard.is_some() {
            return Err(VaultError::UrlImport(
                "別のURL取り込みが進行中です。完了またはキャンセルしてからお試しください"
                    .to_string(),
            ));
        }
        let token = Arc::new(AtomicBool::new(false));
        *guard = Some(Arc::clone(&token));
        Ok(token)
    }

    pub fn finish_url_import(&self, token: &Arc<AtomicBool>) {
        if let Ok(mut guard) = self.url_import_cancel.lock() {
            if guard.as_ref().is_some_and(|cur| Arc::ptr_eq(cur, token)) {
                *guard = None;
            }
        }
    }

    pub fn cancel_url_import(&self) -> bool {
        if let Ok(guard) = self.url_import_cancel.lock() {
            if let Some(token) = guard.as_ref() {
                token.store(true, Ordering::SeqCst);
                return true;
            }
        }
        false
    }

    pub fn current_lan_status(&self) -> LanShareStatus {
        let handle = self.lan_server.lock().ok().and_then(|g| g.clone());
        if let Some(h) = handle {
            let st = h.status();
            if st.running {
                return st;
            }
        }
        let interfaces = list_lan_interfaces();
        let selected_ip = interfaces
            .first()
            .map(|i| i.ip.clone())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        LanShareStatus {
            running: false,
            selected_ip,
            interfaces,
            pause_idle_lock: true,
            ..Default::default()
        }
    }

    pub fn stop_lan_share_with_reason(&self, reason: &str) {
        let handle = self.lan_server.lock().ok().and_then(|mut g| g.take());
        if let Some(h) = handle {
            h.stop(reason);
        }
    }

    pub fn notify_lan_library_changed(&self) {
        let handle = self.lan_server.lock().ok().and_then(|g| g.clone());
        if let Some(h) = handle {
            h.broadcast_library_changed();
        }
    }

    pub fn notify_lan_url_progress(&self, ev: &UrlImportProgressEvent) {
        let handle = self.lan_server.lock().ok().and_then(|g| g.clone());
        if let Some(h) = handle {
            h.broadcast_url_progress(ev);
        }
    }

    pub fn notify_lan_thumbnails_updated(&self, item_ids: Vec<String>) {
        if item_ids.is_empty() {
            return;
        }
        let handle = self.lan_server.lock().ok().and_then(|g| g.clone());
        if let Some(h) = handle {
            h.broadcast_thumbnails_updated(item_ids);
        }
    }

    /// Stops any active LAN server, drops the open DB handle, purges RAM media cache, and stops clipboard watch while retaining path for password unlock.
    pub fn lock_vault(&self) {
        self.stop_lan_share_with_reason("vault_locked");
        self.stop_clipboard_watch();
        self.cancel_url_import();
        self.clear_media_cache();
        if let Ok(mut guard) = self.db.lock() {
            *guard = None;
        }
        self.stop_local_media_server();
    }

    /// Completely closes the vault and forgets the path.
    pub fn close_vault(&self) {
        self.lock_vault();
        if let Ok(mut p) = self.locked_path.lock() {
            *p = None;
        }
    }

    pub fn current_status(&self) -> Result<VaultStatus, VaultError> {
        let watch = self.clipboard_watch_enabled.load(Ordering::SeqCst);
        let maybe_status = {
            let guard = self.lock_db()?;
            match guard.as_ref() {
                Some(db) => Some(db.status()?),
                None => None,
            }
        };
        match maybe_status {
            Some(mut status) => {
                status.clipboard_watch_enabled = watch;
                #[cfg(target_os = "linux")]
                if let Ok(lms) = self.local_media_server.lock() {
                    if let Some(handle) = lms.as_ref() {
                        status.local_media_base_url = Some(handle.base_url().to_string());
                        status.local_media_token = Some(handle.token().to_string());
                    }
                }
                Ok(status)
            }
            None => Ok(VaultStatus {
                is_open: false,
                can_unlock: self.can_unlock(),
                ..Default::default()
            }),
        }
    }
}
