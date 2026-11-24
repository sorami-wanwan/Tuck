use std::fs;
use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use rusqlite::params;
use tempfile::TempDir;

use crate::db::{ImportOutcome, VaultDb};
use crate::error::VaultError;
use crate::models::{
    EntryKind, EntryRef, PackageExportRequest, PackageImportRequest, QueryScope, SearchQuery,
};

use super::*;

fn make_png(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut img = RgbaImage::new(width, height);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = Rgba([
            r.wrapping_add((x % 30) as u8),
            g.wrapping_add((y % 30) as u8),
            b,
            255,
        ]);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .unwrap();
    out
}

fn import_id(db: &mut VaultDb, bytes: &[u8], title: Option<&str>) -> String {
    match db.import_bytes(bytes, title).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        ImportOutcome::Duplicate => panic!("unexpected duplicate"),
    }
}

fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn test_partial_package_export_and_import_merge() {
    let dir = TempDir::new().unwrap();
    let vault_a_path = dir.path().join("vault_a.vault");
    let pkg_path = dir.path().join("exchange.tuckpack");
    let vault_b_path = dir.path().join("vault_b.vault");

    let img1_bytes = make_png(80, 80, 20, 100, 200);
    let p1_bytes = make_png(80, 110, 200, 50, 50);
    let p2_bytes = make_png(80, 110, 50, 200, 50);
    let other_bytes = make_png(60, 60, 120, 120, 120);

    // 1. Populate Source Vault A
    {
        let mut db_a = VaultDb::create(&vault_a_path, "vault-A-pass", false).unwrap();
        let f_root = db_a.create_folder(None, "作家X", "#4c8dff").unwrap();
        let f_sub = db_a.create_folder(Some(&f_root.id), "単行本", "").unwrap();

        let img1 = import_id(&mut db_a, &img1_bytes, Some("イラスト1"));
        db_a.add_tag(&img1, "風景").unwrap();
        db_a.add_tag(&img1, "夜").unwrap();
        db_a.set_rating(&img1, 5).unwrap();
        db_a.add_entries_to_folder(
            &f_root.id,
            &[EntryRef {
                kind: EntryKind::Image,
                id: img1,
            }],
        )
        .unwrap();

        let gp1 = import_id(&mut db_a, &p1_bytes, Some("m1"));
        let gp2 = import_id(&mut db_a, &p2_bytes, Some("m2"));
        let manga = db_a
            .create_group_from_items(&[gp1, gp2], Some("単行本1巻"), Some(&f_sub.id))
            .unwrap();
        db_a.add_group_tag(&manga.id, "アクション").unwrap();
        db_a.set_group_reader_prefs(&manga.id, "ltr", "spread", false)
            .unwrap();

        let other = import_id(&mut db_a, &other_bytes, Some("除外イラスト"));
        db_a.add_tag(&other, "除外").unwrap();

        let report = export_partial_package(
            &db_a,
            &PackageExportRequest {
                dest_path: pkg_path.display().to_string(),
                passphrase: "share-pass-99".to_string(),
                overwrite: false,
                folder_ids: vec![f_root.id],
                include_subfolders: true,
                tag_names: vec![],
                entries: vec![],
            },
        )
        .unwrap();

        assert_eq!(report.item_count, 1);
        assert_eq!(report.group_count, 1);
        assert_eq!(report.page_count, 2);
        assert_eq!(report.folder_count, 2);
        assert_eq!(report.tag_count, 3);
    }

    // Verify .tuckpack encryption at rest & password isolation
    let raw_pkg = fs::read(&pkg_path).unwrap();
    assert!(!contains_subsequence(&raw_pkg, "作家X".as_bytes()));
    assert!(!contains_subsequence(&raw_pkg, "風景".as_bytes()));
    assert!(!contains_subsequence(&raw_pkg, "アクション".as_bytes()));
    assert!(!contains_subsequence(
        &raw_pkg,
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
    ));
    assert!(matches!(
        VaultDb::open(&pkg_path, "vault-A-pass"),
        Err(VaultError::WrongPassword)
    ));
    assert!(VaultDb::open(&pkg_path, "share-pass-99").is_ok());

    // 2. Import into Destination Vault B (pre-populated with duplicate img1 + existing tag)
    let mut db_b = VaultDb::create(&vault_b_path, "vault-B-pass", false).unwrap();
    let existing_img1 = import_id(&mut db_b, &img1_bytes, Some("既存タイトル"));
    db_b.add_tag(&existing_img1, "既存タグ").unwrap();

    let imp1 = import_partial_package(
        &mut db_b,
        &PackageImportRequest {
            source_path: pkg_path.display().to_string(),
            passphrase: "share-pass-99".to_string(),
            target_folder_id: None,
        },
    )
    .unwrap();

    assert_eq!(imp1.merged_duplicate_items, 1);
    assert_eq!(imp1.imported_items, 2); // 2 manga pages
    assert_eq!(imp1.imported_groups, 1);
    assert_eq!(imp1.imported_folders, 2);

    // Verify merged tags & existing title preserved
    let img1_detail = db_b.get_item_detail(&existing_img1).unwrap();
    assert_eq!(img1_detail.title, "既存タイトル");
    assert!(img1_detail.tags.contains(&"既存タグ".to_string()));
    assert!(img1_detail.tags.contains(&"風景".to_string()));
    assert!(img1_detail.tags.contains(&"夜".to_string()));

    // Verify manga group & nested folders merged intact
    let entries_b = db_b.query_entries(&SearchQuery::default()).unwrap();
    assert_eq!(entries_b.len(), 2); // img1 + manga1 (no "除外イラスト"!)
    let manga_entry = entries_b
        .iter()
        .find(|e| e.kind == EntryKind::Group)
        .unwrap();
    assert_eq!(manga_entry.page_count, 2);
    let manga_detail = db_b.get_group_detail(&manga_entry.id).unwrap();
    assert_eq!(manga_detail.reading_dir, "ltr");
    assert_eq!(manga_detail.spread_mode, "spread");
    assert!(!manga_detail.cover_alone);
    assert_eq!(manga_detail.tags, vec!["アクション".to_string()]);

    let folders_b = db_b.list_folders().unwrap();
    assert_eq!(folders_b.len(), 2);
    let root_b = folders_b.iter().find(|f| f.name == "作家X").unwrap();
    let sub_b = folders_b.iter().find(|f| f.name == "単行本").unwrap();
    assert_eq!(sub_b.parent_id.as_deref(), Some(root_b.id.as_str()));

    // 3. Re-import same package -> must be idempotent
    let imp2 = import_partial_package(
        &mut db_b,
        &PackageImportRequest {
            source_path: pkg_path.display().to_string(),
            passphrase: "share-pass-99".to_string(),
            target_folder_id: None,
        },
    )
    .unwrap();
    assert_eq!(imp2.imported_items, 0);
    assert_eq!(imp2.imported_groups, 0);
    assert_eq!(imp2.skipped_duplicate_groups, 1);
    assert_eq!(imp2.imported_folders, 0);
    assert_eq!(
        db_b.query_entries(&SearchQuery::default()).unwrap().len(),
        2
    );
}

