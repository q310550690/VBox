//! `csp_Xb6v` 家族集成测试（tiny_http mock + 真站回归）。
//!
//! 覆盖：分类来自 `#menus` 导航（跳过首页/绝对链接）、列表分页 URL、行字段解析、
//! 详情元数据（`div.context` 纯文本行）与磁力线路（只保留 `magnet:`，跳过云盘表）、
//! 搜索（帝国 CMS POST）、播放磁力直出，以及空详情错误路径。
//! 上游 `www.xb6v.com` 在线，另有 1 个 `#[ignore]` 真站用例（列表 + 详情 + 播放）。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const HOME_HTML: &str = r#"<!DOCTYPE html><html><body>
<div id="menus">
  <li><a href="/">首页</a></li>
  <li><a href="https://www.xb6v.com/qian50m.html">最新50部</a></li>
  <li><a href="/dianshiju/">电视剧</a></li>
  <li><a href="/dianshiju/guoju/">国剧</a></li>
</div>
</body></html>"#;

const LIST_HTML: &str = r#"<!DOCTYPE html><html><body>
<div id="post_container">
  <li class="post">
    <div class="post_hover">
      <div class="thumbnail">
        <a class="zoom" href="/dianshiju/guoju/29956.html" title="玫瑰战争"><img src="/upload/vod/1.webp"></a>
      </div>
      <div class="article">
        <h2><a href="/dianshiju/guoju/29956.html">玫瑰战争</a></h2>
        <p><span rel="category tag">国剧</span></p>
      </div>
    </div>
  </li>
  <li class="post">
    <div class="post_hover">
      <div class="thumbnail">
        <a class="zoom" href="/dianshiju/guoju/30000.html" title="第二部剧"><img src="//img.test/2.webp"></a>
      </div>
      <div class="article">
        <h2><a href="/dianshiju/guoju/30000.html">第二部剧</a></h2>
        <p><span rel="category tag">国剧</span></p>
      </div>
    </div>
  </li>
</div>
</body></html>"#;

const LIST_PAGE2_HTML: &str = r#"<!DOCTYPE html><html><body>
<div id="post_container">
  <li class="post">
    <div class="post_hover">
      <div class="thumbnail">
        <a class="zoom" href="/dianshiju/guoju/40000.html" title="第三部剧"><img src="/upload/vod/3.webp"></a>
      </div>
      <div class="article"><h2><a href="/dianshiju/guoju/40000.html">第三部剧</a></h2></div>
    </div>
  </li>
</div>
</body></html>"#;

const DETAIL_HTML: &str = r#"<!DOCTYPE html><html><body>
<h1>站点标题</h1>
<div class="article_container"><h1>玫瑰战争</h1></div>
<div id="post_content">
  <img src="https://img.dytt-tupian.com/upload/vod/1.webp">
  <table><tr><td>磁力：<a href="magnet:?xt=urn:btih:abc">01-06.2160p</a></td></tr></table>
  <table><tr><td><a href="https://pan.xunlei.com/s/xyz">迅雷云盘</a></td></tr></table>
</div>
<div class="context">
  ◎片　　名:　玫瑰戰爭<br>
  ◎年　　代:　2026<br>
  ◎产　　地:　中国香港 / 中国大陆<br>
  ◎类　　别:　剧情<br>
  ◎导　　演:　钟澍佳 Chung Shu Kai<br>
  ◎主　　演:　杨茜尧 Tavia Yeung<br>
  陈豪 Moses Chan<br>
  林夏薇 Rosina Lin<br>
  ◎简　　介<br>
  一宗骇人的碎尸案。
</div>
</body></html>"#;

const DETAIL_EMPTY_HTML: &str = r#"<!DOCTYPE html><html><body><p>空页面</p></body></html>"#;

const SEARCH_HTML: &str = r#"<!DOCTYPE html><html><body>
<div id="post_container">
  <li class="post">
    <div class="post_hover">
      <div class="thumbnail">
        <a class="zoom" href="/dianshiju/guoju/29956.html" title="玫瑰战争"><img src="/upload/vod/1.webp"></a>
      </div>
      <div class="article">
        <h2><a href="/dianshiju/guoju/29956.html">玫瑰战争</a></h2>
        <p><span rel="category tag">国剧</span></p>
      </div>
    </div>
  </li>
</div>
</body></html>"#;

fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header")
}

