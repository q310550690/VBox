//! `csp_XBiu`（星标 / 自动规则推断引擎）家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、`cateManual` 分类输出、分类列表（块 lookback 抽取 +
//! Base64 id）、分页 URL 拼装、详情（规则切片 + 剧集分组）、播放（`play.vod_url`
//! 直链判定）、搜索（search 规则 lookback）。
//!
//! 上游现状：8 站全部不可达（见附录 FA 探测表），因此只有 mock 验收，
//! 无真站 `#[ignore]` 用例。

use std::sync::{Arc, Mutex};
use std::thread;

use base64::engine::general_purpose::URL_SAFE;
use base64::Engine as _;
use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

fn respond(request: tiny_http::Request, body: &str, ctype: &str) {
    let header = Header::from_bytes("Content-Type", ctype).expect("ctype");
    request
        .respond(Response::from_string(body.to_string()).with_header(header))
        .expect("respond");
}

fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Server::http("127.0.0.1:0").expect("server");
    let root = format!("http://{}", server.server_addr());
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = Arc::clone(&log);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            log2.lock().expect("log").push(url.clone());
            if url.starts_with("/list/") {
                let body = r#"<div id="main">
  <div class="row"><a href="/v/100"><img data-src="/p/100.jpg"><span class="title">沙丘2</span><span class="note">HD</span></a></div>
  <div class="row"><a href="/v/101"><img data-src="/p/101.jpg"><span class="title">沙丘</span><span class="note">BD</span></a></div>
</div>"#;
                respond(request, body, "text/html; charset=utf-8");
            } else if url.starts_with("/v/") {
                let body = r#"<div id="detail"><h1>沙丘2</h1><img src="/p/100.jpg"><p>科幻史诗 year:2024-</p>
  <div class="ep"><a href="/play/100-1.html">1</a></div>
  <div class="ep"><a href="/play/100-2.html">2</a></div>
</div>"#;
                respond(request, body, "text/html; charset=utf-8");
            } else if url.starts_with("/play/") {
                let body = if url.contains("100-3") {
                    r#"<div id="playbox"><a href="https://example.com/page.html">播放</a></div>"#
                } else {
                    r#"<div id="playbox"><a href="m3u8:http://cdn.test/stream/100-1.m3u8">播放</a></div>"#
                };
                respond(request, body, "text/html; charset=utf-8");
            } else if url.starts_with("/search") {
                let body = r#"<div class="row"><a href="/v/200"><img data-src="/p/200.jpg"><span class="title">搜索结果</span><span class="note">HD</span></a></div>"#;
                respond(request, body, "text/html; charset=utf-8");
            } else {
                respond(
                    request,
                    r#"<html><body><div id="nav"><ul><li><a href="/list/1---1---.html">电影</a></li></ul></div></body></html>"#,
                    "text/html; charset=utf-8",
                );
            }
        }
    });
    (root, log)
}

