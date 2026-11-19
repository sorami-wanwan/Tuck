use super::*;
use crate::models::*;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use rusqlite::params;
use std::cmp::Ordering;
use std::fs;
use std::io::Cursor;
use tempfile::TempDir;

fn make_png_bytes(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
    let mut img = RgbaImage::new(width, height);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let shift = ((x + y) % 40) as u8;
        *px = Rgba([r.saturating_add(shift), g, b.saturating_sub(shift), a]);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .unwrap();
    out
}

fn make_jpeg_bytes(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut img = image::RgbImage::new(width, height);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([r, g, b]);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
        .unwrap();
    out
}

fn make_gif_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut img = RgbaImage::new(width, height);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = Rgba([40, 180, 220, 255]);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Gif)
        .unwrap();
    out
}

fn make_webp_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut img = RgbaImage::new(width, height);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = Rgba([240, 100, 60, 255]);
    }
    let encoder = webp::Encoder::from_rgba(img.as_raw(), width, height);
    encoder.encode(85.0).to_vec()
}

fn make_minimal_avif_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&24u32.to_be_bytes());
    bytes.extend_from_slice(b"ftyp");
    bytes.extend_from_slice(b"avif");
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"avif");
    bytes.extend_from_slice(b"mif1");
    bytes.extend_from_slice(b"FAKE_AVIF_PAYLOAD_BYTES");
    bytes
}

fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn import_id(db: &mut VaultDb, bytes: &[u8], title: Option<&str>) -> String {
    match db.import_bytes(bytes, title).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        ImportOutcome::Duplicate => panic!("unexpected duplicate"),
    }
}

#[test]
fn test_create_open_wrong_password() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("test.vault");

    {
        let db = VaultDb::create(&vault_path, "correct-pass-123", false).unwrap();
        let status = db.status().unwrap();
        assert!(status.is_open);
        assert_eq!(status.item_count, 0);
    }

    let dup_err = VaultDb::create(&vault_path, "another", false);
    assert!(matches!(dup_err, Err(VaultError::VaultAlreadyExists(_))));

    let wrong_err = VaultDb::open(&vault_path, "wrong-pass");
    assert!(matches!(wrong_err, Err(VaultError::WrongPassword)));

    let db = VaultDb::open(&vault_path, "correct-pass-123").unwrap();
    assert_eq!(db.status().unwrap().item_count, 0);
}

#[test]
fn test_encryption_at_rest_and_single_file() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("secret.vault");
    let png = make_png_bytes(120, 90, 200, 40, 80, 255);
    let jpg = make_jpeg_bytes(100, 80, 30, 120, 210);

    {
        let mut db = VaultDb::create(&vault_path, "UltraSecretKey!99", false).unwrap();
        let id1 = import_id(&mut db, &png, Some("SECRET_TITLE_XYZ"));
        import_id(&mut db, &jpg, Some("SECOND_IMAGE"));
        let folder = db.create_folder(None, "SECRET_FOLDER_NAME", "").unwrap();
        db.add_entries_to_folder(
            &folder.id,
            &[EntryRef {
                kind: EntryKind::Image,
                id: id1.clone(),
            }],
        )
        .unwrap();
        db.add_tag(&id1, "SECRET_TAG_ALPHA").unwrap();
        db.set_memo(&id1, "PRIVATE_MEMO_99999").unwrap();
    }

    let entries: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries, vec!["secret.vault".to_string()]);

    let raw_vault = fs::read(&vault_path).unwrap();
    assert!(!contains_subsequence(&raw_vault, b"SQLite format 3"));
    assert!(!contains_subsequence(&raw_vault, b"SECRET_TAG_ALPHA"));
    assert!(!contains_subsequence(&raw_vault, b"SECRET_FOLDER_NAME"));
    assert!(!contains_subsequence(&raw_vault, b"PRIVATE_MEMO_99999"));
    assert!(!contains_subsequence(&raw_vault, b"SECRET_TITLE_XYZ"));
    assert!(!contains_subsequence(
        &raw_vault,
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
    ));
    assert!(jpg.starts_with(&[0xFF, 0xD8, 0xFF]) && jpg.len() >= 16);
    assert!(!contains_subsequence(&raw_vault, &jpg[..16]));
}

