//! `csp_Ddys` 家族集成测试（tiny_http mock）。
//!
//! `csp_Ddys` 与 `csp_Ddrk` 同站点（`ddys.pro`）、不同 jar 实现，因此共用
//! `families/ddrk.rs` 里的 heredoc 解析器，只在三处分叉：列表页页码片段、
//! 详情剧集的 `srctype`（1→`getvddr2`，4→`v.ddys.pro`）、搜索选择器。
//!
//! 覆盖：分派与原生就绪标记、首页分类、分类第一页 URL、详情三条剧集的
//! 差异化 URL、搜索关键词过滤 + favicon 封面、播放 JSON 直出与返回头。
//!
//! 上游 `ddys.pro` 已 ENOTFOUND，`ddys.app` 被 `ddys-protect` 密码门禁挡住，
//! 故本文件不含真站用例。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const HOME_HTML: &str = r#"<!DOCTYPE html><html><body>
<ul id="menu-main"><li class="menu-item"><a title="热映中" href="/">热映中</a></li>
<li class="menu-item"><a title="欧美剧" href="https://ddys.pro/category/oumeiju/">欧美剧</a></li>
<li class="menu-item"><a title="日剧" href="https://ddys.pro/category/riju/">日剧</a></li></ul>
<div class="post-box-container">
  <a href="https://ddys.pro/huanshi/" class="post-box-image"
     style="background-image: url(https://img.test/home.jpg)"></a>
  <h2 class="post-box-title"><a href="https://ddys.pro/huanshi/">幻视任务(更新至08集)</a></h2>
</div>
</body></html>"#;

const LIST_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="post-box-container">
  <a href="https://ddys.pro/huanshi/" class="post-box-image"
     style="background-image: url(https://img.test/a.jpg)"></a>
  <h2 class="post-box-title"><a href="https://ddys.pro/huanshi/">幻视任务(更新至08集)</a></h2>
</div>
</body></html>"#;

const DETAIL_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="post"><img data-src="https://img.test/cover.jpg"></div>
<h1 class="post-title">幻视任务(更新至08集)</h1>
<div class="abstract">类型: 剧情/科幻 制片国家/地区: 美国 年份: 2025 简介: 一部科幻剧。 演员: 甲/乙 类 导演: 丙 演</div>
<div class="wp-playlist-script">{"tracks":[
  {"caption":"第01集","srctype":"1","src1":"v1","subsrc":"s1"},
  {"caption":"第02集","srctype":"4","src3":"/cdn/v2.m3u8","subsrc":"s2"},
  {"caption":"第03集","srctype":"9","src1":"v3","subsrc":"s3"}]}</div>
</body></html>"#;

const SEARCH_HTML: &str = r##"<div id="container">
  <article><h2>幻视任务 全08集</h2><a href="https://ddys.pro/huanshi/"></a>
    <li class="meta_categories">欧美剧</li></article>
  <article><h2>不相干条目</h2><a href="https://ddys.pro/other/"></a></article>
</div>"##;

const PLAY_JSON: &str = r#"{"url":"https://cdn.test/ddys.m3u8"}"#;

fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header")
}

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header")
}

fn respond_html(request: tiny_http::Request, body: &str) {
    let _ = request.respond(Response::from_string(body).with_header(html_header()));
}

fn respond_json(request: tiny_http::Request, body: &str) {
    let _ = request.respond(Response::from_string(body).with_header(json_header()));
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
        if url.contains("/getvddr2/video") {
            respond_json(request, PLAY_JSON);
        } else if url.starts_with("/?s=") {
            respond_html(request, SEARCH_HTML);
        } else if url.contains("/category/") {
            respond_html(request, LIST_HTML);
        } else if url.contains("/huanshi") {
            respond_html(request, DETAIL_HTML);
        } else if url == "/" {
            respond_html(request, HOME_HTML);
        } else {
            respond_html(request, "<html><body>not found</body></html>");
        }
    });
    (base, seen)
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "ddys".into(),
        name: "低端影视(Ddys)".into(),
        api: "csp_Ddys".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
fn dispatch_and_registry_recognise_ddys() {
    let family = vbox_core::spider::catvod::families::family_of("csp_Ddys");
    assert_eq!(family.as_deref(), Some("ddrk"));
    assert!(vbox_core::spider::catvod::registry::is_native_ready("csp_Ddys"));
    assert!(vbox_core::spider::catvod::registry::blocked_reason("csp_Ddys").is_none());
}

#[test]
fn categories_parse_nav_and_skip_aggregate() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "oumeiju");
    assert_eq!(cats[0].type_name, "欧美剧");
}

#[test]
fn category_first_page_uses_page_segment() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "oumeiju", 1).expect("videos");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "huanshi");
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u.contains("/category/oumeiju/page/1/")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
fn detail_builds_srctype_specific_episode_urls() {
    let cfg = cfg();
    let (base, _) = start_server();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "huanshi").expect("detail");
    assert_eq!(detail.name, "幻视任务");
    assert_eq!(detail.episodes.len(), 1);
    let items = &detail.episodes[0].items;
    assert_eq!(items.len(), 3);
    assert_eq!(
        items[0].url,
        format!("{base}/getvddr2/video?id=v1&type=mix|{base}/subddr/s1")
    );
    assert_eq!(
        items[1].url,
        format!("https://v.ddys.pro/cdn/v2.m3u8|{base}/subddr/s2")
    );
    assert_eq!(items[2].url, format!("|{base}/subddr/s3"));
}

#[test]
fn search_filters_by_keyword_and_uses_favicon() {
    let cfg = cfg();
    let (base, _) = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "幻视", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "huanshi");
    assert_eq!(items[0].name, "幻视任务 全08集");
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://ddys.pro/favicon-32x32.png")
    );
    assert_eq!(items[0].remark.as_deref(), Some("欧美剧"));
}

#[test]
fn player_reads_getvddr2_json_and_sets_mac_ua() {
    let cfg = cfg();
    let (base, _) = start_server();
    let id = format!("{base}/getvddr2/video?id=abc&type=mix|{base}/subddr/xyz");
    let value = vbox_core::sources::spider::player_content(&cfg, &site(&base), "", &id, &[])
        .expect("player");
    assert_eq!(value["url"], "https://cdn.test/ddys.m3u8");
    assert_eq!(value["parse"], 0);
    let header = value["header"].as_str().unwrap_or("");
    assert!(header.contains("Macintosh"), "header: {header}");
    assert!(header.contains("Referer"), "header: {header}");
}

#[test]
fn player_rejects_id_without_separator() {
    let cfg = cfg();
    let (base, _) = start_server();
    let err = vbox_core::sources::spider::player_content(&cfg, &site(&base), "", "no-sep", &[])
        .expect_err("err");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.result.parse"), "unexpected: {msg}");
}
