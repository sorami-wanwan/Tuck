use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use tempfile::TempDir;
use tokio::sync::mpsc;

use crate::crypto::sha256_hex;
use crate::db::{ImportOutcome, VaultDb};
use crate::error::VaultError;
use crate::models::{EntryKind, EntryRef, PlainExportBatchRequest, PlainExportItemRequest};

use super::naming::{build_group_page_filename, mime_to_extension, sanitize_name_segment};
use super::zip_stream::{crc32_update, ZIP_GPBF_DATA_DESC_UTF8};
use super::*;

fn make_png(w: u32, h: u32, color: [u8; 4]) -> Vec<u8> {
    let mut img = RgbaImage::new(w, h);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = Rgba(color);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .unwrap();
    out
}

fn make_jpeg(w: u32, h: u32, color: [u8; 3]) -> Vec<u8> {
    let mut img = image::RgbImage::new(w, h);
    for (_, _, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb(color);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
        .unwrap();
    out
}

fn make_fake_mp4(size: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(size);
    data.extend_from_slice(&20u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    data.extend_from_slice(b"isom");
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(b"isom");
    while data.len() < size {
        let chunk = ((data.len() * 131) % 251) as u8;
        data.push(chunk);
    }
    data.truncate(size);
    data
}

fn import_item(db: &mut VaultDb, bytes: &[u8], title: Option<&str>) -> String {
    match db.import_bytes(bytes, title).unwrap() {
        ImportOutcome::Imported { id, .. } => id,
        ImportOutcome::Duplicate => panic!("unexpected duplicate"),
    }
}

fn parse_zip_store(archive: &[u8]) -> HashMap<String, Vec<u8>> {
    // Locate EOCD scanning backwards
    let eocd_pos = (0..=archive.len().saturating_sub(22))
        .rev()
        .find(|&i| &archive[i..i + 4] == b"PK\x05\x06")
        .expect("EOCD signature missing");
    let total_entries = u16::from_le_bytes([archive[eocd_pos + 10], archive[eocd_pos + 11]]);
    let cd_size = u32::from_le_bytes(archive[eocd_pos + 12..eocd_pos + 16].try_into().unwrap());
    let cd_off = u32::from_le_bytes(archive[eocd_pos + 16..eocd_pos + 20].try_into().unwrap());
    assert_eq!(cd_off + cd_size, eocd_pos as u32);

    let mut map = HashMap::new();
    let mut cursor = cd_off as usize;
    for _ in 0..total_entries {
        assert_eq!(&archive[cursor..cursor + 4], b"PK\x01\x02");
        let gpbf = u16::from_le_bytes([archive[cursor + 8], archive[cursor + 9]]);
        assert_eq!(gpbf, ZIP_GPBF_DATA_DESC_UTF8);
        let method = u16::from_le_bytes([archive[cursor + 10], archive[cursor + 11]]);
        assert_eq!(method, 0);
        let cd_crc = u32::from_le_bytes(archive[cursor + 16..cursor + 20].try_into().unwrap());
        let cd_comp = u32::from_le_bytes(archive[cursor + 20..cursor + 24].try_into().unwrap());
        let cd_uncomp =
            u32::from_le_bytes(archive[cursor + 24..cursor + 28].try_into().unwrap());
        assert_eq!(cd_comp, cd_uncomp);
        let name_len =
            u16::from_le_bytes([archive[cursor + 28], archive[cursor + 29]]) as usize;
        let extra_len =
            u16::from_le_bytes([archive[cursor + 30], archive[cursor + 31]]) as usize;
        let comment_len =
            u16::from_le_bytes([archive[cursor + 32], archive[cursor + 33]]) as usize;
        let lfh_off =
            u32::from_le_bytes(archive[cursor + 42..cursor + 46].try_into().unwrap()) as usize;
        let name = std::str::from_utf8(&archive[cursor + 46..cursor + 46 + name_len])
            .unwrap()
            .to_string();
        cursor += 46 + name_len + extra_len + comment_len;

        assert_eq!(&archive[lfh_off..lfh_off + 4], b"PK\x03\x04");
        let lfh_name_len =
            u16::from_le_bytes([archive[lfh_off + 26], archive[lfh_off + 27]]) as usize;
        let lfh_extra_len =
            u16::from_le_bytes([archive[lfh_off + 28], archive[lfh_off + 29]]) as usize;
        let data_start = lfh_off + 30 + lfh_name_len + lfh_extra_len;
        let data_end = data_start + (cd_comp as usize);
        let payload = archive[data_start..data_end].to_vec();
        assert_eq!(crc32_update(0, &payload), cd_crc);

        // Check Data Descriptor
        assert_eq!(&archive[data_end..data_end + 4], b"PK\x07\x08");
        let dd_crc =
            u32::from_le_bytes(archive[data_end + 4..data_end + 8].try_into().unwrap());
        let dd_size =
            u32::from_le_bytes(archive[data_end + 8..data_end + 12].try_into().unwrap());
        assert_eq!(dd_crc, cd_crc);
        assert_eq!(dd_size, cd_comp);

        map.insert(name, payload);
    }
    map
}

#[test]
fn test_filename_sanitization_and_extensions() {
    assert_eq!(mime_to_extension("image/jpeg"), "jpg");
    assert_eq!(mime_to_extension("video/quicktime"), "mov");
    assert_eq!(
        sanitize_name_segment(".. / \\ : * ? \" < > |", "fb"),
        "_ _ _ _ _ _ _ _ _"
    );
    assert_eq!(sanitize_name_segment("...", "fb"), "fb");
    assert_eq!(sanitize_name_segment("..", "fb"), "fb");
    assert_eq!(sanitize_name_segment("con.txt", "fb"), "_con.txt");
    assert_eq!(sanitize_name_segment("LPT1", "fb"), "_LPT1");
    assert_eq!(
        build_item_filename("vacation.jpeg", "12345678-aaaa", "image/jpeg"),
        "vacation.jpg"
    );
    assert_eq!(
        build_item_filename("日本語イラスト", "12345678-aaaa", "image/png"),
        "日本語イラスト.png"
    );
    assert_eq!(
        build_item_filename("   ", "abcdef12-3456", "image/webp"),
        "item_abcdef12.webp"
    );
    assert_eq!(build_group_page_filename(1, 20, "", "image/png"), "001.png");
    assert_eq!(
        build_group_page_filename(12, 1200, "cover.png", "image/png"),
        "0012_cover.png"
    );

    let cd = content_disposition_attachment("漫画 \"1\".zip");
    assert!(cd.starts_with("attachment; filename=\""));
    assert!(cd.contains("filename*=UTF-8''"));
}

#[test]
fn test_export_plain_item_and_chunked_video_and_vault_overwrite_guard() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("export_single.vault");
    let mut db = VaultDb::create(&vault_path, "pass-1234", false).unwrap();

    let png = make_png(20, 16, [255, 80, 40, 255]);
    let png_id = import_item(&mut db, &png, Some("sample_png"));

    let vid_bytes = make_fake_mp4(150_000);
    let vid_path = dir.path().join("chunked_src.mp4");
    fs::write(&vid_path, &vid_bytes).unwrap();
    let vid_id = match db
        .import_media_file_with_chunk_params(
            &vid_path,
            Some("chunked_movie"),
            None,
            5000,
            64 * 1024,
            32 * 1024,
        )
        .unwrap()
    {
        ImportOutcome::Imported { id, .. } => id,
        ImportOutcome::Duplicate => panic!("unexpected dup"),
    };

    // Vault self-overwrite is forbidden
    let self_err = export_plain_item(
        &db,
        &PlainExportItemRequest {
            item_id: png_id.clone(),
            dest_path: vault_path.to_string_lossy().to_string(),
            overwrite: true,
        },
    );
    assert!(matches!(self_err, Err(VaultError::InvalidInput(_))));

    // Single PNG export matches exact bytes
    let png_out = dir.path().join("out/pic.png");
    let rep1 = export_plain_item(
        &db,
        &PlainExportItemRequest {
            item_id: png_id.clone(),
            dest_path: png_out.to_string_lossy().to_string(),
            overwrite: false,
        },
    )
    .unwrap();
    assert_eq!(rep1.file_count, 1);
    assert_eq!(fs::read(&png_out).unwrap(), png);

    // Existing file rejected unless overwrite = true
    let dup_err = export_plain_item(
        &db,
        &PlainExportItemRequest {
            item_id: png_id,
            dest_path: png_out.to_string_lossy().to_string(),
            overwrite: false,
        },
    );
    assert!(matches!(dup_err, Err(VaultError::VaultAlreadyExists(_))));

    // Chunked video export matches SHA-256 bit-for-bit
    let vid_out = dir.path().join("out/movie.mp4");
    let rep2 = export_plain_item(
        &db,
        &PlainExportItemRequest {
            item_id: vid_id,
            dest_path: vid_out.to_string_lossy().to_string(),
            overwrite: false,
        },
    )
    .unwrap();
    assert_eq!(rep2.total_bytes, vid_bytes.len() as u64);
    assert_eq!(
        sha256_hex(&fs::read(&vid_out).unwrap()),
        sha256_hex(&vid_bytes)
    );
}

#[test]
fn test_export_plain_batch_manga_folders_collisions_and_zip_stream() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("export_batch.vault");
    let mut db = VaultDb::create(&vault_path, "pass-5678", false).unwrap();

    let p1 = make_png(12, 12, [10, 20, 30, 255]);
    let p2 = make_png(14, 14, [40, 50, 60, 255]);
    let j1 = make_jpeg(16, 16, [200, 100, 50]);
    let j2 = make_jpeg(18, 18, [220, 120, 70]);
    let trash_png = make_png(10, 10, [1, 2, 3, 255]);

    let p1_id = import_item(&mut db, &p1, Some("扉絵"));
    let p2_id = import_item(&mut db, &p2, Some(""));
    let j1_id = import_item(&mut db, &j1, Some("Photo"));
    let j2_id = import_item(&mut db, &j2, Some("photo"));
    let trash_id = import_item(&mut db, &trash_png, Some("trashed"));

    let group = db
        .create_group_from_items(&[p1_id, p2_id], Some("作品タイトル"), None)
        .unwrap();
    let root_folder = db.create_folder(None, "旅行", "").unwrap();
    let child_folder = db.create_folder(Some(&root_folder.id), "沖縄", "").unwrap();

    db.add_entries_to_folder(
        &root_folder.id,
        &[
            EntryRef {
                kind: EntryKind::Image,
                id: j1_id.clone(),
            },
            EntryRef {
                kind: EntryKind::Image,
                id: trash_id.clone(),
            },
        ],
    )
    .unwrap();
    db.add_entries_to_folder(
        &child_folder.id,
        &[
            EntryRef {
                kind: EntryKind::Image,
                id: j2_id.clone(),
            },
            EntryRef {
                kind: EntryKind::Group,
                id: group.id.clone(),
            },
        ],
    )
    .unwrap();
    db.move_entries_to_trash(&[EntryRef {
        kind: EntryKind::Image,
        id: trash_id,
    }])
    .unwrap();

    // 1. Export folder WITH subfolders -> hierarchy + group + excluded trash
    let out_with_sub = dir.path().join("batch_with_sub");
    fs::create_dir_all(&out_with_sub).unwrap();
    let rep_sub = export_plain_batch(
        &db,
        &PlainExportBatchRequest {
            dest_dir: out_with_sub.to_string_lossy().to_string(),
            entries: vec![],
            folder_ids: vec![root_folder.id.clone()],
            include_subfolders: true,
        },
    )
    .unwrap();
    assert_eq!(rep_sub.file_count, 4);
    assert_eq!(rep_sub.group_count, 1);
    assert_eq!(rep_sub.folder_count, 2);
    assert_eq!(fs::read(out_with_sub.join("旅行/Photo.jpg")).unwrap(), j1);
    assert_eq!(
        fs::read(out_with_sub.join("旅行/沖縄/photo.jpg")).unwrap(),
        j2
    );
    assert_eq!(
        fs::read(out_with_sub.join("旅行/沖縄/作品タイトル/001_扉絵.png")).unwrap(),
        p1
    );
    assert_eq!(
        fs::read(out_with_sub.join("旅行/沖縄/作品タイトル/002.png")).unwrap(),
        p2
    );
    assert!(!out_with_sub.join("旅行/trashed.png").exists());

    // 2. Export folder WITHOUT subfolders -> only Photo.jpg
    let out_no_sub = dir.path().join("batch_no_sub");
    fs::create_dir_all(&out_no_sub).unwrap();
    let rep_nosub = export_plain_batch(
        &db,
        &PlainExportBatchRequest {
            dest_dir: out_no_sub.to_string_lossy().to_string(),
            entries: vec![],
            folder_ids: vec![root_folder.id.clone()],
            include_subfolders: false,
        },
    )
    .unwrap();
    assert_eq!(rep_nosub.file_count, 1);
    assert!(out_no_sub.join("旅行/Photo.jpg").exists());
    assert!(!out_no_sub.join("旅行/沖縄").exists());

    // 3. Collision test when both Photo and photo exported together at root
    let out_collide = dir.path().join("batch_collide");
    fs::create_dir_all(&out_collide).unwrap();
    export_plain_batch(
        &db,
        &PlainExportBatchRequest {
            dest_dir: out_collide.to_string_lossy().to_string(),
            entries: vec![
                EntryRef {
                    kind: EntryKind::Image,
                    id: j1_id.clone(),
                },
                EntryRef {
                    kind: EntryKind::Image,
                    id: j2_id.clone(),
                },
            ],
            folder_ids: vec![],
            include_subfolders: true,
        },
    )
    .unwrap();
    assert_eq!(fs::read(out_collide.join("Photo.jpg")).unwrap(), j1);
    assert_eq!(fs::read(out_collide.join("photo (2).jpg")).unwrap(), j2);

    // 4. ZIP STORE streaming matches computed size & parses back cleanly
    let plan = build_plain_export_plan(&db, &[], &[root_folder.id], true).unwrap();
    let predicted_size = compute_zip_archive_size(&plan);

    let state = Arc::new(Mutex::new(Some(db)));
    let (tx, mut rx) = mpsc::channel(8);
    let plan_clone = plan.clone();
    std::thread::spawn(move || {
        stream_zip_plan_to_channel(state, plan_clone, tx);
    });
    let mut archive = Vec::new();
    while let Some(item) = rx.blocking_recv() {
        archive.extend_from_slice(&item.unwrap());
    }
    assert_eq!(archive.len() as u64, predicted_size);

    let parsed = parse_zip_store(&archive);
    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed.get("旅行/Photo.jpg").unwrap(), &j1);
    assert_eq!(parsed.get("旅行/沖縄/photo.jpg").unwrap(), &j2);
    assert_eq!(
        parsed.get("旅行/沖縄/作品タイトル/001_扉絵.png").unwrap(),
        &p1
    );
    assert_eq!(parsed.get("旅行/沖縄/作品タイトル/002.png").unwrap(), &p2);
}

