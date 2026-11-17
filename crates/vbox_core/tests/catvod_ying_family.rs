//! `csp_Ying` 家族集成测试（tiny_http mock）。
//!
//! 覆盖：catalog.js 标签 → 分类、列表（含 region 过滤 URL）、详情（元数据 + 双线路）、
//! 搜索、播放（相对地址补基址）、错误路径。两处上游当前均不可达，故全部为 mock 验收。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const CATALOG: &str = r#"var _labels = [["genre","类型","全部","剧情","喜剧"],["region","地区","全部","大陆","香港"],["year","年份","全部","2024","2023"]];"#;

const LIST_HTML: &str = r#"
<div class="lpic"><ul>
  <li><a href="/showp/123"><img src="//img.example/1.jpg"></a>
      <h2>示例番剧</h2><span><font>更新至12话</font></span></li>
  <li><a href="/showp/456"><img src="//img.example/2.jpg"></a>
      <h2>第二部</h2><span><font>状态 : 完结</font></span></li>
</ul></div>"#;

const LIST_MAINLAND: &str = r#"
<div class="lpic"><ul>
  <li><a href="/showp/789"><img src="//img.example/3.jpg"></a>
      <h2>大陆番</h2><span><font>更新至3话</font></span></li>
</ul></div>"#;

const DETAIL_HTML: &str = r#"
<div class="rate"><h1>示例番剧</h1></div>
<div class="thumb"><img src="//img.example/c.jpg"></div>
<div class="info">一段简介</div>
<div class="sinfo"><span><a>2024</a></span><span><a>大陆</a></span><span><a>动画</a></span></div>
<ul class="menu0"><li>线路A</li><li>线路B</li></ul>
<div class="main0">
  <div><a href="/p/1-1-1.html">第1集</a><a href="/p/1-1-2.html">第2集</a></div>
  <div><a href="/p/1-2-1.html">第1集</a></div>
</div>"#;

fn text_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header")
}

fn respond(request: tiny_http::Request, body: &str, content_type: Header) {
    let _ = request.respond(Response::from_string(body).with_header(content_type));
}

fn start_server() -> Arc<Server> {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let handle = Arc::clone(&server);
    thread::spawn(move || loop {
        let Ok(request) = handle.recv() else { return };
        let url = request.url().to_string();
        if url.starts_with("/yxsf/js/yx_catalog.js") {
            respond(request, CATALOG, text_header());
        } else if url.starts_with("/list/") {
            let body = if url.contains("region=%E5%A4%A7%E9%99%86") || url.contains("region=大陆") {
                LIST_MAINLAND
            } else {
                LIST_HTML
            };
            respond(request, body, text_header());
        } else if url.starts_with("/showp/123") {
            respond(request, DETAIL_HTML, text_header());
        } else if url.starts_with("/showp/bad") {
            respond(request, "<html><body>not detail</body></html>", text_header());
        } else if url.starts_with("/s_all") {
            respond(request, LIST_MAINLAND, text_header());
        } else {
            respond(request, "<html>404</html>", text_header());
        }
    });
    server
}

fn base_of(server: &Arc<Server>) -> String {
    format!("http://{}", server.server_addr())
}

fn site(base: &str) -> Site {
    Site {
        key: "ying".to_string(),
        name: "樱花测试站".to_string(),
        kind: 3,
        api: "csp_Ying".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
fn categories_parse_catalog_labels() {
    let server = start_server();
    let cfg = TvConfig::default();
    let cats =
        vbox_core::sources::spider::list_categories(&cfg, &site(&base_of(&server))).expect("cats");
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(names, vec!["大陆", "香港"]);
    assert_eq!(cats[0].type_id, "大陆");
}

#[test]
fn list_videos_via_dispatch() {
    let server = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::list_videos(&cfg, &site(&base_of(&server)), "", 1)
        .expect("videos");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "123");
    assert_eq!(rows[0].name, "示例番剧");
    assert_eq!(rows[0].cover.as_deref(), Some("https://img.example/1.jpg"));
    assert_eq!(rows[0].remark.as_deref(), Some("更新至12话"));
    assert_eq!(rows[1].remark.as_deref(), Some("完结"));
}

#[test]
fn list_videos_applies_region_filter() {
    let server = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::list_videos(&cfg, &site(&base_of(&server)), "大陆", 1)
        .expect("videos");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "789");
}

#[test]
fn detail_maps_meta_and_two_playlists() {
    let server = start_server();
    let cfg = TvConfig::default();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base_of(&server)), "123")
        .expect("detail");
    assert_eq!(detail.name, "示例番剧");
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.desc.as_deref(), Some("一段简介"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路A");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].url, "/p/1-1-2.html");
    assert_eq!(detail.episodes[1].group, "线路B");
}

#[test]
fn detail_reports_parse_error_when_layout_changes() {
    let server = start_server();
    let cfg = TvConfig::default();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base_of(&server)), "bad")
        .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.result.parse"), "{err:?}");
}

#[test]
fn search_via_dispatch() {
    let server = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::search(&cfg, &site(&base_of(&server)), "大陆", false)
        .expect("search");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "789");
    assert_eq!(rows[0].name, "大陆番");
}

#[test]
fn player_prefixes_base_and_returns_ua_header() {
    let server = start_server();
    let cfg = TvConfig::default();
    let base = base_of(&server);
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "线路A",
        "/p/1-1-1.html",
        &[],
    )
    .expect("player");
    assert_eq!(value.get("parse"), Some(&json!(0)));
    assert_eq!(
        value.get("url").and_then(|v| v.as_str()),
        Some(format!("{base}/p/1-1-1.html").as_str())
    );
    assert!(value
        .get("header")
        .and_then(|h| h.get("User-Agent"))
        .and_then(|v| v.as_str())
        .map(|ua| ua.contains("Chrome/109"))
        .unwrap_or(false));
}
