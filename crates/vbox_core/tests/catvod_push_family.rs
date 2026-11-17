//! `csp_Push`（推送型蜘蛛）集成验收。
//!
//! 原类 `Push`（jadx 54 行、未混淆）没有 home/category/search：只能把外部推来的
//! 播放地址（官方视频站 / 直链 / 网页 / 阿里云盘分享）映射成一条线路。本文件只覆盖
//! 不触网的路径（官方站、直链、网页、错误分支），阿里云盘展开需要真实上游，放单测
//! 里按纯函数验证分类逻辑。

use serde_json::json;

use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::registry;
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site() -> Site {
    Site {
        key: "push".to_string(),
        name: "推送".to_string(),
        api: "csp_Push".to_string(),
        kind: 3,
        ..Default::default()
    }
}

#[test]
fn dispatch_routes_push_to_native_family() {
    assert!(registry::is_native_ready("csp_Push"));
    assert!(registry::blocked_reason("csp_Push").is_none());
    assert_eq!(
        vbox_core::spider::catvod::families::family_of("csp_Push"),
        Some("push")
    );
}

#[test]
fn categories_are_empty_through_dispatch() {
    let cats = list_categories(&cfg(), &site()).expect("categories");
    assert!(cats.is_empty());
}

#[test]
fn official_detail_keeps_official_line() {
    let detail = get_detail(&cfg(), &site(), "https://www.iqiyi.com/v_19r.html").expect("detail");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "官源");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(detail.episodes[0].items[0].title, "播放");
    assert!(detail.episodes[0].items[0].url.contains("iqiyi.com"));
}

#[test]
fn direct_media_detail_uses_direct_line() {
    let detail = get_detail(&cfg(), &site(), "https://cdn.example.org/a.m3u8").expect("detail");
    assert_eq!(detail.episodes[0].group, "直连");
}

#[test]
fn unknown_url_detail_falls_back_to_web_line() {
    let detail = get_detail(&cfg(), &site(), "https://example.org/watch/1.html").expect("detail");
    assert_eq!(detail.episodes[0].group, "网页");
}

#[test]
fn blank_id_is_a_parse_error() {
    let err = get_detail(&cfg(), &site(), "   ").expect_err("blank");
    let text = format!("{err}");
    assert!(text.contains("推送地址为空"), "unexpected: {text}");
}

#[test]
fn player_official_requires_parse_one() {
    let value = player_content(
        &cfg(),
        &site(),
        "官源",
        "https://www.iqiyi.com/v_19r.html",
        &[],
    )
    .expect("player");
    assert_eq!(value["parse"], json!(1));
}

#[test]
fn player_direct_passes_url_through() {
    let value = player_content(&cfg(), &site(), "直连", "https://cdn.example.org/a.m3u8", &[]).expect("player");
    assert_eq!(value["parse"], json!(0));
    assert_eq!(value["url"], json!("https://cdn.example.org/a.m3u8"));
}

#[test]
fn videos_are_not_supported() {
    let err = list_videos(&cfg(), &site(), "1", 1).expect_err("no category");
    let text = format!("{err}");
    assert!(text.contains("推送源没有分类浏览") || text.contains("catvod.result.parse"), "unexpected: {text}");
}

#[test]
fn search_is_empty_by_design() {
    let hits = search(&cfg(), &site(), "任意关键词", false).expect("search");
    assert!(hits.is_empty());
}