#[test]
fn test_deduplication_and_formats() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("formats.vault");
    let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();

    let png = make_png_bytes(800, 500, 50, 120, 200, 180);
    let jpg = make_jpeg_bytes(600, 900, 180, 60, 40);
    let gif = make_gif_bytes(320, 240);
    let webp_data = make_webp_bytes(500, 500);
    let avif = make_minimal_avif_bytes();

    let png_id = import_id(&mut db, &png, Some("alpha"));
    assert!(matches!(
        db.import_bytes(&png, Some("alpha-dup")).unwrap(),
        ImportOutcome::Duplicate
    ));

    let jpg_id = import_id(&mut db, &jpg, None);
    let gif_id = import_id(&mut db, &gif, None);
    let webp_id = import_id(&mut db, &webp_data, None);
    let avif_id = match db.import_bytes(&avif, Some("avif-pic")).unwrap() {
        ImportOutcome::Imported {
            id, thumb_pending, ..
        } => {
            assert!(thumb_pending);
            id
        }
        _ => panic!("expected imported"),
    };

    assert_eq!(db.status().unwrap().item_count, 5);
    assert_eq!(db.get_blob_bytes(&png_id).unwrap(), png);
    assert_eq!(db.get_blob_bytes(&jpg_id).unwrap(), jpg);
    assert_eq!(db.get_blob_bytes(&gif_id).unwrap(), gif);
    assert_eq!(db.get_blob_bytes(&webp_id).unwrap(), webp_data);
    assert_eq!(db.get_blob_bytes(&avif_id).unwrap(), avif);

    let thumb = db.get_thumbnail_bytes(&png_id).unwrap();
    assert_eq!(&thumb[0..4], b"RIFF");
    assert_eq!(&thumb[8..12], b"WEBP");
    let png_detail = db.get_item_detail(&png_id).unwrap();
    assert_eq!((png_detail.width, png_detail.height), (800, 500));
    assert!(!png_detail.palette.is_empty());

    assert!(!db.get_item_detail(&avif_id).unwrap().has_thumbnail);
    let client_png = make_png_bytes(300, 200, 100, 200, 120, 255);
    let updated_avif = db
        .save_media_thumbnail(&avif_id, &client_png, 1920, 1080, 0)
        .unwrap();
    assert!(updated_avif.has_thumbnail);
    assert_eq!((updated_avif.width, updated_avif.height), (1920, 1080));

    let txt_err = db.import_bytes(b"not an image file", None);
    assert!(matches!(txt_err, Err(VaultError::UnsupportedFormat(_))));
}

