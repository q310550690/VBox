//! `csp_Libvio` 原生家族集成测试（本地 mock；上游站点均为反爬/403，无真站用例）。
//!
//! 覆盖：首页导航分类筛选、分类列表、第二页 12 段 URL、详情元数据与 SN 线路过滤、
//! 搜索（相对封面补全）、播放（播放页 `var player_` → 解析页 → `urls` 直链）、
//! 错误分级，以及 `sources::spider` 分派接线。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};

const HOME_HTML: &str = r#"<!doctype html><html><body>
<ul class="stui-header__menu">
  <li><a href="/">首页</a></li>
  <li><a href="/type/1.html">电影</a></li>
  <li><a href="/type/2.html">剧集</a></li>
  <li><a href="/type/30.html">综艺</a></li>
  <li><a href="/type/4.html">动漫</a></li>
</ul>
</body></html>"#;

const LIST_HTML: &str = r#"<!doctype html><html><body>
<div class="stui-vodlist__box">
  <a class="stui-vodlist__thumb" href="/detail/123.html" title="示例电影" data-original="/upload/a.jpg">
    <span class="pic-text text-right">更新至10集</span>
  </a>
</div>
<div class="stui-vodlist__box">
  <a class="stui-vodlist__thumb" href="/detail/456.html" title="另一部" data-original="https://img.test/b.jpg"></a>
</div>
</body></html>"#;

const DETAIL_HTML: &str = r#"<!doctype html><html><body>
<div class="stui-content__thumb"><a href="/detail/123.html"><img data-original="/upload/cover.jpg"></a></div>
<div class="stui-content__detail"><h1>示例电影</h1></div>
<p class="data">类型：剧情&nbsp;地区：中国大陆&nbsp;年份：2021</p>
<p class="data">主演：张三&nbsp;导演：李四</p>
<p class="data">更新：更新至10集</p>
<span class="detail-content"> 一段简介文本 </span>
<div class="stui-vodlist__head"><h3>AI播放</h3></div>
<ul class="stui-content__playlist">
  <li><a href="/play/123-1-1.html">第1集</a></li>
  <li><a href="/play/123-1-2.html">第2集</a></li>
</ul>
<div class="stui-vodlist__head"><h3>未知线路</h3></div>
<ul class="stui-content__playlist">
  <li><a href="/play/123-9-9.html">第9集</a></li>
</ul>
</body></html>"#;

const SEARCH_JSON: &str = r#"{"total":2,"list":[
  {"id":"7","name":"示例","pic":"/p.jpg"},
  {"id":8,"name":"另一部","pic":"https://cdn.test/q.jpg"}
]}"#;

const PARSE_HTML: &str = r#"<!doctype html><html><body>
<script>var urls = 'http://cdn.test/video.m3u8';</script>
</body></html>"#;

fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = Arc::clone(&seen);
    let base2 = base.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            seen2.lock().unwrap().push(url.clone());
            let (status, body, ctype) = if url == "/" {
                (200, HOME_HTML.to_string(), "text/html; charset=utf-8")
            } else if url.starts_with("/show/") {
                (200, LIST_HTML.to_string(), "text/html; charset=utf-8")
            } else if url.starts_with("/detail/bad.html") {
                (200, "<html><body></body></html>".to_string(), "text/html; charset=utf-8")
            } else if url.starts_with("/detail/") {
                (200, DETAIL_HTML.to_string(), "text/html; charset=utf-8")
            } else if url.starts_with("/index.php/ajax/suggest") {
                (200, SEARCH_JSON.to_string(), "application/json")
            } else if url.starts_with("/play/") {
                let player = format!(
                    r#"<script>var player_ = {{"from":"line401","url":"{base2}/parse/page.html"}};</script>"#
                );
                (200, player, "text/html; charset=utf-8")
            } else if url.starts_with("/parse/page.html") {
                (200, PARSE_HTML.to_string(), "text/html; charset=utf-8")
            } else {
                (404, "{}".to_string(), "application/json")
            };
            let _ = req.respond(
                Response::from_string(body)
                    .with_status_code(status)
                    .with_header(Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap()),
            );
        }
    });
    (base, seen)
}

fn site(base: &str) -> Site {
    Site {
        key: "libvio".to_string(),
        name: "LibVio".to_string(),
        api: "csp_Libvio".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
fn home_categories_filter_menu_items() {
    let (base, _seen) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&TvConfig::default(), &site(&base))
        .expect("分类");
    assert_eq!(cats.len(), 3, "综艺应被过滤，实际: {cats:?}");
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[2].type_id, "4");
    assert_eq!(cats[2].type_name, "动漫");
}

#[test]
fn list_parses_rows_and_second_page_url() {
    let (base, seen) = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&TvConfig::default(), &site(&base), "1", 2)
            .expect("列表");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].id, "123");
    assert_eq!(items[0].name, "示例电影");
    assert_eq!(items[0].remark.as_deref(), Some("更新至10集"));
    assert_eq!(items[0].cover.as_deref(), Some("/upload/a.jpg"));
    assert_eq!(items[1].id, "456");
    assert!(items[1].remark.is_none());
    let paths = seen.lock().unwrap().clone();
    assert!(
        paths.iter().any(|p| p == "/show/1--------2---.html"),
        "第二页应使用 12 段 URL，实际: {paths:?}"
    );
}

#[test]
fn detail_parses_metadata_and_known_groups() {
    let (base, _seen) = start_server();
    let d = vbox_core::sources::spider::get_detail(&TvConfig::default(), &site(&base), "123")
        .expect("详情");
    assert_eq!(d.name, "示例电影");
    assert_eq!(d.year.as_deref(), Some("2021"));
    assert_eq!(d.area.as_deref(), Some("中国大陆"));
    assert_eq!(d.actor.as_deref(), Some("张三"));
    assert_eq!(d.director.as_deref(), Some("李四"));
    assert_eq!(d.desc.as_deref(), Some("一段简介文本"));
    assert_eq!(d.cover.as_deref(), Some(format!("{base}/upload/cover.jpg").as_str()));
    assert_eq!(d.episodes.len(), 1, "未知线路应被 SN 表过滤: {d:?}");
    assert_eq!(d.episodes[0].group, "AI播放");
    assert_eq!(d.episodes[0].items.len(), 2);
    assert_eq!(d.episodes[0].items[0].url, "123-1-1");
    assert_eq!(d.episodes[0].items[1].title, "第2集");
}

#[test]
fn detail_without_content_reports_parse_error() {
    let (base, _seen) = start_server();
    let err = vbox_core::sources::spider::get_detail(&TvConfig::default(), &site(&base), "bad")
        .expect_err("应报错");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "{text}");
}

#[test]
fn search_fills_relative_cover() {
    let (base, _seen) = start_server();
    let items =
        vbox_core::sources::spider::search(&TvConfig::default(), &site(&base), "示例", false)
            .expect("搜索");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "7");
    assert_eq!(items[0].cover.as_deref(), Some(format!("{base}/p.jpg").as_str()));
    assert_eq!(items[1].cover.as_deref(), Some("https://cdn.test/q.jpg"));
}

#[test]
fn player_follows_player_page_to_direct_url() {
    let (base, _seen) = start_server();
    let out = vbox_core::sources::spider::player_content(
        &TvConfig::default(),
        &site(&base),
        "AI播放",
        "123-1-1",
        &[],
    )
    .expect("播放");
    assert_eq!(out["parse"], 0);
    assert_eq!(out["url"], "http://cdn.test/video.m3u8");
    assert_eq!(out["header"]["Referer"], base);
}