fn respond(request: tiny_http::Request, body: &str) {
    let _ = request.respond(Response::from_string(body).with_header(html_header()));
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
        if url == "/" {
            respond(request, HOME_HTML);
        } else if url.contains("/e/search/index.php") {
            respond(request, SEARCH_HTML);
        } else if url.contains("/dianshiju/guoju/bad.html") {
            respond(request, DETAIL_EMPTY_HTML);
        } else if url.contains("index_2.html") {
            respond(request, LIST_PAGE2_HTML);
        } else if url.contains("/dianshiju/guoju/") && url.ends_with(".html") {
            respond(request, DETAIL_HTML);
        } else if url.contains("/dianshiju/guoju/") {
            respond(request, LIST_HTML);
        } else {
            respond(request, "<html><body>not found</body></html>");
        }
    });
    (base, seen)
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "xb6v".into(),
        name: "6v".into(),
        api: "csp_Xb6v".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

fn live_site() -> Site {
    Site {
        key: "xb6v".into(),
        name: "6v".into(),
        api: "csp_Xb6v".into(),
        ..Default::default()
    }
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_come_from_menu_and_skip_home() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "/dianshiju/");
    assert_eq!(cats[0].type_name, "电视剧");
    assert_eq!(cats[1].type_id, "/dianshiju/guoju/");
    assert_eq!(cats[1].type_name, "国剧");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn videos_parse_rows_and_absolutize_cover() {
    let cfg = cfg();
    let (base, _) = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "/dianshiju/guoju/", 1).expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "/dianshiju/guoju/29956.html");
    assert_eq!(items[0].name, "玫瑰战争");
    assert_eq!(
        items[0].cover.as_deref(),
        Some(format!("{base}/upload/vod/1.webp").as_str())
    );
    assert_eq!(items[0].remark.as_deref(), Some("国剧"));
    assert_eq!(items[1].cover.as_deref(), Some("https://img.test/2.webp"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn videos_page_two_uses_index_segment() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "/dianshiju/guoju/", 2).expect("videos2");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "第三部剧");
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u.contains("/dianshiju/guoju/index_2.html")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_reads_metadata_and_only_magnet_tables() {
    let cfg = cfg();
    let (base, _) = start_server();
    let id = "/dianshiju/guoju/29956.html";
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), id).expect("detail");
    assert_eq!(detail.name, "玫瑰战争");
    assert_eq!(detail.year.as_deref(), Some("2026"));
    assert_eq!(detail.area.as_deref(), Some("中国香港 / 中国大陆"));
    assert_eq!(detail.director.as_deref(), Some("钟澍佳 Chung Shu Kai"));
    let actor = detail.actor.unwrap_or_default();
    assert!(actor.contains("杨茜尧"), "actor={actor}");
    assert!(actor.contains("陈豪 Moses Chan"), "actor={actor}");
    assert!(detail
        .desc
        .as_deref()
        .unwrap_or_default()
        .contains("碎尸案"));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "磁力线路1");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(detail.episodes[0].items[0].url, "magnet:?xt=urn:btih:abc");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_without_data_reports_parse_error() {
    let cfg = cfg();
    let (base, _) = start_server();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "/dianshiju/guoju/bad.html")
        .expect_err("err");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.result.parse"), "unexpected: {msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_posts_empire_form_and_parses_rows() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "玫瑰", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "玫瑰战争");
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u.contains("/e/search/index.php")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_returns_magnet_with_headers() {
    let cfg = cfg();
    let (base, _) = start_server();
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "",
        "magnet:?xt=urn:btih:abc",
        &[],
    )
    .expect("player");
    assert_eq!(value["url"], "magnet:?xt=urn:btih:abc");
    assert_eq!(value["parse"], 0);
    let ua = value["header"]["User-Agent"].as_str().unwrap_or("");
    assert!(ua.contains("Chrome/122"), "ua={ua}");
}

/// 真站回归：分类（含「国剧」）、国剧列表首页 ≥10 条、首条详情有标题与磁力线路。
#[test]
#[ignore]
fn live_xb6v_lists_and_details() {
    let cfg = cfg();
    let site = live_site();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("live categories");
    assert!(cats.iter().any(|c| c.type_name == "国剧"), "cats={cats:?}");

    let items = vbox_core::sources::spider::list_videos(&cfg, &site, "/dianshiju/guoju/", 1)
        .expect("live videos");
    assert!(items.len() >= 10, "len={}", items.len());
    assert!(items[0].name.contains("玫瑰") || !items[0].name.is_empty());

    let first = &items[0];
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site, &first.id).expect("live detail");
    assert!(!detail.name.is_empty(), "detail name empty");
    assert!(!detail.episodes.is_empty(), "detail episodes empty");

    let magnet = detail.episodes[0]
        .items
        .first()
        .map(|e| e.url.clone())
        .unwrap_or_default();
    let value = vbox_core::sources::spider::player_content(&cfg, &site, "", &magnet, &[])
        .expect("live player");
    assert_eq!(value["parse"], 0);
    assert!(value["url"].as_str().unwrap_or("").starts_with("magnet:"));
}