#[test]
fn test_tags_nfc_search_rating_memo_delete() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("organize.vault");
    let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();

    let p1 = make_png_bytes(100, 100, 10, 20, 30, 255);
    let p2 = make_png_bytes(100, 100, 200, 20, 30, 255);
    let j3 = make_jpeg_bytes(120, 120, 40, 200, 80);

    let id1 = import_id(&mut db, &p1, Some("イラストA"));
    let id2 = import_id(&mut db, &p2, Some("イラストB"));
    let id3 = import_id(&mut db, &j3, Some("写真C"));

    db.add_tag(&id1, "か\u{3099}そ\u{3099}う").unwrap();
    db.add_tag(&id2, "がぞう").unwrap();
    db.add_tag(&id1, "お気に入り").unwrap();

    let tags = db.list_tags().unwrap();
    assert_eq!(tags.len(), 2);
    assert_eq!(tags[0].name, "がぞう");
    assert_eq!(tags[0].count, 2);

    db.set_rating(&id1, 5).unwrap();
    db.set_rating(&id2, 3).unwrap();
    db.set_memo(&id2, "背景の資料").unwrap();

    let res = db
        .query_entries(&SearchQuery {
            tag_names: vec!["か\u{3099}そ\u{3099}う".into(), "お気に入り".into()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].id, id1);

    let untagged = db
        .query_entries(&SearchQuery {
            scope: QueryScope::Untagged,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(untagged.len(), 1);
    assert_eq!(untagged[0].id, id3);

    let rated = db
        .query_entries(&SearchQuery {
            min_rating: Some(3),
            text: "資料".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rated.len(), 1);
    assert_eq!(rated[0].id, id2);

    let deleted = db
        .permanently_delete_entries(&[EntryRef {
            kind: EntryKind::Image,
            id: id1.clone(),
        }])
        .unwrap();
    assert_eq!(deleted, 1);
    assert!(matches!(
        db.get_item_detail(&id1),
        Err(VaultError::NotFound(_))
    ));

    let remaining_tags = db.list_tags().unwrap();
    assert_eq!(remaining_tags.len(), 1);
    assert_eq!(remaining_tags[0].name, "がぞう");
    assert_eq!(remaining_tags[0].count, 1);
}

#[test]
fn test_migrate_v1_to_v2() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("legacy_v1.vault");
    let png = make_png_bytes(64, 64, 90, 110, 130, 255);

    // Manually initialize a pure Phase-1 v1 database
    let item_id = Uuid::new_v4().to_string();
    {
        let conn = Connection::open(&vault_path).unwrap();
        apply_sqlcipher_key(&conn, "migrate-pass").unwrap();
        apply_runtime_pragmas(&conn).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute(
            "INSERT INTO vault_meta (key, value) VALUES ('schema_version', '1')",
            [],
        )
        .unwrap();
        conn.execute(
                "INSERT INTO items (id, title, sha256, mime_type, width, height, byte_size, rating, memo, palette, created_at, updated_at, deleted_at)
                 VALUES (?1, 'v1イラスト', 'abc123', 'image/png', 64, 64, ?2, 4, 'memo', '[]', 1000, 1000, NULL)",
                params![item_id, png.len() as i64],
            )
            .unwrap();
        conn.execute(
            "INSERT INTO blobs (item_id, data) VALUES (?1, ?2)",
            params![item_id, png],
        )
        .unwrap();
        conn.execute("INSERT INTO tags (name) VALUES ('レガシー')", [])
            .unwrap();
        let tid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
            params![item_id, tid],
        )
        .unwrap();
    }

    // Open via Phase 3+ VaultDb::open -> auto-migrates to v4
    let mut db = VaultDb::open(&vault_path, "migrate-pass").unwrap();
    let ver: String = db
        .conn
        .query_row(
            "SELECT value FROM vault_meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ver, "4");

    let detail = db.get_item_detail(&item_id).unwrap();
    assert_eq!(detail.title, "v1イラスト");
    assert_eq!(detail.duration_ms, 0);
    assert_eq!(detail.source_url, "");
    assert_eq!(detail.tags, vec!["レガシー".to_string()]);

    let folder = db.create_folder(None, "移行後フォルダ", "#4c8dff").unwrap();
    db.add_entries_to_folder(
        &folder.id,
        &[EntryRef {
            kind: EntryKind::Image,
            id: item_id.clone(),
        }],
    )
    .unwrap();
    let in_folder = db
        .query_entries(&SearchQuery {
            scope: QueryScope::Folder {
                folder_id: folder.id,
            },
            ..Default::default()
        })
        .unwrap();
    assert_eq!(in_folder.len(), 1);
    assert_eq!(in_folder[0].id, item_id);
}

#[test]
fn test_virtual_folders_hierarchy_and_multi_membership() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("folders.vault");
    let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();

    let root = db.create_folder(None, "作家別", "#4c8dff").unwrap();
    let child = db
        .create_folder(Some(&root.id), "お気に入り作家", "")
        .unwrap();
    let grandchild = db.create_folder(Some(&child.id), "商業誌", "").unwrap();
    let other_root = db.create_folder(None, "資料", "").unwrap();

    // Cycle prevention: moving root under grandchild must fail
    let cycle_err = db.move_folder(&root.id, Some(&grandchild.id));
    assert!(matches!(cycle_err, Err(VaultError::InvalidInput(_))));
    let self_err = db.move_folder(&root.id, Some(&root.id));
    assert!(matches!(self_err, Err(VaultError::InvalidInput(_))));

    // Valid move: grandchild -> other_root
    let moved = db
        .move_folder(&grandchild.id, Some(&other_root.id))
        .unwrap();
    assert_eq!(moved.parent_id.as_deref(), Some(other_root.id.as_str()));

    let img_id = import_id(
        &mut db,
        &make_png_bytes(50, 50, 10, 20, 30, 255),
        Some("一枚絵"),
    );
    let g1 = import_id(
        &mut db,
        &make_png_bytes(50, 50, 40, 20, 30, 255),
        Some("p1"),
    );
    let g2 = import_id(
        &mut db,
        &make_png_bytes(50, 50, 70, 20, 30, 255),
        Some("p2"),
    );
    let group = db
        .create_group_from_items(&[g1, g2], Some("単行本1巻"), None)
        .unwrap();

    // Initially both are Uncategorized
    assert_eq!(db.status().unwrap().uncategorized_count, 2);

    // Assign both to 2 folders simultaneously
    let refs = vec![
        EntryRef {
            kind: EntryKind::Image,
            id: img_id.clone(),
        },
        EntryRef {
            kind: EntryKind::Group,
            id: group.id.clone(),
        },
    ];
    db.add_entries_to_folder(&child.id, &refs).unwrap();
    db.add_entries_to_folder(&other_root.id, &refs).unwrap();
    assert_eq!(db.status().unwrap().uncategorized_count, 0);

    // Remove from child ONLY -> still exists in other_root & vault
    db.remove_entries_from_folder(&child.id, &refs).unwrap();
    assert_eq!(db.status().unwrap().uncategorized_count, 0);
    let in_other = db
        .query_entries(&SearchQuery {
            scope: QueryScope::Folder {
                folder_id: other_root.id.clone(),
            },
            ..Default::default()
        })
        .unwrap();
    assert_eq!(in_other.len(), 2);

    // Deleting folder removes links ONLY; items & groups remain safely in Uncategorized
    db.delete_folder(&other_root.id).unwrap();
    assert_eq!(db.status().unwrap().item_count, 2);
    assert_eq!(db.status().unwrap().uncategorized_count, 2);
}

#[test]
fn test_manga_groups_pages_and_ungroup() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("manga.vault");
    let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();

    let p1 = import_id(
        &mut db,
        &make_png_bytes(80, 120, 10, 10, 10, 255),
        Some("p1"),
    );
    let p2 = import_id(
        &mut db,
        &make_png_bytes(80, 120, 50, 10, 10, 255),
        Some("p2"),
    );
    let p3 = import_id(
        &mut db,
        &make_png_bytes(80, 120, 90, 10, 10, 255),
        Some("p3"),
    );
    db.add_tag(&p1, "漫画").unwrap();

    let folder = db.create_folder(None, "本棚", "").unwrap();
    db.add_entries_to_folder(
        &folder.id,
        &[EntryRef {
            kind: EntryKind::Image,
            id: p1.clone(),
        }],
    )
    .unwrap();

    let group = db
        .create_group_from_items(&[p1.clone(), p2.clone()], Some("第1話"), None)
        .unwrap();
    assert_eq!(group.page_count, 2);
    assert_eq!(group.tags, vec!["漫画".to_string()]);
    assert_eq!(group.folder_ids, vec![folder.id.clone()]);

    // Gallery must show group (1 entry) + p3 (1 entry), NOT p1/p2 standalone
    let all = db.query_entries(&SearchQuery::default()).unwrap();
    assert_eq!(all.len(), 2);
    let g_entry = all.iter().find(|e| e.kind == EntryKind::Group).unwrap();
    assert_eq!(g_entry.page_count, 2);

    // Add p3 and reorder [p3, p1, p2]
    db.add_items_to_group(&group.id, std::slice::from_ref(&p3))
        .unwrap();
    let reordered = db
        .reorder_group_pages(&group.id, &[p3.clone(), p1.clone(), p2.clone()])
        .unwrap();
    assert_eq!(
        reordered
            .pages
            .iter()
            .map(|p| p.item_id.as_str())
            .collect::<Vec<_>>(),
        vec![p3.as_str(), p1.as_str(), p2.as_str()]
    );

    // Remove p3 -> returns to standalone with group's tags & folder!
    let after_remove = db.remove_page_from_group(&group.id, &p3).unwrap().unwrap();
    assert_eq!(after_remove.page_count, 2);
    let p3_detail = db.get_item_detail(&p3).unwrap();
    assert_eq!(p3_detail.tags, vec!["漫画".to_string()]);
    assert_eq!(p3_detail.folder_ids, vec![folder.id.clone()]);

    // Update reader prefs
    let prefs = db
        .set_group_reader_prefs(&group.id, "ltr", "spread", false)
        .unwrap();
    assert_eq!(prefs.reading_dir, "ltr");
    assert_eq!(prefs.spread_mode, "spread");
    assert!(!prefs.cover_alone);

    // Ungroup restores remaining items back to standalone with tags & folder preserved
    let released = db.ungroup(&group.id).unwrap();
    assert_eq!(released, vec![p1.clone(), p2.clone()]);
    assert!(matches!(
        db.get_group_detail(&group.id),
        Err(VaultError::NotFound(_))
    ));
    assert_eq!(
        db.get_item_detail(&p2).unwrap().tags,
        vec!["漫画".to_string()]
    );
    assert_eq!(db.query_entries(&SearchQuery::default()).unwrap().len(), 3);
}

