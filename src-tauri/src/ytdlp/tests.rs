use super::download::{collect_multi_staged_files, extract_video_metadata_list};
use super::process::{format_ytdlp_failure, parse_progress_line};
use super::*;
use serde_json::{json, Value};
use tempfile::TempDir;

#[test]
fn test_validate_http_url() {
    assert_eq!(
        validate_http_url("  https://x.com/user/status/12345?s=20  ").unwrap(),
        "https://x.com/user/status/12345?s=20"
    );
    assert_eq!(
        validate_http_url("https://www.tiktok.com/@creator/video/987654").unwrap(),
        "https://www.tiktok.com/@creator/video/987654"
    );
    assert!(validate_http_url("").is_err());
    assert!(validate_http_url("file:///etc/passwd").is_err());
    assert!(validate_http_url("javascript:alert(1)").is_err());
    assert!(validate_http_url("-o /tmp/bad").is_err());
    assert!(validate_http_url("https://x.com/a b").is_err());
    assert!(validate_http_url("https://x.com/a\nb").is_err());
    assert!(validate_http_url("https://").is_err());
    let long_url = format!("https://example.com/{}", "a".repeat(2050));
    assert!(validate_http_url(&long_url).is_err());
}

#[test]
fn test_cookie_browser_allowlist_and_twitter_url_normalize() {
    assert!(is_valid_cookie_browser(""));
    assert!(is_valid_cookie_browser("arc"));
    assert!(is_valid_cookie_browser("Chrome"));
    assert!(is_valid_cookie_browser("safari"));
    assert!(is_valid_cookie_browser("file"));
    assert!(!is_valid_cookie_browser("chrome --exec=rm"));
    assert!(!is_valid_cookie_browser("unknown-browser"));

    assert_eq!(
        normalize_twitter_url("https://x.com/someone/status/123456789/photo/1?s=20"),
        "https://x.com/someone/status/123456789?s=20"
    );
    assert_eq!(
        normalize_twitter_url("https://twitter.com/someone/status/123456789/photo/2"),
        "https://twitter.com/someone/status/123456789"
    );
    assert_eq!(
        normalize_twitter_url("https://x.com/someone/status/123456789/video/2?t=abc"),
        "https://x.com/someone/status/123456789?t=abc"
    );
    assert_eq!(
        normalize_twitter_url("https://www.tiktok.com/@u/video/111/photo/1"),
        "https://www.tiktok.com/@u/video/111/photo/1"
    );
}

#[test]
fn test_extract_metadata_playlist_and_needs_ffmpeg() {
    let progressive = json!({
        "title": "  Twitter GIF  ",
        "webpage_url": "https://x.com/u/status/1",
        "duration": 4.2,
        "filesize": 1_250_000u64,
        "protocol": "https"
    });
    let list1 = extract_video_metadata_list(&progressive, "https://x.com/u/status/1");
    assert_eq!(list1.len(), 1);
    assert_eq!(list1[0].title, "Twitter GIF");
    assert_eq!(list1[0].duration_ms, 4200);
    assert_eq!(list1[0].filesize_estimate, Some(1_250_000));
    assert!(!list1[0].needs_ffmpeg);
    assert_eq!(list1[0].playlist_index, 1);

    let multi = json!({
        "_type": "playlist",
        "title": "親タイトル",
        "webpage_url": "https://x.com/u/status/2",
        "entries": [
            {
                "title": "投稿 #1",
                "duration": 3.5,
                "filesize": 900_000u64,
                "protocol": "https",
                "playlist_index": 1
            },
            {
                "title": "投稿 #2",
                "duration": 185.5,
                "playlist_index": 2,
                "requested_formats": [
                    { "protocol": "m3u8_native", "filesize_approx": 150_000_000u64 },
                    { "protocol": "https", "filesize": 12_000_000u64 }
                ]
            }
        ]
    });
    let list2 = extract_video_metadata_list(&multi, "fallback");
    assert_eq!(list2.len(), 2);
    assert_eq!(list2[0].title, "投稿 #1");
    assert_eq!(list2[0].source_url, "https://x.com/u/status/2");
    assert!(!list2[0].needs_ffmpeg);
    assert_eq!(list2[1].title, "投稿 #2");
    assert_eq!(list2[1].duration_ms, 185_500);
    assert_eq!(list2[1].filesize_estimate, Some(162_000_000));
    assert!(list2[1].needs_ffmpeg);

    // Verify MAX_MULTI_MEDIA_ITEMS (20) cap
    let entries_25: Vec<Value> = (1..=25)
        .map(|i| {
            json!({
                "title": format!("Item #{i}"),
                "playlist_index": i,
                "duration": 5.0,
                "protocol": "https"
            })
        })
        .collect();
    let big_playlist = json!({
        "_type": "playlist",
        "webpage_url": "https://x.com/u/status/3",
        "entries": entries_25
    });
    let capped = extract_video_metadata_list(&big_playlist, "fallback");
    assert_eq!(capped.len(), 20);
    assert_eq!(capped[19].playlist_index, 20);
    assert_eq!(capped[19].title, "Item #20");
}

