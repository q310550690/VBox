//! `csp_Yinyuetai` 家族集成测试（tiny_http mock + 真站 `#[ignore]`）。
//!
//! 覆盖：固定 4 分类、分类列表（channelId + offset 分页）、歌手锚点分类、
//! MV 详情（按 `display` 分线路、`#` 拆行、artist 合并）、歌手详情（listByArtist）、
//! 搜索（POST JSON）、播放（直链 + UA）、非法 JSON 报错。
//!
//! 真站用例：上游 `*.yinyuetai.com` 证书 2026-10-30 到期，家族对本站关闭证书校验
//! （见 `yinyuetai.rs` 模块头），故可用 `#[ignore]` 真站测试验证协议仍然成立。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const LIST_JSON: &str = r#"{"code":0,"msg":"success","data":[
{"id":"7075422331362983936","title":"催眠大师 Live","headImg":"https://img/x.jpg","content":"吴青峰 A 翻全场"},
{"id":"7075422338778509312","title":"第二首","headImg":"","content":""}]}"#;

const ARTIST_JSON: &str = r#"{"code":0,"msg":"success","data":[
{"id":"7101497843034206208","title":"你在烦恼什么 Live","headImg":"https://img/a.jpg","fullClip":{"urls":[{"display":"完整版","url":"https://cdn/a.mp4"}]}},
{"id":"7083655576550490112","title":"年度专辑 Live","headImg":"https://img/b.jpg","fullClip":{"urls":[{"display":"完整版","url":"https://cdn/b.mp4"}]}}]}"#;

const DETAIL_JSON: &str = r#"{"code":0,"msg":"success","data":{
"id":"7075422331362983936","title":"催眠大师 Live","headImg":"https://img/x.jpg",
"content":"吴青峰 A 翻全场",
"artists":[{"id":"1","name":"吴青峰"},{"id":"2","name":"五月天"}],
"fullClip":{"urls":[
 {"display":"完整版2500码流","url":"https://cdn/a-hd.mp4#https://cdn/a-2.mp4"},
 {"display":"MP3播放地址","url":"https://cdn/a.mp3"}]}}}"#;

const SEARCH_JSON: &str = r#"{"code":0,"msg":"success","data":[
{"id":"7075422331362983936","title":"催眠大师 Live","headImg":"https://img/x.jpg","content":"吴青峰"}]}"#;

fn respond_json(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
        .expect("header");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_handle = Arc::clone(&seen);
    let handle = Arc::clone(&server);
    thread::spawn(move || loop {
        let Ok(request) = handle.recv() else { return };
        let url = request.url().to_string();
        seen_handle.lock().unwrap().push(url.clone());
        if url.contains("id=bad") {
            respond_json(request, "不是 JSON");
        } else if url.contains("channelVideos") {
            respond_json(request, LIST_JSON);
        } else if url.contains("listByArtist") {
            respond_json(request, ARTIST_JSON);
        } else if url.contains("video/get") {
            respond_json(request, DETAIL_JSON);
        } else if url.contains("get_search_result") {
            respond_json(request, SEARCH_JSON);
        } else {
            respond_json(request, "{}");
        }
    });
    (base, seen)
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "yinyuetai".into(),
        name: "音悦台".into(),
        api: "csp_Yinyuetai".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
fn categories_are_fixed_four() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    assert_eq!(cats.len(), 4);
    assert_eq!(cats[0].type_name, "华语");
    assert_eq!(cats[2].type_name, "韩语");
}

#[test]
fn list_channel_videos_uses_offset_paging() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "6998499728862334976", 2)
        .expect("list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "7075422331362983936");
    assert_eq!(items[0].name, "催眠大师 Live");
    assert_eq!(items[0].cover.as_deref(), Some("https://img/x.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("吴青峰 A 翻全场"));
    assert_eq!(items[1].cover, None);
    let urls = seen.lock().unwrap();
    assert!(urls.iter().any(|u| u.contains("channelId=6998499728862334976") && u.contains("offset=20")));
}

#[test]
fn artist_anchor_category_returns_single_item() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cat = "6926073053449290079$$$吴青峰$$$https://img/a.jpg$$${pg}";
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), cat, 1).expect("list");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "6926073053449290079$$$吴青峰$$$https://img/a.jpg$$$act");
    assert_eq!(items[0].name, "吴青峰");
    assert_eq!(items[0].remark.as_deref(), Some("歌手"));
}

#[test]
fn mv_detail_groups_episodes_by_display() {
    let cfg = cfg();
    let (base, _) = start_server();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "7075422331362983936")
        .expect("detail");
    assert_eq!(detail.name, "催眠大师 Live");
    assert_eq!(detail.cover.as_deref(), Some("https://img/x.jpg"));
    assert_eq!(detail.actor.as_deref(), Some("吴青峰、五月天"));
    assert!(detail.desc.as_deref().unwrap().ends_with("吴青峰 A 翻全场"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "完整版2500码流");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].url, "https://cdn/a-2.mp4");
    assert_eq!(detail.episodes[1].items[0].url, "https://cdn/a.mp3");
}

#[test]
fn artist_detail_uses_list_by_artist() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let id = "6926073053449290079$$$吴青峰$$$https://img/a.jpg$$$act";
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), id).expect("detail");
    assert_eq!(detail.name, "吴青峰");
    assert_eq!(detail.cover.as_deref(), Some("https://img/a.jpg"));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[0].title, "你在烦恼什么 Live");
    assert!(detail.episodes[0].group.starts_with("TG:https://t.me/TVBoxxoo"));
    let urls = seen.lock().unwrap();
    assert!(urls.iter().any(|u| u.contains("listByArtist") && u.contains("artistId=6926073053449290079")));
}

#[test]
fn search_posts_json_query() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "催眠", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "催眠大师 Live");
    let urls = seen.lock().unwrap();
    assert!(urls.iter().any(|u| u.contains("get_search_result")));
}

#[test]
fn player_returns_direct_url_with_ua() {
    let cfg = cfg();
    let (base, _) = start_server();
    let out = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "线路",
        "https://cloud-cdn.yinyuetai.com/x-hd.mp4",
        &[],
    )
    .expect("player");
    assert_eq!(out["url"], "https://cloud-cdn.yinyuetai.com/x-hd.mp4");
    assert_eq!(out["parse"], 0);
    assert!(out["header"]["User-Agent"].as_str().unwrap().contains("Chrome/128"));
}

#[test]
fn invalid_json_surfaces_error() {
    let cfg = cfg();
    let (base, _) = start_server();
    // 非数字 id 且不以 $$$act 结尾 → 走 video/get → mock 返回非法 JSON
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "bad").expect_err("err");
    let text = err.to_string();
    assert!(text.contains("JSON") || text.contains("解析"), "unexpected: {text}");
}

#[test]
#[ignore = "真站：上游证书 2026-10-30 过期（家族对该站关闭校验），需手动/真站验证时运行"]
fn live_channel_videos() {
    let cfg = cfg();
    let mut s = Site {
        key: "yinyuetai".into(),
        name: "音悦台".into(),
        api: "csp_Yinyuetai".into(),
        ..Default::default()
    };
    s.ext = None;
    let items = vbox_core::sources::spider::list_videos(&cfg, &s, "6998499728862334976", 1)
        .expect("live list");
    assert!(!items.is_empty());
    let detail = vbox_core::sources::spider::get_detail(&cfg, &s, &items[0].id).expect("live detail");
    assert!(!detail.episodes.is_empty());
}
