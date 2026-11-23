//! `csp_YingSo`（鹰搜）集成验收：`extends Push` 的网盘搜索型家族。
//!
//! 站点 `https://yingso.fun` 在 2026-11 已不可达（fetch failed），因此这里用
//! tiny_http mock 走完整分派链路：`search` → POST `{base}/v2/ali/search` →
//! 解析 `data[]` → 三个平台（阿里/夸克/迅雷）链接。mock 服务端会校验请求体，
//! 缺字段直接回 400，从而把「请求编码」也纳入离线验收。

use std::io::Read;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, player_content, search};

const SEARCH_BODY: &str = r#"{"code":0,"data":[
 {"root":1,"cat":"dy","key":"ali123","title":"阿里资源","time":1700000000},
 {"root":2,"cat":"ds","key":"quark456","title":"夸克资源","time":1700000000},
 {"root":3,"cat":"qt","key":"xl789","title":"迅雷资源","time":1700000000},
 {"root":4,"cat":"dy","key":"pk000","title":"PikPak 分支（原类无 root=4）","time":1700000000}
]}"#;

fn respond_json(request: tiny_http::Request, body: String, status: u16) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header");
    let resp = Response::from_string(body)
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(resp);
}

fn start_server(body_ok: bool) -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || loop {
        let Ok(mut request) = server.recv() else {
            return;
        };
        let url = request.url().to_string();
        if !body_ok {
            respond_json(request, "<html>not json</html>".to_string(), 200);
            continue;
        }
        if url == "/v2/ali/search" && request.method() == &tiny_http::Method::Post {
            let mut body = String::new();
            let _ = request
                .as_reader()
                .take(64 * 1024)
                .read_to_string(&mut body);
            let encoded_ok = body.contains("\"pageSize\":30")
                && body.contains("\"pageNum\":1")
                && body.contains("\"root\":0")
                && body.contains("\"cat\":\"all\"")
                && body.contains("\"title\":\"");
            if !encoded_ok {
                respond_json(
                    request,
                    format!("{{\"error\":\"bad body: {body}\"}}"),
                    400,
                );
                continue;
            }
            respond_json(request, SEARCH_BODY.to_string(), 200);
        } else {
            respond_json(request, "{\"error\":\"not found\"}".to_string(), 404);
        }
    });
    base
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "yingso-mock".to_string(),
        name: "鹰搜".to_string(),
        kind: 3,
        api: "csp_YingSo".to_string(),
        ext: Some(json!(format!("./lib/token.json$$${base}$$$null$$$1"))),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn dispatch_routes_yingso_to_native_family() {
    assert!(vbox_core::spider::catvod::is_native_ready("csp_YingSo"));
    assert!(vbox_core::spider::catvod::blocked_reason("csp_YingSo").is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_are_empty_for_search_only_family() {
    let base = start_server(true);
    let cats = list_categories(&cfg(), &site(&base)).unwrap_or_default();
    assert!(cats.is_empty(), "cats={cats:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_posts_expected_body_and_maps_three_providers() {
    let base = start_server(true);
    let hits = search(&cfg(), &site(&base), "关键字", false).expect("search");
    assert_eq!(hits.len(), 3, "hits={hits:?}");
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids[0], "https://www.alipan.com/s/ali123");
    assert_eq!(ids[1], "https://pan.quark.cn/s/quark456");
    assert_eq!(ids[2], "https://pan.xunlei.com/s/xl789");
    let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, vec!["阿里资源", "夸克资源", "迅雷资源"]);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_remark_carries_keyword_date_and_category() {
    let base = start_server(true);
    let hits = search(&cfg(), &site(&base), "关键字", false).expect("search");
    assert_eq!(
        hits[0].remark.as_deref(),
        Some("关键字/2023-11-14/电影"),
        "hits={hits:?}"
    );
    assert_eq!(hits[1].remark.as_deref(), Some("关键字/2023-11-14/电视剧"));
    assert_eq!(hits[2].remark.as_deref(), Some("关键字/2023-11-14/综艺"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_sends_keyword_in_body() {
    let base = start_server(true);
    // mock 服务端会校验 body 含 title 字段；换关键词再跑一遍，确保关键字真的进了 body。
    let hits = search(&cfg(), &site(&base), "第二个关键词", false).expect("search");
    assert_eq!(hits.len(), 3);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_rejects_non_json_response() {
    let base = start_server(false);
    let err = search(&cfg(), &site(&base), "关键字", false).expect_err("not json");
    let text = err.to_string();
    assert!(text.contains("不是 JSON") || text.contains("data"), "err={text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_reports_host_unsupported() {
    let err = get_detail(&cfg(), &site(""), "https://pan.quark.cn/s/ali123")
        .expect_err("push detail needs android parser");
    let text = err.to_string();
    assert!(text.contains("catvod.host.unsupported"), "err={text}");
    assert!(text.contains("Android parser"), "err={text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_reports_host_unsupported() {
    let err = player_content(
        &cfg(),
        &site(""),
        "阿里",
        "https://www.alipan.com/s/ali123",
        &[],
    )
    .expect_err("push player needs android parser");
    let text = err.to_string();
    assert!(text.contains("catvod.host.unsupported"), "err={text}");
}
