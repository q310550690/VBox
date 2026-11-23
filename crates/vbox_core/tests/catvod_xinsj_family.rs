//! `csp_Xinsj`（✨新视觉，MacCMS v10）集成验收。
//!
//! 站点 `https://www.6080yy3.com/` 与 ext 里的 `https://www.hdmyy.com/` 2026-11 探测
//! 均 ENOTFOUND，因此用 tiny_http mock 走完整分派链路：首页导航分类、分类列表、
//! 详情（含「夸克」线路跳过）、搜索、播放（`var player_` + `playerconfig.js` 前缀）。
//! 基址经 ext 首个 http(s) 段覆盖（测试钩子偏差，见家族模块头）。

use std::io::Read;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};

const HOME_BODY: &str = r##"<!doctype html><html><body>
<ul class="nav-menu-items">
  <li class="nav-menu-item"><a href="/">首页</a></li>
  <li class="nav-menu-item"><a href="/vodtype/1.html">电影</a></li>
  <li class="nav-menu-item"><a href="/vodtype/2.html">电视剧</a></li>
  <li class="nav-menu-item"><a href="/vodtype/3.html">动漫</a></li>
  <li class="nav-menu-item"><a href="/vodtype/4.html">综艺</a></li>
</ul>
<div class="module-items">
  <div class="module-item">
    <div class="module-item-titlebox"><a href="/video/100.html">首页电影</a></div>
    <div class="module-item-pic"><a href="/video/100.html"><img data-src="/upload/100.jpg"></a></div>
    <div class="module-item-text">更新至01集</div>
  </div>
</div>
</body></html>"##;

const LIST_BODY: &str = r##"<!doctype html><html><body>
<div class="module-items">
  <div class="module-item-cover">
    <div class="module-item-pic"><a href="/video/200.html">分类电影</a><img data-src="/upload/200.jpg"></div>
    <div class="module-item-text">HD</div>
  </div>
</div>
<div id="page">
  <a href="/vodshow/1----------1---.html">1</a>
  <a href="/vodshow/1----------2---.html">2</a>
</div>
</body></html>"##;

const DETAIL_BODY: &str = r##"<!doctype html><html><body>
<div class="module-item-pic"><img data-src="/upload/300.jpg"></div>
<div class="video-info-header"><h1 class="page-title">详情剧名</h1></div>
<div class="video-info-main">
  <span class="video-info-itemtitle">分类：国产剧</span>
  <span class="video-info-itemtitle">上映：2025</span>
  <span class="video-info-itemtitle">地区：大陆</span>
  <span class="video-info-itemtitle">更新：更新至12集</span>
  <span class="video-info-itemtitle">主演：演员甲</span>
  <span class="video-info-itemtitle">导演：导演乙</span>
</div>
<div class="video-info-content"><span>详情简介文本</span></div>
<div class="module-tab-content"><div><span>线路一</span></div><div><span>夸克</span></div></div>
<div class="module-play-list"><a href="/vplay/300-1-1.html">第01集</a><a href="/vplay/300-1-2.html">第02集</a></div>
<div class="module-play-list"><a href="/vplay/300-2-1.html">夸克1</a></div>
</body></html>"##;

const SEARCH_BODY: &str = r##"<!doctype html><html><body>
<div class="module-items">
  <div class="module-search-item">
    <div class="module-item-pic"><a href="/video/400.html"><img class="lazyload" data-src="/upload/400.jpg" alt="搜索剧名"></a></div>
    <div class="video-info-header"><a href="/video/400.html">HD</a></div>
  </div>
</div>
</body></html>"##;

const PLAYER_BODY: &str = r##"<!doctype html><html><body>
<script>var player_aaaa={"flag":"lzm3u8","encrypt":0,"url":"https://cdn.example.com/ep1/index.m3u8","from":"dplayer"};</script>
</body></html>"##;

const PLAYER_ENCRYPT_BODY: &str = r##"<!doctype html><html><body>
<script>var player_aaaa={"flag":"lzm3u8","encrypt":1,"url":"aabbccdd","from":"dplayer"};</script>
</body></html>"##;

