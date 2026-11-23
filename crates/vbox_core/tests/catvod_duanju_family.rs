//! `csp_Duanju` 家族集成测试（tiny_http mock）。
//!
//! 覆盖：8 个固定分类、分类列表（12 段 URL 与第二页）、详情（tab × 分集容器配对、
//! 标签里的年份/地区）、搜索、播放（`player_aaaa.url` 直出）、空详情/空播放页错误路径。
//! 上游 `duanju.one` 已 DNS 失效，因此本文件没有真站用例。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const LIST_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="module-items">
  <div class="module-item">
    <div class="module-item-pic">
      <a href="/voddetail/123.html" title="逆袭人生"><img data-src="//img.test/a.jpg"></a>
    </div>
    <div class="module-item-text">更新至12集</div>
  </div>
  <div class="module-item">
    <div class="module-item-pic">
      <a href="/voddetail/456.html" title="重生之我在都市"><img data-src="/u/456.jpg"></a>
    </div>
    <div class="module-item-text">全20集</div>
  </div>
</div>
</body></html>"#;

const LIST_PAGE2_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="module-items">
  <div class="module-item">
    <div class="module-item-pic">
      <a href="/voddetail/789.html" title="穿越之我在古代"><img data-src="/u/789.jpg"></a>
    </div>
    <div class="module-item-text">更新至3集</div>
  </div>
</div>
</body></html>"#;

const DETAIL_HTML: &str = r#"<!DOCTYPE html><html><body>
<h1 class="page-title">逆袭人生</h1>
<div class="tag-link"><a>抖剧</a></div>
<a class="tag-link">都市</a>
<a class="tag-link">2026</a>
<div class="title-info"><span>已完结</span></div>
<div class="module-info-pic"><img data-src="//img.test/cover.jpg"></div>
<div class="module-info-introduction"><div class="module-info-content">一个小人物的逆袭故事。</div></div>
<div class="module-tab-item tab-item"><span>线路1</span><small>12</small></div>
<div class="module-tab-item tab-item"><span>线路2</span><small>3</small></div>
<div class="scroll-content">
  <a href="/vodplay/123-1-1.html">第01集</a>
  <a href="/vodplay/123-1-2.html">第02集</a>
</div>
<div class="scroll-content">
  <a href="/vodplay/123-2-1.html">第01集</a>
</div>
</body></html>"#;

const DETAIL_EMPTY_HTML: &str = r#"<!DOCTYPE html><html><body>
<h1 class="page-title">无线路短剧</h1>
</body></html>"#;

const SEARCH_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="module-search-item">
  <div class="module-item-pic">
    <a href="/voddetail/9.html"><img alt="重生之我在都市" data-src="/u/9.jpg"></a>
  </div>
  <div class="video-info-header"><a class="video-serial">全20集</a></div>
</div>
</body></html>"#;

const PLAY_HTML: &str = r#"<!DOCTYPE html><html><body>
<script>var player_aaaa={"flag":"play","encrypt":3,"url":"https:\/\/cdn.test\/duanju.m3u8","from":"line1"};</script>
</body></html>"#;

const PLAY_BAD_HTML: &str = r#"<!DOCTYPE html><html><body><p>没有播放器</p></body></html>"#;

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
        if url.contains("/voddetail/bad.html") {
            respond(request, DETAIL_EMPTY_HTML);
        } else if url.contains("/voddetail/") {
            respond(request, DETAIL_HTML);
        } else if url.contains("/vodplay/bad.html") {
            respond(request, PLAY_BAD_HTML);
        } else if url.contains("/vodplay/") {
            respond(request, PLAY_HTML);
        } else if url.contains("/vodsearch/") {
            respond(request, SEARCH_HTML);
        } else if url.contains("2---.html") {
            respond(request, LIST_PAGE2_HTML);
        } else if url.contains("/vodshow/") {
            respond(request, LIST_HTML);
        } else {
            respond(request, "<html><body>not found</body></html>");
        }
    });
    (base, seen)
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "duanju".into(),
        name: "短剧".into(),
        api: "csp_Duanju".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_are_fixed_eight() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    assert_eq!(cats.len(), 8);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "抖剧");
    assert!(cats.iter().any(|c| c.type_id == "32" && c.type_name == "其他"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn videos_use_twelve_segment_url_and_parse_rows() {
    let cfg = cfg();
    let (base, _) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "1", 1).expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, format!("{base}/voddetail/123.html"));
    assert_eq!(items[0].name, "逆袭人生");
    assert_eq!(items[0].cover.as_deref(), Some("https://img.test/a.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("更新至12集"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn videos_page_two_hits_page_segment() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "1", 2).expect("videos p2");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "穿越之我在古代");
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u.contains("/vodshow/1--------2---.html")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_pairs_tabs_with_episode_containers() {
    let cfg = cfg();
    let (base, _) = start_server();
    let id = format!("{base}/voddetail/123.html");
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), &id).expect("detail");
    assert_eq!(detail.name, "逆袭人生");
    assert_eq!(detail.year.as_deref(), Some("2026"));
    assert_eq!(detail.area.as_deref(), Some("都市"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路1(共12集)");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(
        detail.episodes[0].items[0].url,
        format!("{base}/vodplay/123-1-1.html")
    );
    assert_eq!(detail.episodes[1].group, "线路2(共3集)");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_without_episodes_reports_parse_error() {
    let cfg = cfg();
    let (base, _) = start_server();
    let id = format!("{base}/voddetail/bad.html");
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), &id).expect_err("err");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.result.parse"), "unexpected: {msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_parses_module_search_rows() {
    let cfg = cfg();
    let (base, _) = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "逆袭", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, format!("{base}/voddetail/9.html"));
    assert_eq!(items[0].name, "重生之我在都市");
    assert_eq!(items[0].remark.as_deref(), Some("全20集"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_returns_player_aaaa_url() {
    let cfg = cfg();
    let (base, _) = start_server();
    let id = format!("{base}/vodplay/123-1-1.html");
    let value = vbox_core::sources::spider::player_content(&cfg, &site(&base), "线路1", &id, &[])
        .expect("player");
    assert_eq!(value["url"], "https://cdn.test/duanju.m3u8");
    assert_eq!(value["parse"], 0);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_without_player_aaaa_reports_host_api_error() {
    let cfg = cfg();
    let (base, _) = start_server();
    let id = format!("{base}/vodplay/bad.html");
    let err = vbox_core::sources::spider::player_content(&cfg, &site(&base), "", &id, &[])
        .expect_err("err");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.host.unsupported"), "unexpected: {msg}");
}
