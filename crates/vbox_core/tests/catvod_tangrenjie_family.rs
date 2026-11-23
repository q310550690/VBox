//! `csp_Tangrenjie` 家族集成测试（tiny_http mock + 真站 `#[ignore]`）。
//!
//! 覆盖：首页导航分类、列表第 1 页 `/type/{id}.html`、第 2 页
//! `/show/{id}--------2---.html`、详情（元数据 + 两条播放线路）、
//! 播放页 `player_aaaa` 直链、搜索页、以及缺标题/缺 player JSON 的错误映射。
//!
//! 真站用例默认 `#[ignore]`（现役 `https://www.tangrenjie.net`，需要外网）。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const HOME_HTML: &str = r#"<html><body>
<ul id="nav" class="ui-nav">
  <li class="nav-item current"><a class="nav-link" href="/">网站首页</a></li>
  <li class="nav-item 20=0"><a class="nav-link" href="/type/20.html">电影</a></li>
  <li class="nav-item 35=0"><a class="nav-link" href="/type/35.html">动漫</a></li>
  <li class="nav-item "><a class="nav-link" href="/topic.html">专题</a></li>
  <li class="nav-item mac_user"><a class="nav-link" href="javascript:;">会员</a></li>
</ul></body></html>"#;

const LIST_HTML: &str = r#"<html><body><ul class="img-list">
<li><a href="/vod/SCCCCKw8.html" title="梁祝传说"><img src="https://img.tangrenjie.net/a.jpg" alt="梁祝传说"/><h2>梁祝传说</h2><p>祝英台,梁山伯</p><i>正片</i></a></li>
<li><a href="/vod/SCCCCKwA.html"><img data-original="https://img.tangrenjie.net/b.jpg"/><h2>寄生狂潮</h2><i>更新至10集</i></a></li>
</ul></body></html>"#;

const LIST2_HTML: &str = r#"<html><body><ul class="img-list">
<li><a href="/vod/SCCCCKrx.html" title="第二页影片"><img src="https://img.tangrenjie.net/c.jpg"/></a></li>
</ul></body></html>"#;

const DETAIL_HTML: &str = r#"<html><body>
<div class="detail-cols fn-clear">
  <div class="detail-pic fn-left"><img src="https://img.tangrenjie.net/p.jpg" alt="海报" /></div>
  <div class="detail-info fn-left">
    <div class="detail-title fn-clear"><h1>吸血新一代</h1></div>
    <div class="info">
      <dl><dt>主演：</dt><dd>斯派斯,凯利</dd></dl>
      <dl><dt>导演：</dt><dd>弗兰克</dd></dl>
      <dl><dt>语言：</dt><dd>英语</dd></dl>
      <dl><dt>地区：</dt><dd>美国</dd></dl>
      <dl><dt>年份：</dt><dd>2026</dd></dl>
    </div>
  </div>
</div>
<div class="vod-intro"><p>剧情梗概一。</p><p>剧情梗概二。</p></div>
<div class="box" id="playlist_1"><div class="box-title"><h2><i class="title-icon">▶️</i>主线路-在线播放</h2></div>
  <div class="video_list fn-clear"><a href="/play/SCCCCKyi-1-1.html">正片</a></div></div>
<div class="box" id="playlist_2"><div class="box-title"><h2>其他线路-在线播放</h2></div>
  <div class="video_list fn-clear"><a href="/play/SCCCCKyi-2-1.html">正片</a></div></div>
</body></html>"#;

const PLAY_HTML: &str = r#"<html><body><script>var player_aaaa={"flag":"play","encrypt":0,"trysee":0,"points":0,"url":"https:\/\/hn.bfvvs.com\/play\/bqxVWjDa\/index.m3u8","link":"\/play\/SCCCCKyi-1-1.html"};</script></body></html>"#;

const SEARCH_HTML: &str = r#"<html><body><ul class="show-list">
<li><a class="play-img" href="/vod/SCCCCKix.html"><img src="https://img.tangrenjie.net/s.jpg"/></a>
  <div class="play-txt"><h2><a href="/vod/SCCCCKix.html">吸血鬼嘉年华</a></h2>
    <dl><dt>主演：</dt><dd>内尔,基隆</dd></dl>
    <dl><dt>状态：</dt><dd><span class="color">正片</span></dd></dl></div></li>
</ul></body></html>"#;

const NO_TITLE_HTML: &str = r#"<html><body><div class="box">系统提示......</div></body></html>"#;

const BROKEN_PLAY_HTML: &str = r#"<html><body><script>var other = 1;</script></body></html>"#;