#[test]
fn test_trash_restore_and_permanent_delete() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("trash.vault");
    let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();

    let standalone = import_id(
        &mut db,
        &make_png_bytes(40, 40, 20, 40, 60, 255),
        Some("solo"),
    );
    let gp1 = import_id(
        &mut db,
        &make_png_bytes(40, 40, 80, 40, 60, 255),
        Some("g1"),
    );
    let gp2 = import_id(
        &mut db,
        &make_png_bytes(40, 40, 140, 40, 60, 255),
        Some("g2"),
    );
    let group = db
        .create_group_from_items(&[gp1.clone(), gp2.clone()], Some("trash-book"), None)
        .unwrap();
    db.add_group_tag(&group.id, "ONLY_TRASH_TAG").unwrap();

    let folder = db.create_folder(None, "書庫", "").unwrap();
    db.add_entries_to_folder(
        &folder.id,
        &[EntryRef {
            kind: EntryKind::Group,
            id: group.id.clone(),
        }],
    )
    .unwrap();

    // Move both to trash
    db.move_entries_to_trash(&[
        EntryRef {
            kind: EntryKind::Image,
            id: standalone.clone(),
        },
        EntryRef {
            kind: EntryKind::Group,
            id: group.id.clone(),
        },
    ])
    .unwrap();

    assert_eq!(db.status().unwrap().item_count, 0);
    assert_eq!(db.status().unwrap().trash_count, 2);
    assert!(db
        .query_entries(&SearchQuery {
            scope: QueryScope::Folder {
                folder_id: folder.id.clone()
            },
            ..Default::default()
        })
        .unwrap()
        .is_empty());

    let trashed = db
        .query_entries(&SearchQuery {
            scope: QueryScope::Trash,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(trashed.len(), 2);

    // Restore group
    db.restore_entries_from_trash(&[EntryRef {
        kind: EntryKind::Group,
        id: group.id.clone(),
    }])
    .unwrap();
    assert_eq!(db.status().unwrap().item_count, 1);
    assert_eq!(db.status().unwrap().trash_count, 1);
    assert_eq!(
        db.query_entries(&SearchQuery {
            scope: QueryScope::Folder {
                folder_id: folder.id.clone()
            },
            ..Default::default()
        })
        .unwrap()
        .len(),
        1
    );

    // Move group back to trash & empty trash
    db.move_entries_to_trash(&[EntryRef {
        kind: EntryKind::Group,
        id: group.id.clone(),
    }])
    .unwrap();
    db.empty_trash().unwrap();

    assert_eq!(db.status().unwrap().item_count, 0);
    assert_eq!(db.status().unwrap().trash_count, 0);
    assert!(matches!(
        db.get_item_detail(&standalone),
        Err(VaultError::NotFound(_))
    ));
    assert!(matches!(
        db.get_item_detail(&gp1),
        Err(VaultError::NotFound(_))
    ));
    assert!(db.list_tags().unwrap().is_empty());
}

#[test]
fn test_lock_settings_persistence_and_natural_sort() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("settings.vault");

    {
        let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();
        let def = db.get_lock_settings();
        assert_eq!(def.shortcut, "CommandOrControl+Shift+L");
        assert_eq!(def.idle_timeout_minutes, 5);

        db.set_lock_settings(&LockSettings {
            shortcut: "Alt+Shift+L".to_string(),
            idle_timeout_minutes: 10,
        })
        .unwrap();
    }

    let db = VaultDb::open(&vault_path, "pass1234").unwrap();
    let loaded = db.get_lock_settings();
    assert_eq!(loaded.shortcut, "Alt+Shift+L");
    assert_eq!(loaded.idle_timeout_minutes, 10);

    assert_eq!(natural_cmp("p2.png", "p10.png"), Ordering::Less);
    assert_eq!(natural_cmp("第2話", "第10話"), Ordering::Less);
    assert_eq!(natural_cmp("Page01", "page1"), Ordering::Greater);
}

