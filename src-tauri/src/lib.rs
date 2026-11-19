mod arc_cookies;
mod clipboard;
mod commands;
mod crypto;
mod db;
mod error;
mod imaging;
mod lan_server;
mod local_media;
mod media_stream;
mod models;
mod package;
mod plain_export;
mod state;
mod thumbnail_regen;
mod ytdlp;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use db::ImportOutcome;
use error::VaultError;
use media_stream::resolve_media_slice;
use models::ClipboardImportEvent;
use state::AppState;
use tauri::http::{header, Method, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub(crate) fn resolve_allowed_origin(origin: &str) -> Option<&'static str> {
    const PROD_ORIGINS: &[&str] = &[
        "tauri://localhost",
        "http://tauri.localhost",
        "https://tauri.localhost",
    ];
    for &allowed in PROD_ORIGINS {
        if origin.eq_ignore_ascii_case(allowed) {
            return Some(allowed);
        }
    }
    #[cfg(debug_assertions)]
    {
        const DEV_ORIGINS: &[&str] = &["http://localhost:1420", "http://127.0.0.1:1420"];
        for &allowed in DEV_ORIGINS {
            if origin.eq_ignore_ascii_case(allowed) {
                return Some(allowed);
            }
        }
    }
    None
}

fn respond_status(
    responder: tauri::UriSchemeResponder,
    status: StatusCode,
    content_range: Option<&str>,
    allowed_origin: Option<&'static str>,
) {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::VARY, "Origin");
    if let Some(origin) = allowed_origin {
        builder = builder.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    if let Some(cr) = content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }
    if let Ok(resp) = builder.body(Vec::new()) {
        responder.respond(resp);
    }
}

fn send_media_response(
    responder: tauri::UriSchemeResponder,
    status: StatusCode,
    mime_type: &str,
    content_range: Option<&str>,
    content_length: usize,
    body: Vec<u8>,
    allowed_origin: Option<&'static str>,
) {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, content_length.to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::VARY, "Origin")
        .header(
            "Access-Control-Expose-Headers",
            "Content-Range, Accept-Ranges, Content-Length",
        );
    if let Some(origin) = allowed_origin {
        builder = builder.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    if let Some(cr) = content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }
    if let Ok(resp) = builder.body(body) {
        responder.respond(resp);
    }
}

fn respond_vault_err(
    responder: tauri::UriSchemeResponder,
    err: &VaultError,
    allowed_origin: Option<&'static str>,
) {
    match err {
        VaultError::VaultNotOpen => {
            respond_status(responder, StatusCode::FORBIDDEN, None, allowed_origin)
        }
        VaultError::NotFound(_) => {
            respond_status(responder, StatusCode::NOT_FOUND, None, allowed_origin)
        }
        _ => respond_status(
            responder,
            StatusCode::INTERNAL_SERVER_ERROR,
            None,
            allowed_origin,
        ),
    }
}

static GLOBAL_SHORTCUT_READY: AtomicBool = AtomicBool::new(false);

fn can_init_global_shortcut() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

pub fn sync_global_lock_shortcut(app: &AppHandle, shortcut_str: &str) {
    if !GLOBAL_SHORTCUT_READY.load(Ordering::SeqCst) {
        return;
    }
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if let Ok(shortcut) = shortcut_str.parse::<Shortcut>() {
        let _ = gs.register(shortcut);
    }
}

pub fn clear_global_lock_shortcuts(app: &AppHandle) {
    if !GLOBAL_SHORTCUT_READY.load(Ordering::SeqCst) {
        return;
    }
    let _ = app.global_shortcut().unregister_all();
}

