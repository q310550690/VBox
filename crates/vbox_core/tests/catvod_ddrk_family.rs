//! `csp_Ddrk` 家族集成测试（tiny_http mock）。
//!
//! 覆盖：首页导航分类、首页推荐、分类列表（含第二页 URL）、详情（首屏「第1季」
//! 与 `.post-page-numbers` 触发的「第2季」）、搜索、播放（视频接口 JSON 直出）、
//! 空标题错误路径。
//!
//! 上游 `ddys.pro` 已 ENOTFOUND，`ddys.app`/`ddys.tv` 被 `ddys-protect` ALTCHA
//! 门禁挡住（返回的门禁页里没有 `post-box-container`），故本文件不含真站用例。

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
<div class="post-box-container">
  <a href="https://ddys.pro/other/" class="post-box-image"
     style="background-image:url('https://img.test/b.jpg')"></a>
  <h2 class="post-box-title"><a href="https://ddys.pro/other/">另一部电影 全30集</a></h2>
</div>
<div class="nav-links"><span class="current">1</span><a href="https://ddys.pro/category/oumeiju/page/2/">2</a></div>
</body></html>"#;

const LIST_PAGE2_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="post-box-container">
  <a href="https://ddys.pro/third/" class="post-box-image"
     style="background-image: url(https://img.test/c.jpg)"></a>
  <h2 class="post-box-title"><a href="https://ddys.pro/third/">第三部(全10集)</a></h2>
</div>
</body></html>"#;

const DETAIL_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="post"><img data-src="https://img.test/cover.jpg"></div>
<h1 class="post-title">幻视任务(更新至08集)</h1>
<div class="abstract">类型: 剧情/科幻 制片国家/地区: 美国 年份: 2025 简介: 一部科幻剧。 演员: 甲/乙 类 导演: 丙 演</div>
<div class="wp-playlist-script">{"tracks":[{"caption":"第01集","src1":"v1","subsrc":"s1"},{"caption":"第02集","src1":"v2","subsrc":"s2"}]}</div>
<div class="post-page-numbers">1</div><div class="post-page-numbers">2</div>
</body></html>"#;

const DETAIL_SEASON2_HTML: &str = r#"<!DOCTYPE html><html><body>
<h1 class="post-title">幻视任务 第二季</h1>
<div class="wp-playlist-script">{"tracks":[{"caption":"第01集","src1":"v3","subsrc":"s3"}]}</div>
</body></html>"#;

const DETAIL_EMPTY_HTML: &str = r#"<!DOCTYPE html><html><body><p>没有标题</p></body></html>"#;

const SEARCH_HTML: &str = r#"<!DOCTYPE html><html><body>
<article>
  <div class="post-title"><a href="https://ddys.pro/huanshi/">幻视任务</a></div>
  <time>2025-01-02</time><div class="cat-links">欧美剧</div>
</article>
</body></html>"#;

const PLAY_JSON: &str = r#"{"url":"https://cdn.test/ddrk.m3u8"}"#;

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
        if url.contains("/getvddr/video") {
            respond_json(request, PLAY_JSON);
        } else if url.starts_with("/?s=") {
            respond_html(request, SEARCH_HTML);
        } else if url.contains("/category/oumeiju/page/2/") {
            respond_html(request, LIST_PAGE2_HTML);
        } else if url.contains("/category/") {
            respond_html(request, LIST_HTML);
        } else if url.contains("/bad/") || url.ends_with("/bad") {
            respond_html(request, DETAIL_EMPTY_HTML);
        } else if url.contains("/huanshi/2") {
            respond_html(request, DETAIL_SEASON2_HTML);
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
        key: "ddrk".into(),
        name: "低端影视".into(),
        api: "csp_Ddrk".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
fn categories_parse_nav_and_skip_aggregate() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "oumeiju");
    assert_eq!(cats[0].type_name, "欧美剧");
    assert_eq!(cats[1].type_id, "riju");
    assert_eq!(cats[1].type_name, "日剧");
}

#[test]
fn home_lists_recommendations() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "", 1).expect("home");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "幻视任务");
    assert_eq!(items[0].remark.as_deref(), Some("更新至08集"));
    assert_eq!(items[0].cover.as_deref(), Some("https://img.test/home.jpg"));
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|u| u == "/"), "unexpected urls: {seen:?}");
}

#[test]
fn videos_parse_rows_with_and_without_parentheses() {
    let cfg = cfg();
    let (base, _) = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "oumeiju", 1).expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "huanshi");
    assert_eq!(items[0].name, "幻视任务");
    assert_eq!(items[0].remark.as_deref(), Some("更新至08集"));
    assert_eq!(items[0].cover.as_deref(), Some("https://img.test/a.jpg"));
    assert_eq!(items[1].id, "other");
    assert_eq!(items[1].name, "另一部电影");
    assert_eq!(items[1].remark.as_deref(), Some("全30集"));
    assert_eq!(items[1].cover.as_deref(), Some("https://img.test/b.jpg"));
}

#[test]
fn videos_page_two_hits_page_url() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "oumeiju", 2).expect("page2");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "第三部");
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u.contains("/category/oumeiju/page/2/")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
fn detail_merges_first_and_extra_seasons() {
    let cfg = cfg();
    let (base, _) = start_server();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "huanshi").expect("detail");
    assert_eq!(detail.name, "幻视任务");
    assert_eq!(detail.year.as_deref(), Some("2025"));
    assert_eq!(detail.area.as_deref(), Some("美国"));
    assert!(detail.desc.as_deref().unwrap_or("").contains("一部科幻剧"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "第1季");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(
        detail.episodes[0].items[0].url,
        format!("{base}/getvddr/video?id=v1&type=mix|{base}/subddr/s1")
    );
    assert_eq!(detail.episodes[1].group, "第2季");
    assert_eq!(detail.episodes[1].items.len(), 1);
}

#[test]
fn detail_without_title_reports_parse_error() {
    let cfg = cfg();
    let (base, _) = start_server();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "bad").expect_err("err");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.result.parse"), "unexpected: {msg}");
}

#[test]
fn search_parses_articles_with_fixed_cover() {
    let cfg = cfg();
    let (base, _) = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "幻视", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "huanshi");
    assert_eq!(items[0].name, "幻视任务");
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://s1.ax1x.com/2023/01/30/pSw9wrV.png")
    );
    assert_eq!(items[0].remark.as_deref(), Some("2025-01-02 欧美剧"));
}

#[test]
fn player_reads_video_api_url() {
    let cfg = cfg();
    let (base, _) = start_server();
    let id = format!("{base}/getvddr/video?id=abc&type=mix|{base}/subddr/xyz");
    let value = vbox_core::sources::spider::player_content(&cfg, &site(&base), "", &id, &[])
        .expect("player");
    assert_eq!(value["url"], "https://cdn.test/ddrk.m3u8");
    assert_eq!(value["parse"], 0);
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
