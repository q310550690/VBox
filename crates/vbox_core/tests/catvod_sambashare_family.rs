//! `csp_SambaShare` 家族集成测试（本机目录模拟 SMB 挂载，全离线）。
//!
//! 覆盖：分派/registry 识别、清单解析（单层 + 分组）、分享目录浏览、子目录浏览、
//! 目录/单文件详情展开、`file://` 播放地址、本地关键词搜索、缺少清单源的解析错误。
//!
//! 上游差异：Android 侧 `SambaShare` 经 `aliproxy` 伴生服务访问 SMB；桌面端改为
//! ext JSON 的 `smb_root` 指向本机已挂载目录（第一层子目录对应 share 名），
//! 用 `std::fs` 列目录。本测试用临时目录充当挂载根。

use std::path::PathBuf;

use serde_json::json;

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::families;
use vbox_core::spider::catvod::registry;

const SHARE: &str = "host/share1";

fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vbox_sambashare_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("share1/sub")).unwrap();
    std::fs::write(dir.join("share1/电影.mkv"), vec![0u8; 64]).unwrap();
    std::fs::write(dir.join("share1/note.txt"), b"notes").unwrap();
    std::fs::write(dir.join("share1/sub/ep1.mp4"), vec![0u8; 128]).unwrap();
    std::fs::write(dir.join("share1/sub/ep2.MP4"), vec![0u8; 256]).unwrap();
    dir
}

fn config() -> TvConfig {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 2_000;
    cfg
}

fn site(root: &PathBuf) -> Site {
    let ext = format!(
        "{{\"smb_root\":{:?},\"list_source\":\"host/share1 剧集一\\nhost/share1/sub 分组/子目录\"}}",
        root.to_string_lossy()
    );
    Site {
        key: "sambashare".into(),
        name: "SMB分享".into(),
        api: "csp_SambaShare".into(),
        kind: 3,
        ext: Some(json!(ext)),
        ..Default::default()
    }
}

#[test]
fn dispatch_and_registry_recognise_sambashare() {
    let root = temp_root();
    let s = site(&root);
    assert!(registry::is_native_ready("csp_SambaShare"));
    assert!(registry::blocked_reason("csp_SambaShare").is_none());
    assert_eq!(families::family_of("csp_SambaShare"), Some("sambashare"));
    assert_eq!(families::family_of("  csp_sambashare  "), Some("sambashare"));
    let cats = spider::list_categories(&config(), &s).expect("categories");
    assert_eq!(cats.len(), 2, "{cats:?}");
    assert_eq!(cats[0].type_id, SHARE);
    assert_eq!(cats[0].type_name, "剧集一");
    assert_eq!(cats[1].type_id, "分组");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn videos_flat_lists_share_directory() {
    let root = temp_root();
    let items = spider::list_videos(&config(), &site(&root), SHARE, 1).expect("videos");
    let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["sub", "电影.mkv"], "{items:?}");
    assert_eq!(items[0].id, format!("{SHARE}###sub"));
    assert_eq!(items[0].remark.as_deref(), Some("文件夹"));
    assert_eq!(items[1].id, format!("{SHARE}###电影.mkv"));
    assert_eq!(items[1].remark, None);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn videos_group_returns_manifest_entries() {
    let root = temp_root();
    let items = spider::list_videos(&config(), &site(&root), "分组", 1).expect("videos");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].name, "子目录");
    assert_eq!(items[0].id, format!("{SHARE}/sub"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn videos_subdirectory_browses_files() {
    let root = temp_root();
    let items = spider::list_videos(&config(), &site(&root), &format!("{SHARE}###sub"), 1).expect("videos");
    let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["ep1.mp4", "ep2.MP4"], "{items:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn detail_directory_expands_episodes() {
    let root = temp_root();
    let detail = spider::get_detail(&config(), &site(&root), &format!("{SHARE}/sub")).expect("detail");
    assert_eq!(detail.name, "sub");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "SMB分享");
    let titles: Vec<_> = detail.episodes[0].items.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, vec!["ep1", "ep2"]);
    assert_eq!(detail.episodes[0].items[0].url, format!("{SHARE}###sub/ep1.mp4"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn detail_single_file_makes_one_episode() {
    let root = temp_root();
    let detail = spider::get_detail(&config(), &site(&root), &format!("{SHARE}###电影.mkv")).expect("detail");
    assert_eq!(detail.name, "电影");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(detail.episodes[0].items[0].title, "电影");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn player_returns_file_url() {
    let root = temp_root();
    let value = spider::player_content(&config(), &site(&root), "", &format!("{SHARE}###电影.mkv"), &[])
        .expect("player");
    let url = value.get("url").and_then(|v| v.as_str()).expect("url");
    assert!(url.starts_with("file:///"), "{url}");
    assert!(url.contains("%E7%94%B5%E5%BD%B1.mkv"), "{url}");
    assert_eq!(value.get("parse").and_then(|v| v.as_i64()), Some(0));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn player_missing_file_reports_parse_error() {
    let root = temp_root();
    let err = spider::player_content(&config(), &site(&root), "", &format!("{SHARE}###nope.mp4"), &[])
        .expect_err("缺文件");
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn search_matches_manifest_names() {
    let root = temp_root();
    let items = spider::search(&config(), &site(&root), "子目录", false).expect("search");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].id, format!("{SHARE}/sub"));
    let none = spider::search(&config(), &site(&root), "不存在", false).expect("search");
    assert!(none.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn missing_list_source_reports_parse_error() {
    let root = temp_root();
    let ext = format!("{{\"smb_root\":{:?}}}", root.to_string_lossy());
    let s = Site {
        key: "sambashare".into(),
        name: "SMB分享".into(),
        api: "csp_SambaShare".into(),
        kind: 3,
        ext: Some(json!(ext)),
        ..Default::default()
    };
    let err = spider::list_categories(&config(), &s).expect_err("无清单源");
    assert!(err.to_string().contains("清单源"), "{err}");
    std::fs::remove_dir_all(&root).ok();
}