#[test]
fn test_package_export_by_tags_and_entries() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("source.vault");
    // Verify legacy .kvpack extension remains import-compatible as well
    let pkg_path = dir.path().join("tags_only.kvpack");

    let mut db = VaultDb::create(&vault_path, "pass1234", false).unwrap();
    let img_id = import_id(&mut db, &make_png(40, 40, 10, 20, 30), Some("A"));
    let b = import_id(&mut db, &make_png(40, 40, 50, 20, 30), Some("B"));
    let _c = import_id(&mut db, &make_png(40, 40, 90, 20, 30), Some("C"));
    db.add_tag(&img_id, "お気に入り").unwrap();

    let mut mp4 = Vec::new();
    mp4.extend_from_slice(&20u32.to_be_bytes());
    mp4.extend_from_slice(b"ftyp");
    mp4.extend_from_slice(b"isom");
    mp4.extend_from_slice(&0u32.to_be_bytes());
    mp4.extend_from_slice(b"mp42");
    mp4.extend_from_slice(b"PKG_VIDEO");
    let vid = match db
        .import_media_bytes(
            &mp4,
            Some("Video"),
            Some("https://www.tiktok.com/@pkg/video/77"),
            9_500,
        )
        .unwrap()
    {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected imported video"),
    };
    db.add_tag(&vid, "お気に入り").unwrap();

    // Empty selection must error cleanly
    let empty_err = export_partial_package(
        &db,
        &PackageExportRequest {
            dest_path: pkg_path.display().to_string(),
            passphrase: "p1".to_string(),
            overwrite: true,
            folder_ids: vec![],
            include_subfolders: true,
            tag_names: vec!["存在しないタグ".to_string()],
            entries: vec![],
        },
    );
    assert!(matches!(empty_err, Err(VaultError::InvalidInput(_))));

    // Combine tag_names + explicit entry
    let rep = export_partial_package(
        &db,
        &PackageExportRequest {
            dest_path: pkg_path.display().to_string(),
            passphrase: "pkg-pass".to_string(),
            overwrite: true,
            folder_ids: vec![],
            include_subfolders: true,
            tag_names: vec!["お気に入り".to_string()],
            entries: vec![EntryRef {
                kind: EntryKind::Image,
                id: b,
            }],
        },
    )
    .unwrap();
    assert_eq!(rep.item_count, 3);
    assert_eq!(rep.tag_count, 1);

    let pkg_db = VaultDb::open(&pkg_path, "pkg-pass").unwrap();
    let items = pkg_db
        .query_entries(&SearchQuery {
            scope: QueryScope::All,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(items.len(), 3);
    let pkg_vid = pkg_db.get_item_detail(&vid).unwrap();
    assert_eq!(pkg_vid.duration_ms, 9_500);
    assert_eq!(pkg_vid.source_url, "https://www.tiktok.com/@pkg/video/77");

    // Verify importing video package preserves duration_ms, source_url, and reports video_pending_ids
    let dst_path = dir.path().join("dest.vault");
    let mut dst_db = VaultDb::create(&dst_path, "dst-pass", false).unwrap();
    let imp = import_partial_package(
        &mut dst_db,
        &PackageImportRequest {
            source_path: pkg_path.display().to_string(),
            passphrase: "pkg-pass".to_string(),
            target_folder_id: None,
        },
    )
    .unwrap();
    assert_eq!(imp.imported_items, 3);
    assert_eq!(imp.video_pending_ids, vec![vid.clone()]);
    let imported_vid = dst_db.get_item_detail(&vid).unwrap();
    assert_eq!(imported_vid.duration_ms, 9_500);
    assert_eq!(
        imported_vid.source_url,
        "https://www.tiktok.com/@pkg/video/77"
    );
}

#[test]
fn test_package_export_import_chunked_video() {
    let dir = TempDir::new().unwrap();
    let src_vault = dir.path().join("src_chunk.vault");
    let pkg_path = dir.path().join("chunked.tuckpack");
    let dst_vault = dir.path().join("dst_chunk.vault");
    let video_file = dir.path().join("sample_chunk.mp4");

    let mut payload = Vec::new();
    payload.extend_from_slice(&20u32.to_be_bytes());
    payload.extend_from_slice(b"ftyp");
    payload.extend_from_slice(b"isom");
    payload.extend_from_slice(&0u32.to_be_bytes());
    payload.extend_from_slice(b"mp42");
    for i in 0..10_000u32 {
        payload.push(((i * 19 + 5) % 253) as u8);
    }
    fs::write(&video_file, &payload).unwrap();

    let mut src_db = VaultDb::create(&src_vault, "src-pass", false).unwrap();
    let vid = match src_db
        .import_media_file_with_chunk_params(
            &video_file,
            Some("チャンク動画"),
            Some("https://x.com/example/status/777"),
            180_000,
            1024,
            4096,
        )
        .unwrap()
    {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected imported chunked video"),
    };

    export_partial_package(
        &src_db,
        &PackageExportRequest {
            dest_path: pkg_path.display().to_string(),
            passphrase: "pkg-chunk-pass".to_string(),
            overwrite: true,
            folder_ids: vec![],
            include_subfolders: true,
            tag_names: vec![],
            entries: vec![EntryRef {
                kind: EntryKind::Image,
                id: vid.clone(),
            }],
        },
    )
    .unwrap();

    let mut dst_db = VaultDb::create(&dst_vault, "dst-pass", false).unwrap();
    let imp = import_partial_package(
        &mut dst_db,
        &PackageImportRequest {
            source_path: pkg_path.display().to_string(),
            passphrase: "pkg-chunk-pass".to_string(),
            target_folder_id: None,
        },
    )
    .unwrap();
    assert_eq!(imp.imported_items, 1);
    assert_eq!(imp.video_pending_ids, vec![vid.clone()]);
    assert_eq!(dst_db.get_blob_bytes(&vid).unwrap(), payload);
    let detail = dst_db.get_item_detail(&vid).unwrap();
    assert_eq!(detail.duration_ms, 180_000);
    assert_eq!(detail.source_url, "https://x.com/example/status/777");
}

#[test]
fn test_package_import_readonly_restore_group_and_preserve_standalone_links() {
    let dir = TempDir::new().unwrap();
    let src_vault = dir.path().join("src_preserve.vault");
    let pkg_path = dir.path().join("readonly_preserve.tuckpack");
    let dst_vault = dir.path().join("dst_preserve.vault");

    let p1_bytes = make_png(24, 24, 15, 25, 35);
    let p2_bytes = make_png(26, 24, 45, 55, 65);
    let p3_bytes = make_png(28, 24, 75, 85, 95);

    let mut src_db = VaultDb::create(&src_vault, "src-pass", false).unwrap();
    let s1 = match src_db.import_bytes(&p1_bytes, Some("p1")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected s1"),
    };
    let s2 = match src_db.import_bytes(&p2_bytes, Some("p2")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected s2"),
    };
    let s3 = match src_db.import_bytes(&p3_bytes, Some("p3")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected s3"),
    };
    let src_grp_a = src_db
        .create_group_from_items(&[s1.clone(), s2.clone()], Some("連載A"), None)
        .unwrap();
    let src_grp_b = src_db
        .create_group_from_items(std::slice::from_ref(&s3), Some("連載B"), None)
        .unwrap();
    src_db.set_group_memo(&src_grp_b.id, "連載メモ").unwrap();
    src_db.set_group_rating(&src_grp_b.id, 4).unwrap();

    export_partial_package(
        &src_db,
        &PackageExportRequest {
            dest_path: pkg_path.display().to_string(),
            passphrase: "pkg-ro-pass".to_string(),
            overwrite: true,
            folder_ids: vec![],
            include_subfolders: true,
            tag_names: vec![],
            entries: vec![
                EntryRef {
                    kind: EntryKind::Group,
                    id: src_grp_a.id.clone(),
                },
                EntryRef {
                    kind: EntryKind::Group,
                    id: src_grp_b.id.clone(),
                },
            ],
        },
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&pkg_path).unwrap().permissions();
        perms.set_mode(0o444);
        fs::set_permissions(&pkg_path, perms).unwrap();
    }

    // Destination already has p1 standalone (with tag + folder) AND p3 in a trashed group
    let mut dst_db = VaultDb::create(&dst_vault, "dst-pass", false).unwrap();
    let d1 = match dst_db.import_bytes(&p1_bytes, Some("d1")).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected d1"),
    };
    let solo_dir = dst_db.create_folder(None, "既存フォルダ", "").unwrap();
    dst_db.add_tag(&d1, "単体タグ").unwrap();
    dst_db
        .add_entries_to_folder(
            &solo_dir.id,
            &[EntryRef {
                kind: EntryKind::Image,
                id: d1,
            }],
        )
        .unwrap();

    let d3 = match dst_db.import_bytes(&p3_bytes, None).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected d3"),
    };
    let dst_grp_b = dst_db
        .create_group_from_items(&[d3], None, None)
        .unwrap();
    dst_db
        .conn()
        .execute(
            "UPDATE groups SET title = '' WHERE id = ?1",
            params![dst_grp_b.id],
        )
        .unwrap();
    dst_db
        .move_entries_to_trash(&[EntryRef {
            kind: EntryKind::Group,
            id: dst_grp_b.id.clone(),
        }])
        .unwrap();

    let imp = import_partial_package(
        &mut dst_db,
        &PackageImportRequest {
            source_path: pkg_path.display().to_string(),
            passphrase: "pkg-ro-pass".to_string(),
            target_folder_id: None,
        },
    )
    .unwrap();
    assert_eq!(imp.imported_groups, 1);
    assert_eq!(imp.skipped_duplicate_groups, 1);

    // Existing trashed duplicate group & metadata restored cleanly
    let restored_b = dst_db.get_group_detail(&dst_grp_b.id).unwrap();
    assert!(restored_b.deleted_at.is_none());
    assert_eq!(restored_b.title, "連載B");
    assert_eq!(restored_b.memo, "連載メモ");
    assert_eq!(restored_b.rating, 4);
    let page0_deleted: Option<i64> = dst_db
        .conn()
        .query_row(
            "SELECT deleted_at FROM items WHERE id = ?1",
            params![restored_b.pages[0].item_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(page0_deleted.is_none());

    // New group A inherited destination standalone d1's tag & folder
    let active_groups = dst_db
        .query_entries(&SearchQuery {
            scope: QueryScope::All,
            media_type: crate::models::MediaFilter::Manga,
            ..Default::default()
        })
        .unwrap();
    let imported_a = active_groups
        .into_iter()
        .find(|e| e.title == "連載A")
        .unwrap();
    let detail_a = dst_db.get_group_detail(&imported_a.id).unwrap();
    assert!(detail_a.tags.iter().any(|t| t == "単体タグ"));
    let folder_a = dst_db
        .query_entries(&SearchQuery {
            scope: QueryScope::Folder {
                folder_id: solo_dir.id,
            },
            ..Default::default()
        })
        .unwrap();
    assert!(folder_a
        .iter()
        .any(|e| e.kind == EntryKind::Group && e.id == imported_a.id));
}
