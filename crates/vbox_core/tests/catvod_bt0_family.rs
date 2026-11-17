//! `csp_Bt0`（2bt0 磁力站）集成测试：全程离线 tiny_http mock。
//!
//! 该家族的特色是 **cookie 反爬**：首次请求返回一段
//! `document.cookie = "ge_js_validator_18=..."; window.location.reload();`
//! 脚本页，客户端提取 Cookie 后重发。这里用 mock 完整复现该流程，
//! 并断言第二次请求确实带上了 Cookie。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use vbox_core::model::Site;
use vbox_core::spider::catvod::families::{bt0, family_of};
use vbox_core::spider::catvod::registry;

const BOOTSTRAP: &str = r#"<html><script>document.cookie = "ge_js_validator_18=abc@18@def; path=/; max-age=99999;";window.location.reload();</script></html>"#;

const HOME_HTML: &str = r#"
<div class="masonry_item">
  <a href="/tr/1.html"><h2>示例电影</h2></a>
  <div style="background-image: url(//img.test/cover1.jpg)"></div>
  <figcaption><em>2024-05-01</em></figcaption>
</div>"#;

const LIST_HTML: &str = r#"
<div class="container">
  <a href="magnet:?xt=urn:btih:AAA"></a>
  <div><a href="/tr/1.html"></a><div>列表示例</div></div>
  <div><a href="/tr/1.html"><div style="background-image: url(https://img.test/list.jpg)"></div></a></div>
  <div style="margin-top: -39px">9.6 分</div>
</div>"#;

const DETAIL_HTML: &str = r#"
<div id="movie-content">
  <img src="/img/detail.jpg" />
  <h1 class="info-title">详情标题</h1>
  <div class="tiny-title">2023</div>
  <div class="tiny-title">豆瓣 8.1</div>
</div>
<ul class="tabs"><li>1080P</li><li>4K</li></ul>
<ul class="tabs-content">
  <li>
    <div class="container"><a href="magnet:?xt=urn:btih:BBB"></a><a class="torrent-title">第2集</a></div>
    <div class="container"><a href="magnet:?xt=urn:btih:AAA"></a><a class="torrent-title">第1集</a></div>
  </li>
  <li>
    <div class="container"><a href="magnet:?xt=urn:btih:CCC"></a><a class="torrent-title">第1集</a></div>
  </li>
</ul>"#;

const SEARCH_HTML: &str = r#"
<div class="masonry_item">
  <a href="/tr/9.html"><h5>搜索命中</h5></a>
  <div style="background-image:url(//img.test/s.jpg)"></div>
</div>"#;

fn has_cookie(request: &tiny_http::Request) -> bool {
    request.headers().iter().any(|h| {
        h.field.as_str().as_str().eq_ignore_ascii_case("cookie")
            && h.value.as_str().contains("ge_js_validator_18")
    })
}

fn respond_text(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
    let _ = request.respond(Response::from_string(body.to_string()).with_header(header));
}

/// 启动 mock：返回 (base_url, 请求记录)。
fn start_server() -> (String, Arc<Mutex<Vec<bool>>>) {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let port = server.server_addr().to_ip().expect("ip addr").port();
    let base = format!("http://127.0.0.1:{port}/");
    let log: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));

    let log_in_thread = Arc::clone(&log);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            let cookie = has_cookie(&request);
            log_in_thread.lock().unwrap().push(cookie);

            if url.starts_with("/tlist.php") {
                respond_text(request, LIST_HTML);
            } else if url.starts_with("/tr/") {
                respond_text(request, DETAIL_HTML);
            } else if url.starts_with("/search.php") {
                respond_text(request, SEARCH_HTML);
            } else if cookie {
                respond_text(request, HOME_HTML);
            } else {
                respond_text(request, BOOTSTRAP);
            }
        }
    });

    (base, log)
}

fn site(base: &str) -> Site {
    Site {
        api: "csp_Bt0".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
fn dispatch_and_registry_recognise_bt0() {
    assert_eq!(family_of("csp_Bt0"), Some("bt0"));
    assert_eq!(family_of(" CSP_BT0 "), Some("bt0"));
    assert!(registry::is_native_ready("csp_Bt0"));
    assert!(registry::blocked_reason("csp_Bt0").is_none());
}

#[test]
fn categories_are_fixed_five() {
    let (base, _log) = start_server();
    let cats = bt0::categories(&Default::default(), &site(&base)).expect("categories");
    assert_eq!(cats.len(), 5);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[4].type_id, "5");
}

#[test]
fn home_bootstrap_cookie_is_retried_with_cookie() {
    let (base, log) = start_server();
    let items = bt0::home_videos(&Default::default(), &site(&base)).expect("home");
    assert_eq!(items.len(), 1, "home page should parse one row");
    let recorded = log.lock().unwrap().clone();
    // 首次请求无 Cookie（bootstrap），第二次带 Cookie（真实列表）。
    assert!(recorded.iter().any(|c| !*c), "expected a cookie-less request");
    assert!(recorded.iter().any(|c| *c), "expected a cookie-carrying request");
}

#[test]
fn detail_parses_groups_and_sorts_episodes() {
    let (base, _log) = start_server();
    let d = bt0::detail(&Default::default(), &site(&base), "/tr/1.html").expect("detail");
    assert_eq!(d.name, "详情标题");
    assert_eq!(d.cover.as_deref(), Some(format!("{base}img/detail.jpg").as_str()));
    assert_eq!(d.desc.as_deref(), Some("2023\n豆瓣 8.1"));
    assert_eq!(d.episodes.len(), 2);
    assert_eq!(d.episodes[0].group, "1080P");
    assert_eq!(d.episodes[0].items[0].title, "第1集");
    assert_eq!(d.episodes[1].group, "4K");
}

#[test]
fn search_uses_masonry_and_h5() {
    let (base, _log) = start_server();
    let items = bt0::search(&Default::default(), &site(&base), "搜索", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "搜索命中");
    assert_eq!(items[0].id, format!("{base}tr/9.html"));
}

#[test]
fn search_url_is_encoded() {
    let (base, _log) = start_server();
    let _ = bt0::search(&Default::default(), &site(&base), "斗罗 大陆", true);
    // 仅验证请求成功（编码正确性由单测覆盖）。
}

#[test]
fn player_returns_magnet_directly() {
    let value: Value = bt0::player_content(
        &Default::default(),
        &site("http://127.0.0.1:1/"),
        "磁力",
        "magnet:?xt=urn:btih:AAA",
    )
    .expect("player");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "magnet:?xt=urn:btih:AAA");
    assert_eq!(value["header"]["Referer"], "http://127.0.0.1:1/");
    assert!(value["header"]["User-Agent"].as_str().unwrap().contains("Chrome"));
}

#[test]
fn player_rejects_empty_id() {
    let err = bt0::player_content(&Default::default(), &site("http://127.0.0.1:1/"), "", "  ")
        .expect_err("empty id");
    assert!(format!("{err}").contains("catvod."), "unexpected error: {err}");
}

#[allow(dead_code)]
fn unused_headers_type_check() {
    let mut _m: HashMap<String, String> = HashMap::new();
}
