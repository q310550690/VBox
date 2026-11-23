//! `csp_Youtube` 家族集成测试（假 `yt-dlp` 脚本，全离线）。
//!
//! 覆盖：分派/registry 识别、分类（搜索 + ext class）、搜索扁平条目、
//! 播放列表 URL 分页、详情元数据映射、播放直链与请求头、缺失可执行文件错误。
//!
//! 上游差异：`Youtube.java` 自带 InnerTube 客户端与 DASH 生成器、签名格式需要执行
//! 混淆 JS；桌面端改用 `yt-dlp` 子进程桥（决策见计划 §21.3/§23.1），本测试用
//! 假脚本模拟 `-J` 输出。

use std::path::PathBuf;

use serde_json::{json, Value};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::families;
use vbox_core::spider::catvod::registry;

const SCRIPT: &str = r#"const args = process.argv.slice(2).join(' ');
if (args.includes('ytsearch')) {
  console.log(JSON.stringify({entries:[
    {id:'s1',title:'搜索结果一',uploader:'作者甲',thumbnail:'https://i.test/s1.jpg',duration:61.0},
    {id:'s2',title:'搜索结果二'}
  ]}));
} else if (args.includes('--flat-playlist')) {
  console.log(JSON.stringify({entries:[
    {id:'p1',title:'列表一'},{id:'p2',title:'列表二'},{id:'p3',title:'列表三'}
  ]}));
} else if (args.includes('-f')) {
  console.log(JSON.stringify({id:'v1',title:'视频一',formats:[
    {url:'https://cdn.test/a.webm',http_headers:{'User-Agent':'UA-X','Referer':'https://youtube.com/'}},
    {url:'https://cdn.test/b.mp4',http_headers:{'User-Agent':'UA-Y'}}
  ]}));
} else {
  console.log(JSON.stringify({id:'v1',title:'视频一',thumbnail:'https://i.test/v1.jpg',description:'简介',uploader:'作者乙',upload_date:'20250102',duration:125}));
}
"#;

fn fake_ytdlp() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vbox_youtube_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake-ytdlp.mjs");
    std::fs::write(&script, SCRIPT).unwrap();
    script
}

fn config() -> TvConfig {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 2_000;
    cfg
}

fn site_with_runner(runner: &str) -> Site {
    let ext = json!({ "ytdlp": runner }).to_string();
    Site {
        key: "youtube".into(),
        name: "YouTube".into(),
        api: "csp_Youtube".into(),
        kind: 3,
        ext: Some(Value::String(ext)),
        ..Default::default()
    }
}

fn site() -> Site {
    let script = fake_ytdlp();
    site_with_runner(&format!("node {}", script.to_string_lossy()))
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_and_registry_recognise_youtube() {
    assert!(registry::is_native_ready("csp_Youtube"));
    assert!(registry::blocked_reason("csp_Youtube").is_none());
    assert_eq!(families::family_of("csp_Youtube"), Some("youtube"));
    assert_eq!(families::family_of("  csp_youtube  "), Some("youtube"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_include_search_entry() {
    let mut s = site();
    s.ext = Some(Value::String(
        json!({"class":[{"type_id":"PLabc","type_name":"我的列表"}]}).to_string(),
    ));
    let cats = spider::list_categories(&config(), &s).expect("categories");
    let ids: Vec<_> = cats.iter().map(|c| c.type_id.as_str()).collect();
    assert_eq!(ids, vec!["JARSEARCH", "PLabc"], "{cats:?}");
    assert_eq!(cats[0].type_name, "搜索");
    assert_eq!(cats[1].type_name, "我的列表");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_returns_flat_entries() {
    let items = spider::search(&config(), &site(), "测试关键词", false).expect("search");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].id, "s1");
    assert_eq!(items[0].name, "搜索结果一");
    assert_eq!(items[0].cover.as_deref(), Some("https://i.test/s1.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("作者甲"));
    assert_eq!(items[1].remark, None);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_empty_keyword_returns_empty_without_running() {
    let s = site_with_runner("definitely-not-a-real-binary-xyz");
    let items = spider::search(&config(), &s, "   ", false).expect("search");
    assert!(items.is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_playlist_pages_entries() {
    let s = site();
    let page1 = spider::list_videos(&config(), &s, "https://www.youtube.com/playlist?list=PL1", 1)
        .expect("videos");
    assert_eq!(page1.len(), 3, "{page1:?}");
    let page2 = spider::list_videos(&config(), &s, "https://www.youtube.com/playlist?list=PL1", 2)
        .expect("videos");
    assert!(page2.is_empty());
    let search_cat = spider::list_videos(&config(), &s, "音乐", 1).expect("videos");
    assert_eq!(search_cat.len(), 2, "{search_cat:?}");
    let jar_search = spider::list_videos(&config(), &s, "JARSEARCH", 1).expect("videos");
    assert!(jar_search.is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_maps_single_video() {
    let d = spider::get_detail(&config(), &site(), "v1").expect("detail");
    assert_eq!(d.name, "视频一");
    assert_eq!(d.cover.as_deref(), Some("https://i.test/v1.jpg"));
    assert_eq!(d.desc.as_deref(), Some("简介"));
    assert_eq!(d.director.as_deref(), Some("作者乙"));
    assert_eq!(d.year.as_deref(), Some("2025"));
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "YouTube");
    assert_eq!(d.episodes[0].items[0].url, "v1");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_returns_picked_format_with_headers() {
    let v = spider::player_content(&config(), &site(), "", "v1", &[]).expect("player");
    assert_eq!(v.get("parse").and_then(Value::as_i64), Some(0));
    assert_eq!(v.get("playUrl").and_then(Value::as_str), Some(""));
    assert_eq!(
        v.get("url").and_then(Value::as_str),
        Some("https://cdn.test/b.mp4")
    );
    let header = v.get("header").and_then(Value::as_object).expect("header");
    assert_eq!(
        header.get("User-Agent").and_then(Value::as_str),
        Some("UA-Y")
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn missing_runner_reports_host_error() {
    let s = site_with_runner("definitely-not-a-real-binary-xyz");
    let err = spider::get_detail(&config(), &s, "v1").expect_err("should fail");
    assert!(
        err.to_string().contains("catvod.host.unsupported"),
        "{err}"
    );
}