fn make_noisy_png_bytes(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let mut img = RgbaImage::new(width, height);
    let mut state = seed.wrapping_add(1);
    for (_, _, px) in img.enumerate_pixels_mut() {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        let r = (state >> 24) as u8;
        let g = (state >> 16) as u8;
        let b = (state >> 8) as u8;
        *px = Rgba([r, g, b, 255]);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .unwrap();
    out
}

#[test]
fn test_health_optimize_and_atomic_backup() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("optimize.vault");
    let backup_path = dir.path().join("backup.vault");

    let mut keep_id = String::new();
    let mut delete_ids = Vec::new();

    {
        let mut db = VaultDb::create(&vault_path, "OptSecret!42", false).unwrap();
        for i in 0..6u32 {
            let png = make_noisy_png_bytes(220, 220, i * 97 + 13);
            let id = import_id(&mut db, &png, Some(&format!("noisy_{i}")));
            if i == 0 {
                keep_id = id;
                db.add_tag(&keep_id, "KEEP_TAG_SECRET").unwrap();
            } else {
                delete_ids.push(EntryRef {
                    kind: EntryKind::Image,
                    id,
                });
            }
        }

        db.permanently_delete_entries(&delete_ids).unwrap();
        let health_before = db.health_info().unwrap();
        assert!(health_before.integrity_ok);
        assert!(health_before.reclaimable_bytes > 0);

        let opt = db.optimize().unwrap();
        assert!(opt.integrity_ok);
        assert!(opt.size_after_bytes < opt.size_before_bytes);
        assert!(opt.reclaimed_bytes > 0);

        let health_after = db.health_info().unwrap();
        assert!(health_after.integrity_ok);
        assert_eq!(health_after.freelist_count, 0);

        let backup = db.backup_to(&backup_path, false).unwrap();
        assert_eq!(backup.bytes_written, opt.size_after_bytes);

        let dup_backup = db.backup_to(&backup_path, false);
        assert!(matches!(dup_backup, Err(VaultError::VaultAlreadyExists(_))));
    }

    // Verify raw file encryption intact & no temp/journal files left
    let raw = fs::read(&vault_path).unwrap();
    assert!(!contains_subsequence(&raw, b"KEEP_TAG_SECRET"));
    assert!(!contains_subsequence(
        &raw,
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
    ));

    let files: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(files.len(), 2);

    // Verify backup opens cleanly with same password and intact item
    let backup_db = VaultDb::open(&backup_path, "OptSecret!42").unwrap();
    assert_eq!(backup_db.status().unwrap().item_count, 1);
    assert_eq!(
        backup_db.get_item_detail(&keep_id).unwrap().tags,
        vec!["KEEP_TAG_SECRET".to_string()]
    );
}

fn make_minimal_mp4_bytes(tag: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&20u32.to_be_bytes());
    bytes.extend_from_slice(b"ftyp");
    bytes.extend_from_slice(b"isom");
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"mp42");
    bytes.extend_from_slice(tag);
    bytes
}

fn make_minimal_mov_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&20u32.to_be_bytes());
    bytes.extend_from_slice(b"ftyp");
    bytes.extend_from_slice(b"qt  ");
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"qt  ");
    bytes.extend_from_slice(b"MOV_PAYLOAD");
    bytes
}

fn make_minimal_webm_bytes() -> Vec<u8> {
    let mut bytes = vec![0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x82, 0x84];
    bytes.extend_from_slice(b"webm");
    bytes.extend_from_slice(b"WEBM_PAYLOAD");
    bytes
}

