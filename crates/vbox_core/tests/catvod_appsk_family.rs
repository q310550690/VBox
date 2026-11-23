//! `csp_AppSK`（小柚 App）集成验收。
//!
//! 上游 `https://yz.yzys.top:9777` 已不可达，故用 tiny_http mock 走完整分派链路：
//! type/list / vod/list（分类 + 首页 aqy）/ vod/one / search/pages / vod/skjson
//! 五类端点；并验证 `authorization: Bearer <token>` 由 ext.token 注入。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};
use vbox_core::spider::catvod::families::appsk;

fn respond(request: tiny_http::Request, body: &str, status: u16) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header");
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}

fn has_bearer(request: &tiny_http::Request) -> bool {
    request.headers().iter().any(|h| {
        h.field.equiv("authorization") && h.value.as_str().trim() == "Bearer t-123"
    })
}

fn start_server() -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || loop {
        let Ok(request) = server.recv() else {
            return;
        };
        let url = request.url().to_string();
        if url.contains("/sk-api/type/list") {
            respond(
                request,
                r#"{"data":[{"type_id":"1","type_name":"电影"},{"type_id":2,"type_name":"电视剧"}]}"#,
                200,
            );
        } else if url.contains("/sk-api/vod/list") && url.contains("type=aqy") {
            respond(
                request,
                r#"{"data":[{"vod_id":9,"vod_name":"首页一","vod_pic":"/img/h.jpg","vod_remarks":"HD"}]}"#,
                200,
            );
        } else if url.contains("/sk-api/vod/list") {
            respond(
                request,
                r#"{"data":[
                    {"vod_id":11,"vod_name":"列一","vod_pic":"/img/a.jpg","vod_remarks":"更新至 3 集"},
                    {"vod_id":"12","vod_name":"列二","vod_pic":"","vod_remarks":"","vod_douban_score":"8.1"}
                ]}"#,
                200,
            );
        } else if url.contains("/sk-api/vod/one") {
            // 只有带正确 Bearer 头的请求才返回详情，用于验证头注入。
            if !has_bearer(&request) {
                respond(request, r#"{"error":"unauthorized"}"#, 401);
                continue;
            }
            respond(
                request,
                r#"{"data":{
                    "vod_name":"示例剧","vod_pic":"https://cdn/d.jpg","vod_class":"动作",
                    "vod_year":"2024","vod_area":"大陆","vod_actor":"演员甲","vod_director":"导演乙",
                    "vod_content":"<p>剧情&nbsp;简介</p>",
                    "vod_play_from":"线路一$$$线路二",
                    "vod_play_url":"第1集$https://v/1.m3u8#第2集$https://v/2.m3u8$$$正片$https://v/full.mp4"
                }}"#,
                200,
            );
        } else if url.contains("/sk-api/search/pages") {
            respond(
                request,
                r#"{"data":[{"vod_id":21,"vod_name":"搜一","vod_pic":"https://cdn/s.jpg","vod_remarks":"全 1 集"}]}"#,
                200,
            );
        } else if url.contains("/sk-api/vod/skjson") {
            respond(request, r#"{"data":{"url":"https://cdn/real.m3u8"}}"#, 200);
        } else {
            respond(request, "{\"error\":\"not found\"}", 404);
        }
    });
    base
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "appsk".to_string(),
        name: "小柚".to_string(),
        api: "csp_AppSK".to_string(),
        kind: 3,
        ext: Some(json!({ "host": format!("{base}/sk/"), "token": "t-123" })),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_routes_appsk_to_native_family() {
    assert!(appsk::handles("csp_AppSK"));
    assert!(appsk::handles(" CSP_APPSK "));
    assert_eq!(
        vbox_core::spider::catvod::families::family_of("csp_AppSK"),
        Some("appsk")
    );
    let base = start_server();
    let s = site(&base);
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&s.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&s.api).is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_through_dispatch() {
    let base = start_server();
    let cats = list_categories(&cfg(), &site(&base)).expect("categories");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[1].type_id, "2");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_normalize_cover_and_fallback_remark() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "1", 2).expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "11");
    assert_eq!(items[0].cover.as_deref(), Some(format!("{base}/sk/img/a.jpg").as_str()));
    assert_eq!(items[0].remark.as_deref(), Some("更新至 3 集"));
    assert_eq!(items[1].remark.as_deref(), Some("8.1"));
    assert!(items[1].cover.is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn home_videos_direct_call() {
    let base = start_server();
    let items = appsk::home_videos(&cfg(), &site(&base)).expect("home");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "首页一");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_uses_server_play_lists_and_sends_bearer() {
    let base = start_server();
    let detail = get_detail(&cfg(), &site(&base), "42").expect("detail");
    assert_eq!(detail.name, "示例剧");
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].title, "第2集");
    assert_eq!(detail.episodes[1].items[0].url, "https://v/full.mp4");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_through_dispatch() {
    let base = start_server();
    let items = search(&cfg(), &site(&base), "关键词", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "21");
    assert_eq!(items[0].name, "搜一");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_resolves_skjson_url() {
    let base = start_server();
    let value = player_content(&cfg(), &site(&base), "线路一", "https://page.example.com/play/1", &[])
        .expect("player");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn/real.m3u8");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_passes_through_direct_video_without_network() {
    // 直链分支不需要 mock：即便 host 不可达也应直接返回。
    let s = Site {
        key: "appsk".to_string(),
        name: "小柚".to_string(),
        api: "csp_AppSK".to_string(),
        kind: 3,
        ext: Some(json!({ "host": "http://127.0.0.1:1/dead" })),
        ..Default::default()
    };
    let value = player_content(&cfg(), &s, "", "https://cdn/x.m3u8", &[]).expect("player");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn/x.m3u8");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_rejects_blank_id() {
    let base = start_server();
    let err = get_detail(&cfg(), &site(&base), "  ").expect_err("blank id");
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
}
