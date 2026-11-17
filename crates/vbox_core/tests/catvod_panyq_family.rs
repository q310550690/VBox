//! `csp_Panyq` 家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、分类固定「搜索」项、`GET /api/search?sign=…` 的
//! `{data:{hits:[…]}}` 解析（名称/链接/提取码/封面）、宿主注入 sign 优先、
//! 缺 sign 的 `catvod.host.unsupported` 错误、空关键词短路、空 id 的错误路径。
//!
//! 真实站点：`https://panyq.com/search/{token}` 目前返回 Next.js 错误边界，
//! sign 只能由宿主 WebView 捕获；本测试用 ext `sign` 钩子模拟宿主注入。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::families::{self, panyq};
use vbox_core::spider::catvod::registry;

/// `/api/search` 响应（`data.hits` 形态，与 jar 的兜底解析一致）。
const HITS: &str = r#"{"data":{"hits":[
    {"d":"<b>名称：</b>三体 全集✔<a>https://pan.quark.cn/s/abc123</a> 提取码: 4f2a","group":"quark","desc":"同名电视剧"},
    {"d":"<b>名称：</b>无关资源✔https://pan.quark.cn/s/zzz","group":"quark"},
    {"d":"没有链接"}
]}}"#;

fn respond_json(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes("Content-Type", "application/json").expect("ctype");
    request
        .respond(Response::from_string(body.to_string()).with_header(header))
        .expect("respond json");
}

/// 启动 mock：记录请求路径并统一返回 [`HITS`]。
struct Mock {
    base: String,
    requests: Arc<AtomicUsize>,
    last_url: Arc<std::sync::Mutex<String>>,
}

fn start_mock() -> Mock {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base = format!("http://{}", server.server_addr());
    let requests = Arc::new(AtomicUsize::new(0));
    let last_url = Arc::new(std::sync::Mutex::new(String::new()));
    let counter = Arc::clone(&requests);
    let last = Arc::clone(&last_url);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            counter.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut slot) = last.lock() {
                *slot = request.url().to_string();
            }
            respond_json(request, HITS);
        }
    });
    Mock {
        base,
        requests,
        last_url,
    }
}

fn site_with_ext(ext: serde_json::Value) -> Site {
    site_with_key("panyq", ext)
}

fn site_with_key(key: &str, ext: serde_json::Value) -> Site {
    Site {
        key: key.to_string(),
        name: "盘友圈".into(),
        api: "csp_Panyq".into(),
        kind: 3,
        ext: Some(ext),
        ..Default::default()
    }
}

#[test]
fn dispatch_and_registry_recognise_panyq() {
    assert!(panyq::handles("csp_Panyq"));
    assert!(panyq::handles(" CSP_PANYQ "));
    assert!(!panyq::handles("csp_PushShare"));
    assert!(registry::is_native_ready("csp_Panyq"));
    assert!(registry::blocked_reason("csp_Panyq").is_none());
    assert_eq!(families::family_of("csp_Panyq"), Some("panyq"));
}

#[test]
fn categories_return_single_search_entry() {
    let cfg = TvConfig::default();
    let site = site_with_ext(json!({}));
    let cats = spider::list_categories(&cfg, &site).expect("categories");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "JARSEARCH");
    assert_eq!(cats[0].type_name, "搜索");
}

#[test]
fn videos_are_empty() {
    let cfg = TvConfig::default();
    let site = site_with_ext(json!({}));
    let items = spider::list_videos(&cfg, &site, "JARSEARCH", 1).expect("videos");
    assert!(items.is_empty());
}

#[test]
fn search_parses_hits_with_password_and_cover() {
    let mock = start_mock();
    let cfg = TvConfig::default();
    let site = site_with_key("panyq-parse", json!({"base": mock.base, "sign": "unit-sign"}));

    let items = spider::search(&cfg, &site, "三体", false).expect("search");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].name, "三体 全集");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/abc123?pwd=4f2a");
    assert!(items[0].cover.as_deref().unwrap_or("").contains("bkimg"));
    assert_eq!(items[0].remark.as_deref(), Some("同名电视剧"));

    let url = mock.last_url.lock().expect("url").clone();
    assert!(url.contains("/api/search?sign=unit-sign"), "{url}");
    assert_eq!(mock.requests.load(Ordering::SeqCst), 1);
}

#[test]
fn search_prefers_host_injected_sign() {
    let mock = start_mock();
    let cfg = TvConfig::default();
    let site = site_with_key("panyq-inject", json!({"base": mock.base, "sign": "from-ext"}));
    panyq::put_sign(&site.key, &format!("{}/api/search?sign=from-host", mock.base));

    let items = spider::search(&cfg, &site, "三体", false).expect("search");
    assert_eq!(items.len(), 1);
    let url = mock.last_url.lock().expect("url").clone();
    assert!(url.contains("sign=from-host"), "{url}");
    // 清理注入，避免影响其它用例。
    panyq::put_sign(&site.key, "");
}

#[test]
fn search_without_sign_reports_host_error() {
    let cfg = TvConfig::default();
    let site = site_with_key("panyq-nosign", json!({"base": "http://127.0.0.1:9"}));
    std::env::remove_var("VBOX_PANYQ_SIGN");
    std::env::remove_var("VBOX_PANYQ_SIGN_CMD");
    let err = spider::search(&cfg, &site, "三体", false).unwrap_err();
    assert!(
        err.to_string().contains("catvod.host.unsupported"),
        "{err}"
    );
}

#[test]
fn search_empty_keyword_returns_empty_without_request() {
    let mock = start_mock();
    let cfg = TvConfig::default();
    let site = site_with_key("panyq-empty", json!({"base": mock.base, "sign": "unit-sign"}));
    let items = spider::search(&cfg, &site, "   ", false).expect("search");
    assert!(items.is_empty());
    assert_eq!(mock.requests.load(Ordering::SeqCst), 0);
}

#[test]
fn empty_id_reports_parse_error() {
    let cfg = TvConfig::default();
    let site = site_with_ext(json!({}));
    let err = spider::get_detail(&cfg, &site, "  ").unwrap_err();
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
    let err = spider::player_content(&cfg, &site, "", "  ", &[]).unwrap_err();
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
}