const PLAYER_CFG_JS: &str = r##"var MacPlayerConfig = {};MacPlayerConfig.server_list={"dplayer":{"parse":"https://jx.example.com/?url=","server":"","path":""}};MacPlayerConfig.player_list={};"##;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "xinsj-mock".into(),
        name: "新视觉".into(),
        kind: 3,
        api: "csp_Xinsj".into(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

fn respond_text(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("header");
    let response = Response::from_string(body.to_string()).with_header(header);
    let _ = request.respond(response);
}

fn start_server() -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("mock server"));
    let addr = format!("http://{}", server.server_addr());
    let server_for_thread = Arc::clone(&server);
    thread::spawn(move || {
        for mut request in server_for_thread.incoming_requests() {
            let url = request.url().to_string();
            let mut body = String::new();
            let _ = request.as_reader().take(64 * 1024).read_to_string(&mut body);
            if url.contains("/static/js/playerconfig.js") {
                respond_text(request, PLAYER_CFG_JS);
            } else if url.contains("/vplay/999-") {
                respond_text(request, PLAYER_ENCRYPT_BODY);
            } else if url.contains("/vplay/") {
                respond_text(request, PLAYER_BODY);
            } else if url.contains("/video/") {
                respond_text(request, DETAIL_BODY);
            } else if url.contains("/vod-s/") {
                respond_text(request, SEARCH_BODY);
            } else if url.contains("/vodshow/") {
                respond_text(request, LIST_BODY);
            } else {
                respond_text(request, HOME_BODY);
            }
        }
    });
    addr
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_routes_xinsj_to_native_family() {
    let site = site("https://mock.invalid");
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&site.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&site.api).is_none());
    assert_eq!(
        vbox_core::spider::catvod::families::family_of(&site.api),
        Some("xinsj")
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_keep_four_mac_cms_entries() {
    let base = start_server();
    let cats = list_categories(&cfg(), &site(&base)).expect("分类");
    let ids: Vec<&str> = cats.iter().map(|c| c.type_id.as_str()).collect();
    assert_eq!(ids, vec!["1", "2", "3", "4"]);
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(names, vec!["电影", "电视剧", "动漫", "综艺"]);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn category_list_through_dispatch() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "1", 2).expect("分类页");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "200");
    assert_eq!(items[0].name, "分类电影");
    assert_eq!(items[0].cover.as_deref(), Some(&format!("{base}/upload/200.jpg")[..]));
    assert_eq!(items[0].remark.as_deref(), Some("HD"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_through_dispatch_skips_quark_line() {
    let base = start_server();
    let detail = get_detail(&cfg(), &site(&base), "300").expect("详情");
    assert_eq!(detail.name, "详情剧名");
    assert_eq!(detail.year.as_deref(), Some("2025"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.actor.as_deref(), Some("演员甲"));
    assert_eq!(detail.director.as_deref(), Some("导演乙"));
    let desc = detail.desc.clone().unwrap_or_default();
    assert!(desc.contains("分类：国产剧"), "desc={desc}");
    assert!(desc.contains("更新：更新至12集"), "desc={desc}");
    assert_eq!(detail.episodes.len(), 1, "「夸克」线路应被跳过");
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].url, "300-1-2");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_through_dispatch() {
    let base = start_server();
    let items = search(&cfg(), &site(&base), "搜索词", false).expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "400");
    assert_eq!(items[0].name, "搜索剧名");
    assert_eq!(items[0].remark.as_deref(), Some("HD"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_through_dispatch_returns_direct_url() {
    let base = start_server();
    let value = player_content(&cfg(), &site(&base), "线路一", "300-1-1", &[]).expect("播放");
    assert_eq!(value["parse"], json!(0));
    assert_eq!(value["playUrl"], json!("300-1-1"));
    assert_eq!(value["url"], json!("https://cdn.example.com/ep1/index.m3u8"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_encrypted_payload_is_explicit_parse_error() {
    let base = start_server();
    let err = player_content(&cfg(), &site(&base), "线路一", "999-1-1", &[]).expect_err("加密串");
    let text = format!("{err}");
    assert!(text.contains("加密播放串"), "unexpected: {text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn category_url_keeps_double_slash_shape() {
    let base = start_server();
    // 家族内构造的 URL 与原类一致是 `…//vodshow/…`，这里用 URL 形态断言防止回归。
    assert_eq!(
        vbox_core::spider::catvod::families::xinsj::list_url("https://www.6080yy3.com/", "1", 2),
        "https://www.6080yy3.com//vodshow/1----------2---.html"
    );
    let _ = base;
}