#[test]
fn test_migrate_v2_to_v3() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("legacy_v2.vault");
    let png = make_png_bytes(64, 64, 10, 120, 200, 255);
    let item_id = Uuid::new_v4().to_string();

    {
        let conn = Connection::open(&vault_path).unwrap();
        apply_sqlcipher_key(&conn, "v2-pass").unwrap();
        apply_runtime_pragmas(&conn).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute(
            "INSERT INTO vault_meta (key, value) VALUES ('schema_version', '2')",
            [],
        )
        .unwrap();
        conn.execute(
                "INSERT INTO items (id, title, sha256, mime_type, width, height, byte_size, rating, memo, palette, created_at, updated_at, deleted_at)
                 VALUES (?1, 'v2画像', 'sha-v2', 'image/png', 64, 64, ?2, 3, 'm', '[]', 2000, 2000, NULL)",
                params![item_id, png.len() as i64],
            )
            .unwrap();
        conn.execute(
            "INSERT INTO blobs (item_id, data) VALUES (?1, ?2)",
            params![item_id, png],
        )
        .unwrap();
    }

    let mut db = VaultDb::open(&vault_path, "v2-pass").unwrap();
    let ver: String = db
        .conn
        .query_row(
            "SELECT value FROM vault_meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ver, "4");

    let updated = db
        .set_source_url(&item_id, "  https://x.com/example/status/1  ")
        .unwrap();
    assert_eq!(updated.source_url, "https://x.com/example/status/1");

    db.set_url_import_settings(&UrlImportSettings {
        enabled: true,
        ytdlp_path: "  /opt/homebrew/bin/yt-dlp  ".to_string(),
        ..Default::default()
    })
    .unwrap();
    drop(db);

    let reopened = VaultDb::open(&vault_path, "v2-pass").unwrap();
    let st = reopened.status().unwrap();
    assert!(st.url_import_settings.enabled);
    assert_eq!(
        st.url_import_settings.ytdlp_path,
        "/opt/homebrew/bin/yt-dlp"
    );
}

#[test]
fn test_migrate_v3_to_v4_and_chunked_media_range() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("legacy_v3.vault");
    let png = make_png_bytes(48, 48, 20, 90, 180, 255);
    let legacy_id = Uuid::new_v4().to_string();

    {
        let conn = Connection::open(&vault_path).unwrap();
        apply_sqlcipher_key(&conn, "v3-pass").unwrap();
        apply_runtime_pragmas(&conn).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();
        conn.execute(
            "INSERT INTO vault_meta (key, value) VALUES ('schema_version', '3')",
            [],
        )
        .unwrap();
        conn.execute(
                "INSERT INTO items (id, title, sha256, mime_type, width, height, byte_size, duration_ms, source_url, rating, memo, palette, created_at, updated_at, deleted_at)
                 VALUES (?1, 'v3画像', 'sha-v3', 'image/png', 48, 48, ?2, 0, 'https://example.com', 5, 'm', '[]', 3000, 3000, NULL)",
                params![legacy_id, png.len() as i64],
            )
            .unwrap();
        conn.execute(
            "INSERT INTO blobs (item_id, data) VALUES (?1, ?2)",
            params![legacy_id, png],
        )
        .unwrap();
    }

    let mut db = VaultDb::open(&vault_path, "v3-pass").unwrap();
    let ver: String = db
        .conn
        .query_row(
            "SELECT value FROM vault_meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ver, "4");
    assert_eq!(db.get_blob_bytes(&legacy_id).unwrap(), png);

    // Build a synthetic >10 KB MP4 payload and store via 4096-byte chunks
    let mut big_mp4 = make_minimal_mp4_bytes(b"");
    for i in 0..10_000u32 {
        big_mp4.push(((i * 31 + 7) % 251) as u8);
    }
    let video_path = dir.path().join("chunked.mp4");
    fs::write(&video_path, &big_mp4).unwrap();

    let chunked_id = match db
        .import_media_file_with_chunk_params(
            &video_path,
            Some("長尺テスト動画"),
            Some("https://x.com/user/status/999"),
            125_000,
            1024,
            4096,
        )
        .unwrap()
    {
        ImportOutcome::Imported {
            id,
            mime_type,
            thumb_pending,
        } => {
            assert_eq!(mime_type, "video/mp4");
            assert!(thumb_pending);
            id
        }
        _ => panic!("expected chunked import"),
    };

    // Duplicate detection via streaming file hash
    assert!(matches!(
        db.import_media_file_with_chunk_params(&video_path, Some("dup"), None, 0, 1024, 4096)
            .unwrap(),
        ImportOutcome::Duplicate
    ));

    let (mime, total, chunk_sz) = db.get_media_info(&chunked_id).unwrap();
    assert_eq!(mime, "video/mp4");
    assert_eq!(total, big_mp4.len());
    assert_eq!(chunk_sz, 4096);

    let chunk_count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM blob_chunks WHERE item_id = ?1",
            params![chunked_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(chunk_count, 3);

    // Verify byte-exact Range read across chunk boundaries (chunk 0 -> 1 -> 2)
    let cross_slice = db
        .read_media_range(&chunked_id, 3500, 9200, chunk_sz)
        .unwrap();
    assert_eq!(cross_slice, big_mp4[3500..=9200]);

    // Verify full reconstruction & cascade delete
    assert_eq!(db.get_blob_bytes(&chunked_id).unwrap(), big_mp4);
    db.permanently_delete_entries(&[EntryRef {
        kind: EntryKind::Image,
        id: chunked_id.clone(),
    }])
    .unwrap();
    let remaining_chunks: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM blob_chunks WHERE item_id = ?1",
            params![chunked_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remaining_chunks, 0);
}

#[test]
fn test_url_import_settings_extended_persistence_and_validation() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("url_settings.vault");
    let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();

    let defaults = db.get_url_import_settings();
    assert!(!defaults.enabled);
    assert_eq!(defaults.cookie_browser, "");
    assert_eq!(defaults.max_video_size_mb, 2048);
    assert_eq!(defaults.max_video_height, 0);

    // Reject injected browser value
    let bad_browser = db.set_url_import_settings(&UrlImportSettings {
        cookie_browser: "chrome --exec=bad".to_string(),
        ..Default::default()
    });
    assert!(matches!(bad_browser, Err(VaultError::InvalidInput(_))));

    // Reject 'file' mode without path
    let missing_file = db.set_url_import_settings(&UrlImportSettings {
        cookie_browser: "file".to_string(),
        cookies_file: "".to_string(),
        ..Default::default()
    });
    assert!(matches!(missing_file, Err(VaultError::InvalidInput(_))));

    // Persist Arc + custom ffmpeg + 4GB limit + 1080p cap
    let saved = db
        .set_url_import_settings(&UrlImportSettings {
            enabled: true,
            ytdlp_path: " /opt/homebrew/bin/yt-dlp ".to_string(),
            ffmpeg_path: " /opt/homebrew/bin/ffmpeg ".to_string(),
            cookie_browser: " ARC ".to_string(),
            cookies_file: " /tmp/cookies.txt ".to_string(),
            max_video_size_mb: 4096,
            max_video_height: 1080,
        })
        .unwrap();
    assert_eq!(saved.cookie_browser, "arc");
    assert_eq!(saved.ffmpeg_path, "/opt/homebrew/bin/ffmpeg");
    assert_eq!(saved.max_video_size_mb, 4096);
    assert_eq!(saved.max_video_height, 1080);
    drop(db);

    let reopened = VaultDb::open(&vault_path, "pass1234").unwrap();
    let loaded = reopened.get_url_import_settings();
    assert_eq!(loaded, saved);
}