#[test]
fn test_collect_multi_staged_files_matching_and_filtering() {
    let dir = TempDir::new().unwrap();
    let mut valid_mp4 = Vec::new();
    valid_mp4.extend_from_slice(&20u32.to_be_bytes());
    valid_mp4.extend_from_slice(b"ftyp");
    valid_mp4.extend_from_slice(b"isom");
    valid_mp4.extend_from_slice(&0u32.to_be_bytes());
    valid_mp4.extend_from_slice(b"mp42");
    valid_mp4.extend_from_slice(&vec![0xAAu8; 400]);

    fs::write(dir.path().join("001_video.mp4"), &valid_mp4).unwrap();
    fs::write(dir.path().join("002_video.mp4.part"), &valid_mp4).unwrap();
    fs::write(dir.path().join("003_video.mp4"), &valid_mp4).unwrap();

    let mut oversize_mp4 = valid_mp4.clone();
    oversize_mp4.extend_from_slice(&vec![0xBBu8; 2000]);
    fs::write(dir.path().join("004_video.mp4"), &oversize_mp4).unwrap();
    fs::write(dir.path().join("005_video.mp4"), b"NOT_A_VIDEO").unwrap();

    let metas = vec![
        ExtractedVideoMeta {
            title: "GIF 1".to_string(),
            source_url: "https://x.com/u/status/10".to_string(),
            duration_ms: 3000,
            filesize_estimate: None,
            needs_ffmpeg: false,
            playlist_index: 1,
        },
        ExtractedVideoMeta {
            title: "Clip 2".to_string(),
            source_url: "https://x.com/u/status/10".to_string(),
            duration_ms: 5000,
            filesize_estimate: None,
            needs_ffmpeg: false,
            playlist_index: 2,
        },
        ExtractedVideoMeta {
            title: "GIF 3".to_string(),
            source_url: "https://x.com/u/status/10".to_string(),
            duration_ms: 4000,
            filesize_estimate: None,
            needs_ffmpeg: false,
            playlist_index: 3,
        },
        ExtractedVideoMeta {
            title: "Big 4".to_string(),
            source_url: "https://x.com/u/status/10".to_string(),
            duration_ms: 9000,
            filesize_estimate: None,
            needs_ffmpeg: false,
            playlist_index: 4,
        },
        ExtractedVideoMeta {
            title: "Bad 5".to_string(),
            source_url: "https://x.com/u/status/10".to_string(),
            duration_ms: 1000,
            filesize_estimate: None,
            needs_ffmpeg: false,
            playlist_index: 5,
        },
    ];

    let (items, warnings) = collect_multi_staged_files(dir.path(), &metas, Some(1000), 1);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].title, "GIF 1");
    assert_eq!(items[1].title, "GIF 3");
    assert_eq!(items[1].duration_ms, 4000);
    assert_eq!(warnings.len(), 2);
    assert!(warnings.iter().any(|w| w.contains("Big 4")));
    assert!(warnings.iter().any(|w| w.contains("Bad 5")));
}

#[test]
fn test_parse_progress_and_format_errors() {
    let ev = parse_progress_line(
        "TUCK_PROG| 42.5%| 5.12MiB/s|00:18|44564480|104857600|NA|2|4",
        None,
    )
    .unwrap();
    assert_eq!(ev.stage, "downloading");
    assert_eq!(ev.percent, Some(42.5));
    assert_eq!(ev.speed, "5.12MiB/s");
    assert_eq!(ev.eta, "00:18");
    assert_eq!(ev.downloaded_bytes, 44_564_480);
    assert_eq!(ev.total_bytes, Some(104_857_600));
    assert_eq!(ev.item_index, Some(2));
    assert_eq!(ev.item_total, Some(4));

    let ev_hint = parse_progress_line(
        "TUCK_PROG| 10.0%| 1.0MiB/s|00:05|1024|10240|NA|3|NA",
        Some(4),
    )
    .unwrap();
    assert_eq!(ev_hint.item_index, Some(3));
    assert_eq!(ev_hint.item_total, Some(4));

    let pp = parse_progress_line("TUCK_PP|started|Merger", Some(4)).unwrap();
    assert_eq!(pp.stage, "muxing");
    assert_eq!(pp.item_total, Some(4));
    assert!(pp.message.contains("結合中"));

    let no_cookie_msg = format_ytdlp_failure(
        "meta",
        b"ERROR: [twitter] 123: No video could be found in this tweet",
        false,
        false,
    );
    assert!(!no_cookie_msg.contains("入力値が不正です"));
    assert!(no_cookie_msg.contains("Cookie 連携"));

    let keychain_msg = format_ytdlp_failure(
        "meta",
        b"WARNING: cannot decrypt v10 cookies: no key found",
        true,
        true,
    );
    assert!(keychain_msg.contains("cookies.txt"));

    let ffmpeg_msg = format_ytdlp_failure(
        "download",
        b"ERROR: You have requested merging of multiple formats but ffmpeg is not installed",
        false,
        false,
    );
    assert!(ffmpeg_msg.contains("brew install ffmpeg"));
    assert!(ffmpeg_msg.contains("winget install"));
    assert!(ffmpeg_msg.contains("apt install ffmpeg"));
}
