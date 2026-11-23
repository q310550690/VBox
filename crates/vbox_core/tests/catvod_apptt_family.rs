//! `csp_AppTT` 家族集成测试（tiny_http mock）。
//!
//! 覆盖：分类/列表/搜索/详情（线路分组 + 解析器前缀）/播放（直链与 AES-256-CTR 解密）/
//! 错误路径；并校验 `zh(url)` 的 UA 选择在真实分派链上生效。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const CTR_VECTOR: &str = "T0TrN7c1L8sy8V4ZpeCMtzIh6tGla/n857yfeBEuIB0UsLJG"; // {"url":"http://cdn.test/final.m3u8"}
const CTR_NO_URL: &str = "T0T4KrQ1L9gn"; // {"foo":1}
const CTR_EMPTY: &str = "Txs="; // {}

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header")
}

fn respond(request: tiny_http::Request, body: &str) {
    let response = Response::from_string(body).with_header(json_header());
    let _ = request.respond(response);
}

fn start_server() -> (Arc<Server>, String) {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let addr = format!("http://{}", server.server_addr());
    let handle = Arc::clone(&server);
    thread::spawn(move || loop {
        let Ok(mut request) = handle.recv() else { return };
        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);
        let url = request.url().to_string();
        let ua = request
            .headers()
            .iter()
            .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("user-agent"))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default();

        // UA 选择与 url 形态一致（zh() 语义）。
        let want_ua = if url.contains(".vod") {
            "okhttp/4.1.0"
        } else {
            "Dalvik/2.1.0"
        };
        if ua != want_ua {
            respond(request, &json!({ "error": format!("bad ua {ua}") }).to_string());
            continue;
        }

        if url.starts_with("/api.php/v1.vod/detilldata") {
            respond(
                request,
                &json!({
                    "data": {
                        "videoInfo": {
                            "vod_id": "42", "vod_name": "示例剧", "vod_pic": "http://p.example/cover.jpg",
                            "vod_year": "2024", "vod_area": "大陆", "vod_actor": "演员甲",
                            "vod_director": "导演乙", "vod_content": "  一段简介  "
                        },
                        "source": [
                            { "player_info": { "show": "线路A", "parse": "" },
                              "list": ["第1集$http://cdn.example/1.m3u8", "第2集$http://cdn.example/2.mp4"] },
                            { "player_info": { "show": "线路B", "parse": "http://jx.example/?url=" },
                              "list": ["第1集$abc"] }
                        ]
                    }
                })
                .to_string(),
            );
        } else if url.contains("wd=") {
            respond(
                request,
                &json!({ "data": { "list": [
                    { "vod_id": "7", "vod_name": "搜索结果", "vod_pic": "http://p.example/s.jpg", "vod_remarks": "HD" }
                ]}})
                .to_string(),
            );
        } else if url.contains("type=") {
            respond(
                request,
                &json!({ "data": { "page": 1, "total": 2, "list": [
                    { "vod_id": 12, "vod_name": "第一条", "vod_pic": "http://p.example/1.jpg", "vod_remarks": "更新至3集" },
                    { "vod_id": "13", "vod_name": "第二条", "vod_pic": "http://p.example/2.jpg", "vod_remarks": "" }
                ]}})
                .to_string(),
            );
        } else if url.starts_with("/enc/play") {
            if !url.contains("&sc=1&token=") {
                respond(request, r#"{"error":"missing token"}"#);
            } else {
                respond(request, CTR_VECTOR);
            }
        } else if url.starts_with("/enc/nourl") {
            respond(request, CTR_NO_URL);
        } else if url.starts_with("/enc/empty") {
            respond(request, CTR_EMPTY);
        } else if url.starts_with("/enc/bad") {
            respond(request, "not-base64!!");
        } else {
            respond(request, r#"{"error":"unknown"}"#);
        }
    });
    (server, addr)
}

fn site(base: &str) -> Site {
    Site {
        key: "apptt".to_string(),
        name: "AppTT 测试站".to_string(),
        kind: 3,
        api: "csp_AppTT".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_are_four_static_entries() {
    let (_server, base) = start_server();
    let cfg = TvConfig::default();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    assert_eq!(cats.len(), 4);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[1].type_name, "连续剧");
    assert_eq!(cats[3].type_name, "综艺");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn list_videos_via_dispatch() {
    let (_server, base) = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "1", 1).expect("videos");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "12");
    assert_eq!(rows[0].name, "第一条");
    assert_eq!(rows[0].remark.as_deref(), Some("更新至3集"));
    assert_eq!(rows[1].id, "13");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_via_dispatch() {
    let (_server, base) = start_server();
    let cfg = TvConfig::default();
    let rows = vbox_core::sources::spider::search(&cfg, &site(&base), "hello", false).expect("search");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "7");
    assert_eq!(rows[0].name, "搜索结果");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_maps_meta_and_groups() {
    let (_server, base) = start_server();
    let cfg = TvConfig::default();
    let detail =
        vbox_core::sources::spider::get_detail(&cfg, &site(&base), "42").expect("detail");
    assert_eq!(detail.name, "示例剧");
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.actor.as_deref(), Some("演员甲"));
    assert_eq!(detail.director.as_deref(), Some("导演乙"));
    assert_eq!(detail.desc.as_deref(), Some("一段简介"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路A");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[0].url, "http://cdn.example/1.m3u8");
    assert_eq!(detail.episodes[1].group, "线路B");
    // 非直链：拼上 player_info.parse 前缀
    assert_eq!(detail.episodes[1].items[0].url, "http://jx.example/?url=abc");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_returns_direct_url_unchanged() {
    let (_server, base) = start_server();
    let cfg = TvConfig::default();
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "线路A",
        "http://cdn.example/1.m3u8",
        &[],
    )
    .expect("player");
    assert_eq!(value.get("parse"), Some(&json!(0)));
    assert_eq!(value.get("url"), Some(&json!("http://cdn.example/1.m3u8")));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_decrypts_aes_ctr_response() {
    let (_server, base) = start_server();
    let cfg = TvConfig::default();
    let id = format!("{base}/enc/play?x=1");
    let value = vbox_core::sources::spider::player_content(&cfg, &site(&base), "", &id, &[])
        .expect("player decrypt");
    assert_eq!(value.get("url"), Some(&json!("http://cdn.test/final.m3u8")));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_reports_host_api_when_url_missing() {
    let (_server, base) = start_server();
    let cfg = TvConfig::default();
    let id = format!("{base}/enc/nourl?x=1");
    let err = vbox_core::sources::spider::player_content(&cfg, &site(&base), "", &id, &[])
        .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.host.unsupported"), "{err:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_reports_parse_error_on_bad_ciphertext() {
    let (_server, base) = start_server();
    let cfg = TvConfig::default();
    let id = format!("{base}/enc/bad?x=1");
    let err = vbox_core::sources::spider::player_content(&cfg, &site(&base), "", &id, &[])
        .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.result.parse"), "{err:?}");
}