#[test]
fn test_video_formats_metadata_and_group_guard() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("videos.vault");
    let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();

    let mp4 = make_minimal_mp4_bytes(b"MP4_A");
    let mov = make_minimal_mov_bytes();
    let webm = make_minimal_webm_bytes();
    let avif = make_minimal_avif_bytes();

    assert_eq!(crate::imaging::detect_mime(&mp4).unwrap(), "video/mp4");
    assert_eq!(
        crate::imaging::detect_mime(&mov).unwrap(),
        "video/quicktime"
    );
    assert_eq!(crate::imaging::detect_mime(&webm).unwrap(), "video/webm");
    assert_eq!(crate::imaging::detect_mime(&avif).unwrap(), "image/avif");

    let mp4_id = match db
        .import_media_bytes(
            &mp4,
            Some("クリップA"),
            Some("https://www.tiktok.com/@user/video/123"),
            0,
        )
        .unwrap()
    {
        ImportOutcome::Imported {
            id,
            mime_type,
            thumb_pending,
        } => {
            assert_eq!(mime_type, "video/mp4");
            assert!(thumb_pending);
            id
        }
        _ => panic!("expected imported mp4"),
    };

    let mov_id = import_id(&mut db, &mov, Some("MOV"));
    let webm_id = import_id(&mut db, &webm, Some("WEBM"));
    assert_eq!(
        db.get_item_detail(&mov_id).unwrap().mime_type,
        "video/quicktime"
    );
    assert_eq!(
        db.get_item_detail(&webm_id).unwrap().mime_type,
        "video/webm"
    );

    let client_png = make_png_bytes(320, 180, 90, 140, 240, 255);
    let updated = db
        .save_media_thumbnail(&mp4_id, &client_png, 1080, 1920, 15_200)
        .unwrap();
    assert!(updated.has_thumbnail);
    assert_eq!((updated.width, updated.height), (1080, 1920));
    assert_eq!(updated.duration_ms, 15_200);
    assert_eq!(updated.source_url, "https://www.tiktok.com/@user/video/123");

    let hits = db
        .query_entries(&SearchQuery {
            text: "tiktok.com".to_string(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, mp4_id);
    assert_eq!(hits[0].duration_ms, 15_200);

    let (stream_mime, stream_bytes) = db.get_media_blob_with_mime(&mp4_id).unwrap();
    assert_eq!(stream_mime, "video/mp4");
    assert_eq!(stream_bytes, mp4);

    let png = make_png_bytes(50, 50, 10, 20, 30, 255);
    let png_id = import_id(&mut db, &png, Some("絵"));
    let err = db.create_group_from_items(&[png_id, mp4_id], Some("混在"), None);
    assert!(matches!(err, Err(VaultError::InvalidInput(_))));
}

#[test]
fn test_skip_plain_export_confirm_persistence() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("skip_confirm.vault");
    {
        let mut db = VaultDb::create(&vault_path, "pass-abc", false).unwrap();
        assert!(!db.get_skip_plain_export_confirm());
        assert!(!db.status().unwrap().skip_plain_export_confirm);
        assert!(db.set_skip_plain_export_confirm(true).unwrap());
        assert!(db.status().unwrap().skip_plain_export_confirm);
    }
    let mut reopened = VaultDb::open(&vault_path, "pass-abc").unwrap();
    assert!(reopened.get_skip_plain_export_confirm());
    assert!(reopened.status().unwrap().skip_plain_export_confirm);
    assert!(!reopened.set_skip_plain_export_confirm(false).unwrap());
    assert!(!reopened.status().unwrap().skip_plain_export_confirm);
}