fn respond_html(request: tiny_http::Request, body: &'static str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("header");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

fn start_server() -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || loop {
        let Ok(request) = server.recv() else {
            return;
        };
        let url = request.url().to_string();
        if url == "/" {
            respond_html(request, HOME_HTML);
        } else if url.starts_with("/type/20.html") {
            respond_html(request, LIST_HTML);
        } else if url.starts_with("/show/20--------2---.html") {
            respond_html(request, LIST2_HTML);
        } else if url.starts_with("/vod/SCCCCKyi.html") {
            respond_html(request, DETAIL_HTML);
        } else if url.starts_with("/play/SCCCCKyi-1-1.html") {
            respond_html(request, PLAY_HTML);
        } else if url.starts_with("/play/broken.html") {
            respond_html(request, BROKEN_PLAY_HTML);
        } else if url.starts_with("/search/") {
            respond_html(request, SEARCH_HTML);
        } else if url.starts_with("/vod/gone.html") {
            respond_html(request, NO_TITLE_HTML);
        } else {
            respond_html(request, "<html><body>404</body></html>");
        }
    });
    base
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "tangrenjie".into(),
        name: "唐人街影院".into(),
        api: "csp_Tangrenjie".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_come_from_nav() {
    let cfg = cfg();
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("cats");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "20");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[1].type_id, "35");
    assert_eq!(cats[1].type_name, "动漫");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn videos_first_page_parses_rows() {
    let cfg = cfg();
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "20", 1).expect("list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "/vod/SCCCCKw8.html");
    assert_eq!(items[0].name, "梁祝传说");
    assert_eq!(items[0].cover.as_deref(), Some("https://img.tangrenjie.net/a.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("正片"));
    assert_eq!(items[1].name, "寄生狂潮");
    assert_eq!(items[1].cover.as_deref(), Some("https://img.tangrenjie.net/b.jpg"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn videos_second_page_uses_show_route() {
    let cfg = cfg();
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "20", 2).expect("list");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "/vod/SCCCCKrx.html");
    assert_eq!(items[0].name, "第二页影片");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_parses_meta_and_two_line_groups() {
    let cfg = cfg();
    let base = start_server();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "/vod/SCCCCKyi.html")
        .expect("detail");
    assert_eq!(detail.name, "吸血新一代");
    assert_eq!(detail.cover.as_deref(), Some("https://img.tangrenjie.net/p.jpg"));
    assert_eq!(detail.actor.as_deref(), Some("斯派斯,凯利"));
    assert_eq!(detail.director.as_deref(), Some("弗兰克"));
    assert_eq!(detail.area.as_deref(), Some("美国"));
    assert_eq!(detail.year.as_deref(), Some("2026"));
    assert!(detail.desc.as_deref().unwrap_or("").contains("剧情梗概二"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "主线路");
    assert_eq!(detail.episodes[0].items[0].title, "正片");
    assert_eq!(detail.episodes[0].items[0].url, "/play/SCCCCKyi-1-1.html");
    assert_eq!(detail.episodes[1].group, "其他线路");
    assert_eq!(detail.episodes[1].items[0].url, "/play/SCCCCKyi-2-1.html");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_returns_m3u8_direct_link() {
    let cfg = cfg();
    let base = start_server();
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "flag",
        "/play/SCCCCKyi-1-1.html",
        &[],
    )
    .expect("player");
    assert_eq!(
        value.get("url").and_then(|v| v.as_str()),
        Some("https://hn.bfvvs.com/play/bqxVWjDa/index.m3u8")
    );
    assert_eq!(value.get("parse").and_then(|v| v.as_u64()), Some(0));
    let ua = value
        .get("header")
        .and_then(|h| h.get("User-Agent"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(ua.contains("Chrome/122"), "ua={ua}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_parses_rows() {
    let cfg = cfg();
    let base = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "吸血鬼", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "吸血鬼嘉年华");
    assert_eq!(items[0].id, "/vod/SCCCCKix.html");
    assert_eq!(items[0].cover.as_deref(), Some("https://img.tangrenjie.net/s.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("正片"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_missing_title_maps_to_parse_error() {
    let cfg = cfg();
    let base = start_server();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "/vod/gone.html")
        .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.result.parse"), "err={err:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_without_player_json_maps_to_parse_error() {
    let cfg = cfg();
    let base = start_server();
    let err = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "flag",
        "/play/broken.html",
        &[],
    )
    .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.result.parse"), "err={err:?}");
}

/// 真站冒烟：现役 `https://www.tangrenjie.net` 的分类 + 列表。
#[test]
#[ignore = "需要外网访问 https://www.tangrenjie.net"]
fn live_site_categories_and_list() {
    let cfg = cfg();
    let mut site = Site {
        key: "tangrenjie-live".into(),
        name: "唐人街影院".into(),
        api: "csp_Tangrenjie".into(),
        ..Default::default()
    };
    site.ext = None;
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("live cats");
    assert!(cats.iter().any(|c| c.type_name == "电影"), "cats={cats:?}");
    let first = &cats[0];
    let items = vbox_core::sources::spider::list_videos(&cfg, &site, &first.type_id, 1)
        .expect("live list");
    assert!(!items.is_empty(), "live list empty for {first:?}");
}