#[test]
fn test_sanitize_name_segment_utf8_bytes_header_ext_and_folder_dedup() {
    // 1. 120-char CJK clamped cleanly to <= 180 UTF-8 bytes & <= 100 chars
    let long_cjk = "あ".repeat(120);
    let seg = sanitize_name_segment(&long_cjk, "fallback");
    assert!(seg.len() <= 180);
    assert!(seg.chars().count() <= 100);
    assert!(!seg.is_empty());

    // 2. Content-Disposition preserves extension even on 120-char CJK filename
    let cd = content_disposition_attachment(&format!("{long_cjk}.png"));
    assert!(cd.contains(".png\""));
    assert!(cd.ends_with(".png"));

    // 3. Duplicate subtree folder selection (both orders) + empty folder ignored in folder_count
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("folder_dedup.vault");
    let mut db = VaultDb::create(&vault_path, "pass", false).unwrap();

    let p_dir = db.create_folder(None, "親", "").unwrap();
    let c_dir = db.create_folder(Some(&p_dir.id), "子", "").unwrap();
    let empty_dir = db.create_folder(None, "空フォルダ", "").unwrap();

    let img1 = match db
        .import_bytes(&make_png(8, 8, [10, 20, 30, 255]), Some("a"))
        .unwrap()
    {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected img1"),
    };
    let img2 = match db
        .import_bytes(&make_png(9, 8, [20, 30, 40, 255]), Some("b"))
        .unwrap()
    {
        ImportOutcome::Imported { id, .. } => id,
        _ => panic!("expected img2"),
    };
    db.add_entries_to_folder(
        &p_dir.id,
        &[EntryRef {
            kind: EntryKind::Image,
            id: img1,
        }],
    )
    .unwrap();
    db.add_entries_to_folder(
        &c_dir.id,
        &[EntryRef {
            kind: EntryKind::Image,
            id: img2,
        }],
    )
    .unwrap();

    let plan_pc = build_plain_export_plan(
        &db,
        &[],
        &[p_dir.id.clone(), c_dir.id.clone(), empty_dir.id.clone()],
        true,
    )
    .unwrap();
    assert_eq!(plan_pc.files.len(), 2);
    assert_eq!(plan_pc.folder_count, 2);

    let plan_cp = build_plain_export_plan(
        &db,
        &[],
        &[c_dir.id, p_dir.id, empty_dir.id],
        true,
    )
    .unwrap();
    assert_eq!(plan_cp.files.len(), 2);
    assert_eq!(plan_cp.folder_count, 2);
}