#[test]
fn test_thumbnail_regen_targets_stats_and_dimension_overwrite() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("thumb_regen.vault");
    let mut db = VaultDb::create(&vault_path, "pass-regen", false).unwrap();

    let png_single = make_png_bytes(120, 90, 20, 120, 220, 255);
    let png_p1 = make_png_bytes(100, 140, 40, 200, 60, 255);
    let png_p2 = make_png_bytes(100, 140, 220, 80, 40, 255);
    let png_trashed = make_png_bytes(80, 80, 200, 200, 20, 255);
    let avif = make_minimal_avif_bytes();
    let mp4 = make_minimal_mp4_bytes(b"THUMB_MP4");
    let webm = make_minimal_webm_bytes();

    let single_id = import_id(&mut db, &png_single, Some("SinglePng"));
    let p1_id = import_id(&mut db, &png_p1, Some("Page1"));
    let p2_id = import_id(&mut db, &png_p2, Some("Page2"));
    let trashed_id = import_id(&mut db, &png_trashed, Some("Trashed"));
    let avif_id = import_id(&mut db, &avif, Some("AvifPending"));
    let mp4_id = import_id(&mut db, &mp4, Some("Mp4Pending"));
    let webm_id = import_id(&mut db, &webm, Some("WebmVideo"));

    let group = db
        .create_group_from_items(&[p1_id.clone(), p2_id.clone()], Some("Manga"), None)
        .unwrap();

    // Give webm a bogus 320x240 thumbnail first, then overwrite with real 1920x1080, then preserve on 0x0
    let thumb_png = make_png_bytes(320, 240, 90, 150, 210, 255);
    let bogus = db
        .save_media_thumbnail(&webm_id, &thumb_png, 320, 240, 1_000)
        .unwrap();
    assert!(bogus.has_thumbnail);
    assert_eq!((bogus.width, bogus.height), (320, 240));

    let fixed = db
        .save_media_thumbnail(&webm_id, &thumb_png, 1920, 1080, 4_500)
        .unwrap();
    assert_eq!((fixed.width, fixed.height), (1920, 1080));
    assert_eq!(fixed.duration_ms, 4_500);

    let kept = db
        .save_media_thumbnail(&webm_id, &thumb_png, 0, 0, 0)
        .unwrap();
    assert_eq!((kept.width, kept.height), (1920, 1080));
    assert_eq!(kept.duration_ms, 4_500);

    // Move trashed_id to trash; it must be excluded from all target scopes & stats
    db.move_entries_to_trash(&[EntryRef {
        kind: EntryKind::Image,
        id: trashed_id.clone(),
    }])
    .unwrap();

    let stats_ff = db.thumbnail_stats(true).unwrap();
    assert_eq!(stats_ff.total_items, 6);
    assert_eq!(stats_ff.missing_count, 2); // avif_id + mp4_id
    assert_eq!(stats_ff.video_count, 2); // mp4_id + webm_id
    assert!(stats_ff.ffmpeg_available);

    let stats_no_ff = db.thumbnail_stats(false).unwrap();
    assert!(!stats_no_ff.ffmpeg_available);

    // Scope: Entries (expands group in page_order, deduplicates repeated refs, excludes trashed)
    let entry_targets = db
        .list_thumbnail_target_items(
            ThumbnailRegenScope::Entries,
            &[
                EntryRef {
                    kind: EntryKind::Image,
                    id: single_id.clone(),
                },
                EntryRef {
                    kind: EntryKind::Group,
                    id: group.id.clone(),
                },
                EntryRef {
                    kind: EntryKind::Image,
                    id: p1_id.clone(),
                },
                EntryRef {
                    kind: EntryKind::Image,
                    id: trashed_id.clone(),
                },
            ],
        )
        .unwrap();
    let entry_ids: Vec<&str> = entry_targets.iter().map(|t| t.item_id.as_str()).collect();
    assert_eq!(
        entry_ids,
        vec![single_id.as_str(), p1_id.as_str(), p2_id.as_str()]
    );

    // Scope: MissingOnly
    let missing_targets = db
        .list_thumbnail_target_items(ThumbnailRegenScope::MissingOnly, &[])
        .unwrap();
    let mut missing_ids: Vec<&str> = missing_targets.iter().map(|t| t.item_id.as_str()).collect();
    missing_ids.sort();
    let mut expected_missing = vec![avif_id.as_str(), mp4_id.as_str()];
    expected_missing.sort();
    assert_eq!(missing_ids, expected_missing);

    // Scope: VideosOnly
    let video_targets = db
        .list_thumbnail_target_items(ThumbnailRegenScope::VideosOnly, &[])
        .unwrap();
    let mut video_ids: Vec<&str> = video_targets.iter().map(|t| t.item_id.as_str()).collect();
    video_ids.sort();
    let mut expected_videos = vec![mp4_id.as_str(), webm_id.as_str()];
    expected_videos.sort();
    assert_eq!(video_ids, expected_videos);

    // Scope: All
    let all_targets = db
        .list_thumbnail_target_items(ThumbnailRegenScope::All, &[])
        .unwrap();
    assert_eq!(all_targets.len(), 6);
    assert!(!all_targets.iter().any(|t| t.item_id == trashed_id));
}