fn spawn_clipboard_watcher(app_handle: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(750));

        let Some(state) = app_handle.try_state::<AppState>() else {
            continue;
        };
        if !state.clipboard_watch_enabled.load(Ordering::SeqCst) || !state.is_open() {
            continue;
        }

        let Ok((hash, w, h, rgba)) = clipboard::read_clipboard_raw_hash_and_image() else {
            continue;
        };

        let is_new = {
            let Ok(mut last) = state.clipboard_last_hash.lock() else {
                continue;
            };
            if last.as_deref() == Some(hash.as_str()) {
                false
            } else {
                *last = Some(hash);
                true
            }
        };
        if !is_new {
            continue;
        }

        let Ok(png_bytes) = imaging::rgba_to_png_bytes(w, h, &rgba) else {
            continue;
        };

        if !state.clipboard_watch_enabled.load(Ordering::SeqCst) {
            continue;
        }

        let outcome = {
            let Ok(mut guard) = state.db.lock() else {
                continue;
            };
            let Some(db) = guard.as_mut() else {
                continue;
            };
            db.import_bytes(&png_bytes, None)
        };

        if let Ok(res) = outcome {
            let mut imported = false;
            let payload = match res {
                ImportOutcome::Imported {
                    id, thumb_pending, ..
                } => {
                    imported = true;
                    ClipboardImportEvent {
                        imported_id: Some(id),
                        avif_pending: thumb_pending,
                        duplicate: false,
                    }
                }
                ImportOutcome::Duplicate => ClipboardImportEvent {
                    imported_id: None,
                    avif_pending: false,
                    duplicate: true,
                },
            };
            if imported {
                state.notify_lan_library_changed();
            }
            let _ = app_handle.emit("vault://clipboard-imported", payload);
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("vaultmedia", |ctx, request, responder| {
            let allowed_origin = match request
                .headers()
                .get(header::ORIGIN)
                .and_then(|v| v.to_str().ok())
            {
                Some(raw_origin) => match resolve_allowed_origin(raw_origin) {
                    Some(matched) => Some(matched),
                    None => {
                        respond_status(responder, StatusCode::FORBIDDEN, None, None);
                        return;
                    }
                },
                None => None,
            };

            let method = request.method().clone();
            if method == Method::OPTIONS {
                let mut builder = Response::builder()
                    .status(StatusCode::NO_CONTENT)
                    .header(header::VARY, "Origin")
                    .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
                    .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Range");
                if let Some(origin) = allowed_origin {
                    builder = builder.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                }
                if let Ok(resp) = builder.body(Vec::new()) {
                    responder.respond(resp);
                }
                return;
            }

            let Some(state) = ctx
                .app_handle()
                .try_state::<AppState>()
                .map(|s| s.inner().clone())
            else {
                respond_status(responder, StatusCode::FORBIDDEN, None, allowed_origin);
                return;
            };
            if !state.is_open() {
                respond_status(responder, StatusCode::FORBIDDEN, None, allowed_origin);
                return;
            }

            let item_id = request.uri().path().trim_matches('/').to_string();
            if item_id.is_empty() {
                respond_status(responder, StatusCode::NOT_FOUND, None, allowed_origin);
                return;
            }

            let range_header = request
                .headers()
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            std::thread::spawn(move || {
                match resolve_media_slice(
                    &state,
                    &item_id,
                    method == Method::HEAD,
                    range_header.as_deref(),
                ) {
                    Ok(slice) => {
                        if slice.status == StatusCode::RANGE_NOT_SATISFIABLE {
                            respond_status(
                                responder,
                                slice.status,
                                slice.content_range.as_deref(),
                                allowed_origin,
                            );
                        } else {
                            send_media_response(
                                responder,
                                slice.status,
                                &slice.mime_type,
                                slice.content_range.as_deref(),
                                slice.content_length,
                                slice.body,
                                allowed_origin,
                            );
                        }
                    }
                    Err(e) => respond_vault_err(responder, &e, allowed_origin),
                }
            });
        })
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            if can_init_global_shortcut() {
                let plugin = tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(|app, _shortcut, event| {
                        if event.state() == ShortcutState::Pressed {
                            if let Some(state) = app.try_state::<AppState>() {
                                if state.is_open() {
                                    state.lock_vault();
                                    clear_global_lock_shortcuts(app);
                                    let _ = app.emit("vault://locked", ());
                                }
                            }
                        }
                    })
                    .build();
                if app.handle().plugin(plugin).is_ok() {
                    GLOBAL_SHORTCUT_READY.store(true, Ordering::SeqCst);
                }
            }
            spawn_clipboard_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_vault,
            commands::open_vault,
            commands::unlock_vault,
            commands::lock_vault,
            commands::close_vault,
            commands::get_vault_status,
            commands::set_lock_settings,
            commands::set_url_import_settings,
            commands::check_ytdlp_status,
            commands::set_clipboard_watch,
            commands::import_paths,
            commands::import_folder_as_group,
            commands::import_bytes,
            commands::import_clipboard,
            commands::import_url,
            commands::cancel_url_import,
            commands::save_media_thumbnail,
            commands::regenerate_item_thumbnail,
            commands::list_thumbnail_targets,
            commands::get_thumbnail_stats,
            commands::query_entries,
            commands::get_item_detail,
            commands::get_thumbnail,
            commands::get_image,
            commands::get_display_image,
            commands::set_item_title,
            commands::set_item_rating,
            commands::set_item_memo,
            commands::set_item_source_url,
            commands::add_item_tag,
            commands::remove_item_tag,
            commands::list_tags,
            commands::list_folders,
            commands::create_folder,
            commands::rename_folder,
            commands::move_folder,
            commands::delete_folder,
            commands::add_entries_to_folder,
            commands::remove_entries_from_folder,
            commands::create_group_from_items,
            commands::add_items_to_group,
            commands::reorder_group_pages,
            commands::remove_page_from_group,
            commands::ungroup,
            commands::get_group_detail,
            commands::set_group_title,
            commands::set_group_rating,
            commands::set_group_memo,
            commands::set_group_reader_prefs,
            commands::add_group_tag,
            commands::remove_group_tag,
            commands::move_entries_to_trash,
            commands::restore_entries_from_trash,
            commands::permanently_delete_entries,
            commands::empty_trash,
            commands::get_vault_health,
            commands::optimize_vault,
            commands::backup_vault,
            commands::export_package,
            commands::import_package,
            commands::set_skip_plain_export_confirm,
            commands::export_plain_item,
            commands::export_plain_batch,
            commands::get_lan_network_interfaces,
            commands::get_lan_share_status,
            commands::start_lan_share,
            commands::stop_lan_share,
            commands::update_lan_share_mode,
            commands::extend_lan_share_timer,
            commands::regenerate_lan_credentials,
            commands::disconnect_lan_clients,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                if let Some(state) = window.try_state::<AppState>() {
                    state.close_vault();
                }
                clear_global_lock_shortcuts(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Tuck application");

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<AppState>() {
                state.close_vault();
            }
            clear_global_lock_shortcuts(app_handle);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_allowed_origin() {
        assert_eq!(
            resolve_allowed_origin("tauri://localhost"),
            Some("tauri://localhost")
        );
        assert_eq!(
            resolve_allowed_origin("http://tauri.localhost"),
            Some("http://tauri.localhost")
        );
        assert_eq!(
            resolve_allowed_origin("HTTPS://TAURI.LOCALHOST"),
            Some("https://tauri.localhost")
        );
        assert_eq!(resolve_allowed_origin("https://evil.example"), None);
        assert_eq!(resolve_allowed_origin("null"), None);
    }
}
