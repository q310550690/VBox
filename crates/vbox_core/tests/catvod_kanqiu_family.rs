//! `csp_Kanqiu` 家族集成测试（tiny_http mock）。
//!
//! 覆盖：固定 4 分类、列表行解析（直播按钮 / 历史场次 / 头行跳过）、详情
//! `{id}-url` → `links[]`、播放直出（parse=0 + UA 头）、空 links 错误路径、
//! 搜索保持原实现的空结果。上游 `http://www.88kanqiu.one` 在线但
//! `-url` 端点已改版（404），故详情/播放为 mock 验收，分类/列表可在真站复验。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const LIST_HTML: &str = r#"<!DOCTYPE html><html><body>
<ul class="list-group">
  <li class="list-group-item live-list-group-time">
    <span class="date"></span>2026-11-11
  </li>
  <li class="list-group-item group-game-item">
    <div class="row d-none d-lg-flex justify-content-between align-items-center vertical-center">
      <div class="col-md-1 category-game-time">09:00</div>
      <div class="col-md-3"><span class="game-type">NBA</span></div>
      <div class="col-xs-1 text-right"><img src="/static/img/default.png?t=1" data-src="https://cdn.test/nba.png"></div>
      <div class="col-md-3 pay-btn text-right">
        <a class="btn btn-primary" href="/live/130986/play">直播中</a>
      </div>
    </div>
  </li>
  <li class="list-group-item group-game-item">
    <div class="row d-none d-lg-flex">
      <div class="col-md-1 category-game-time">11:30</div>
      <div class="col-xs-1"><img src="https://cdn.test/lakers.png"></div>
      <div class="col-md-3 pay-btn"><a class="btn btn-default" href="/live/130985/play">已结束</a></div>
    </div>
  </li>
</ul>
</body></html>"#;

const BAD_LINKS: &str = r#"{"links":[]}"#;

fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header")
}

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header")
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
        if url.starts_with("/match/1/live") || url == "/" {
            respond(request, LIST_HTML, html_header());
        } else if url.starts_with("/live/bad/play-url") {
            respond(request, BAD_LINKS, json_header());
        } else if url.starts_with("/live/130986/play-url") || url.starts_with("/live/130985/play-url") {
            let body = r#"{"links":[{"name":"高清1","url":"http://cdn.test/live1.m3u8"},{"name":"","url":"http://cdn.test/live2.m3u8"}]}"#;
            respond(request, body, json_header());
        } else {
            respond(request, "<html>404</html>", html_header());
        }
    });
    server
}

fn base_of(server: &Arc<Server>) -> String {
    format!("http://{}", server.server_addr())
}

fn site(base: &str) -> Site {
    Site {
        key: "kanqiu".to_string(),
        name: "88看球".to_string(),
        api: "csp_Kanqiu".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
fn categories_are_fixed_four_via_dispatch() {
    let cfg = TvConfig::default();
    let site = site("http://127.0.0.1:1");
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("categories");
    assert_eq!(cats.len(), 4);
    assert_eq!(cats[0].type_name, "全部直播");
    assert_eq!(cats[3].type_id, "29");
}

#[test]
fn videos_parse_live_rows_from_mock() {
    let server = start_server();
    let base = base_of(&server);
    let cfg = TvConfig::default();
    let site = site(&base);
    let rows = vbox_core::sources::spider::list_videos(&cfg, &site, "1", 1).expect("videos");
    assert_eq!(rows.len(), 2, "rows = {rows:#?}");
    assert_eq!(rows[0].id, format!("{base}/live/130986/play"));
    assert!(rows[0].name.contains("09:00"));
    assert_eq!(rows[0].remark.as_deref(), Some("直播中"));
    assert_eq!(rows[1].remark.as_deref(), Some("已结束"));
    assert_eq!(rows[1].id, format!("{base}/live/130985/play"));
}

#[test]
fn detail_builds_qile_group_via_dispatch() {
    let server = start_server();
    let base = base_of(&server);
    let cfg = TvConfig::default();
    let site = site(&base);
    let id = format!("{base}/live/130986/play");
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site, &id).expect("detail");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "Qile");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[0].title, "高清1");
    assert_eq!(detail.episodes[0].items[1].title, "第2集");
}

#[test]
fn detail_without_links_reports_parse_error() {
    let server = start_server();
    let base = base_of(&server);
    let cfg = TvConfig::default();
    let site = site(&base);
    let err = vbox_core::sources::spider::get_detail(&cfg, &site, &format!("{base}/live/bad/play"))
        .expect_err("must fail");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "unexpected: {text}");
}

#[test]
fn player_returns_direct_live_url() {
    let cfg = TvConfig::default();
    let site = site("http://127.0.0.1:1");
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site,
        "qile",
        "http://cdn.test/live1.m3u8",
        &[],
    )
    .expect("player");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "http://cdn.test/live1.m3u8");
    assert!(value["header"]["User-Agent"].as_str().unwrap().contains("Chrome"));
}

#[test]
fn search_is_empty_like_original() {
    let cfg = TvConfig::default();
    let site = site("http://127.0.0.1:1");
    let rows = vbox_core::sources::spider::search(&cfg, &site, "湖人", false).expect("search");
    assert!(rows.is_empty());
}

/// 真站验收（列表/分类仍在线；`-url` 详情端点已改版，故不在此断言）。
#[test]
#[ignore]
fn live_site_lists_matches() {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 60_000;
    let site = Site {
        key: "kanqiu".to_string(),
        name: "88看球".to_string(),
        api: "csp_Kanqiu".to_string(),
        ..Default::default()
    };
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("categories");
    assert_eq!(cats.len(), 4);

    let rows = vbox_core::sources::spider::list_videos(&cfg, &site, "1", 1).expect("videos");
    eprintln!("live rows = {}", rows.len());
    for row in rows.iter().take(3) {
        eprintln!("  {:?}", row);
    }
    assert!(!rows.is_empty(), "列表不应为空");
    assert!(rows.iter().all(|row| row.id.starts_with("http")));
}
