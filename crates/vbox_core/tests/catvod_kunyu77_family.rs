//! `csp_Kunyu77` 家族集成测试（mock 服务，无真站）。

use std::sync::Arc;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

fn start_server(payloads: Arc<std::sync::Mutex<Vec<String>>>) -> (String, std::thread::JoinHandle<()>) {
    let server = Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let base_for_thread = base.clone();
    let handle = std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            let body = if url.contains("/searchFilter") {
                payloads.lock().unwrap().push(header_of(&request, "TK"));
                json!({
                    "code": 0,
                    "data": {
                        "page": 1, "total": 48, "pagesize": 24,
                        "result": [
                            {"id": 1, "title": "杀或救", "videoCover": "https://img/1.webp", "msg": "蓝光"},
                            {"id": 2, "title": "第二部", "videoCover": "https://img/2.webp", "msg": "完结"}
                        ]
                    }
                })
                .to_string()
            } else if url.contains("/videoDetail") {
                json!({
                    "code": 0,
                    "data": {
                        "videoName": "杀或救", "year": "2026", "videoCover": "https://img/1.webp",
                        "area": "中国", "actor": "张三", "msg": "蓝光", "director": "李四",
                        "brief": "<p>简介正文</p>", "subCategory": "动作"
                    }
                })
                .to_string()
            } else if url.contains("/videoPlaylist") {
                json!({
                    "code": 0,
                    "data": {"episodes": [
                        {"playurls": [
                            {"playfrom": "line1", "title": "杀或救第01集", "playurl": "https://cdn/1.m3u8"},
                            {"playfrom": "line1", "title": "杀或救第02集", "playurl": "https://cdn/2.m3u8"},
                            {"playfrom": "line2", "title": "第01集", "playurl": "https://cdn/3.m3u8"}
                        ]}
                    ]}
                })
                .to_string()
            } else if url.contains("/searchVideo") {
                json!({
                    "code": 0,
                    "data": [
                        {"id": 9, "videoName": "海贼王", "videoCover": "https://img/h.webp", "msg": "1180集"},
                        {"id": 10, "videoName": "无关", "videoCover": "", "msg": ""}
                    ]
                })
                .to_string()
            } else if url.contains("/parserUrl") {
                json!({"code": 0, "url": format!("{}/play/redirect", base_for_thread)}).to_string()
            } else if url.contains("/play/redirect") {
                json!({"url": "https://cdn/parsed.m3u8"}).to_string()
            } else {
                "{}".to_string()
            };
            let response = Response::from_string(body)
                .with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
            let _ = request.respond(response);
        }
    });
    (base, handle)
}

fn header_of(request: &tiny_http::Request, name: &str) -> String {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
}

fn site(base: &str) -> Site {
    Site {
        key: "kunyu77".to_string(),
        name: "酷云七七".to_string(),
        api: "csp_Kunyu77".to_string(),
        ext: Some(json!(base)),
        searchable: true,
        ..Default::default()
    }
}

#[test]
fn lists_videos_and_sends_signed_headers() {
    let payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (base, _h) = start_server(payloads.clone());
    let cfg = TvConfig::default();
    let site = site(&base);
    let items = vbox_core::sources::spider::list_videos(&cfg, &site, "1", 1).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "1");
    assert_eq!(items[0].name, "杀或救");
    assert_eq!(items[0].remark.as_deref(), Some("蓝光"));
    let tk = payloads.lock().unwrap().first().cloned().unwrap_or_default();
    assert_eq!(tk.len(), 32, "TK header should be md5 hex: {tk}");
    assert!(tk.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn detail_builds_groups_and_metadata() {
    let payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (base, _h) = start_server(payloads);
    let cfg = TvConfig::default();
    let site = site(&base);
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site, "1").unwrap();
    assert_eq!(detail.name, "杀或救");
    assert_eq!(detail.year.as_deref(), Some("2026"));
    assert_eq!(detail.actor.as_deref(), Some("张三"));
    assert_eq!(detail.director.as_deref(), Some("李四"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "line1");
    assert_eq!(detail.episodes[0].items[0].title, "第01集");
}

#[test]
fn search_filters_by_keyword() {
    let payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (base, _h) = start_server(payloads);
    let cfg = TvConfig::default();
    let site = site(&base);
    let items = vbox_core::sources::spider::search(&cfg, &site, "海贼", false).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "海贼王");
}

#[test]
fn player_resolves_known_site_url() {
    let payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (base, _h) = start_server(payloads);
    let cfg = TvConfig::default();
    let site = site(&base);
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site,
        "line1",
        "https://v.qq.com/x/cover/abc.html",
        &[],
    )
    .unwrap();
    assert_eq!(value.get("parse").and_then(Value::as_i64), Some(0));
    assert_eq!(
        value.get("url").and_then(Value::as_str),
        Some("https://cdn/parsed.m3u8")
    );
}

#[test]
fn player_passes_through_direct_links() {
    let payloads = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (base, _h) = start_server(payloads);
    let cfg = TvConfig::default();
    let site = site(&base);
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site,
        "line1",
        "https://cdn.test/1.m3u8",
        &[],
    )
    .unwrap();
    assert_eq!(
        value.get("url").and_then(Value::as_str),
        Some("https://cdn.test/1.m3u8")
    );
}

#[test]
fn garbage_upstream_returns_stable_catvod_error() {
    let server = Server::http("127.0.0.1:0").expect("mock");
    let base = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let _ = req.respond(
                Response::from_string("<html><body>garbage, not kunyu77 api</body></html>")
                    .with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap()),
            );
        }
    });
    let cfg = TvConfig::default();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "12345")
        .expect_err("垃圾响应应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod."), "错误缺少稳定码: {msg}");
}