fn site(root: &str) -> Site {
    let rule = json!({
        "homeUrl": format!("{root}/"),
        "cateManual": { "电影": "1", "电视剧": "2", "综艺": "3" },
        "header": { "User-Agent": "XBiu-Test" },
        "list": {
            "url": format!("{root}/list/{{cateId}}---{{catePg}}---.html"),
            "lb": ["href=\"/v/", 0, 0, 0, 2],
            "vod_id": ["href=\"/v/", "\""],
            "vod_name": ["class=\"title\">", "<"],
            "vod_pic": ["data-src=\"", "\""],
            "vod_remarks": ["class=\"note\">", "<"]
        },
        "detail": {
            "url": format!("{root}/v/{{vid}}.html"),
            "vod_name": ["<h1>", "<"],
            "vod_pic": ["<img src=\"", "\""],
            "vod_content": ["<p>", " year:"],
            "vod_year": ["year:", "-"]
        },
        "playlist": {
            "lb": ["class=\"ep\"", 0, 0, 0, 1],
            "vod_play_url": ["href=\"", "\""],
            "vod_play_from": ["线路一", "线路二"]
        },
        "play": { "vod_url": ["href=\"m3u8:", "\""] },
        "search": {
            "url": format!("{root}/search?wd={{wd}}"),
            "lb": ["href=\"/v/", 0, 0, 0, 2],
            "vod_id": ["href=\"/v/", "\""],
            "vod_name": ["class=\"title\">", "<"],
            "vod_pic": ["data-src=\"", "\""],
            "vod_remarks": ["class=\"note\">", "<"]
        }
    });
    Site {
        key: "xbiu-mock".to_string(),
        name: "星标".into(),
        api: "csp_XBiu".into(),
        kind: 3,
        searchable: true,
        ext: Some(json!(rule.to_string())),
        ..Site::default()
    }
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn decode_id(id: &str) -> serde_json::Value {
    let mut text = id.to_string();
    while text.len() % 4 != 0 {
        text.push('=');
    }
    let bytes = URL_SAFE.decode(text.as_bytes()).expect("base64 id");
    serde_json::from_slice(&bytes).expect("json id")
}

#[test]
fn dispatch_and_registry_recognise_xbiu() {
    let api = "csp_XBiu";
    assert_eq!(
        vbox_core::spider::catvod::families::family_of(api),
        Some("xbiu")
    );
    assert!(vbox_core::spider::catvod::registry::is_native_ready(api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(api).is_none());
}

#[test]
fn categories_follow_cate_manual() {
    let (root, _log) = start_server();
    let cats = spider::list_categories(&cfg(), &site(&root)).expect("categories");
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(names, vec!["电影", "电视剧", "综艺"]);
    assert_eq!(cats[0].type_id, "1");
}

#[test]
fn videos_parse_rows_and_encode_ids() {
    let (root, _log) = start_server();
    let items = spider::list_videos(&cfg(), &site(&root), "1", 1).expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "沙丘2");
    assert_eq!(items[0].remark.as_deref(), Some("HD"));
    assert_eq!(items[0].cover.as_deref(), Some(format!("{root}/p/100.jpg").as_str()));
    assert_eq!(decode_id(&items[0].id)["vod_id"], "100");
}

#[test]
fn videos_use_page_placeholder() {
    let (root, log) = start_server();
    let _ = spider::list_videos(&cfg(), &site(&root), "1", 2).expect("videos p2");
    let urls = log.lock().expect("log").clone();
    assert!(
        urls.iter().any(|u| u.contains("/list/1---2---.html")),
        "urls = {urls:?}"
    );
}

#[test]
fn detail_extracts_fields_and_episode_groups() {
    let (root, _log) = start_server();
    let id = {
        let v = json!({"vod_id": "100"});
        let raw = serde_json::to_vec(&v).expect("json");
        URL_SAFE.encode(raw)
    };
    let detail = spider::get_detail(&cfg(), &site(&root), &id).expect("detail");
    assert_eq!(detail.name, "沙丘2");
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert!(detail.episodes[0].items[0].url.contains("/play/100-1.html"));
}

#[test]
fn search_parses_rows() {
    let (root, log) = start_server();
    let items = spider::search(&cfg(), &site(&root), "沙丘", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "搜索结果");
    let urls = log.lock().expect("log").clone();
    assert!(
        urls.iter().any(|u| u.contains("/search?wd=%E6%B2%99%E4%B8%98")),
        "urls = {urls:?}"
    );
}

#[test]
fn player_reads_vod_url_and_marks_direct() {
    let (root, _log) = start_server();
    let play = spider::player_content(&cfg(), &site(&root), "线路一", &format!("{root}/play/100-1.html"), &[])
        .expect("play");
    assert_eq!(play["parse"], 0);
    assert_eq!(play["url"], "http://cdn.test/stream/100-1.m3u8");
}

#[test]
fn player_falls_back_when_not_video() {
    let (root, _log) = start_server();
    let target = format!("{root}/play/100-3.html");
    let play = spider::player_content(&cfg(), &site(&root), "线路一", &target, &[]).expect("play");
    assert_eq!(play["parse"], 1);
    assert_eq!(play["url"], target);
}
