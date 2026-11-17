//! `csp_NiNi`（泥巴影视）集成验收。
//!
//! 原类 `NiNi extends Spider`，协议是 Nivod `WEB/3.2` 接口：
//! - 请求 URL 带固定参数（`_ts/app_version/platform/market_id/device_code/versioncode/oid`）
//!   与 `sign = md5("__QUERY::…__BODY::…__KEY::2x_Give_it_a_shot")`；
//! - 业务参数走表单 body；响应体是 DES 密文的十六进制串，解密后才是 JSON；
//! - 5 个端点分别覆盖分类/列表/详情/搜索/播放。
//!
//! 上游 `api.nivod.tv` 已停放（返回 SPA HTML），故本文件用 mock 服务器模拟：
//! 服务端调用同款 `des_encrypt_hex` 生成密文响应，顺带断言请求确实带签名参数。

use std::io::Read;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};
use vbox_core::spider::catvod::families::nini::des_encrypt_hex;
use vbox_core::spider::catvod::families::family_of;
use vbox_core::spider::catvod::registry::{blocked_reason, is_native_ready};

const CATEGORIES: &str = r#"{"channels":[{"channelId":"1","channelName":"电影"},{"channelId":"7","channelName":"频道七"}]}"#;
const LIST: &str = r#"{"list":[{"showIdCode":"s1","showTitle":"泥巴剧一","showImg":"http://i/1.jpg","episodesTxt":"更新至12集"},{"showIdCode":"s2","showTitle":"泥巴剧二"}]}"#;
const PAGE2: &str = r#"{"list":[{"showIdCode":"s3","showTitle":"第二页剧"}]}"#;
const DETAIL: &str = r#"{"entity":{"showIdCode":"42","showTitle":"泥巴剧","showImg":"http://i/42.jpg","showDesc":"简介","postYear":"2024","regionName":"大陆","director":"导演","actors":"演员","plays":[{"episodeName":"第1集","playIdCode":"p1"},{"episodeName":"第2集","playIdCode":"p2"}]}}"#;
const SEARCH: &str = r#"{"list":[{"showIdCode":"s9","showTitle":"搜索剧"}]}"#;
const PLAY: &str = r#"{"entity":{"playUrl":"http://cdn.test/v/abc.m3u8"}}"#;
const PLAY_EMPTY: &str = r#"{"entity":{}}"#;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "nini-mock".into(),
        name: "泥巴".into(),
        kind: 3,
        api: "csp_NiNi".into(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

fn respond_cipher(request: tiny_http::Request, plain: &str) {
    let body = des_encrypt_hex(plain);
    let header =
        Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..]).expect("header");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

/// 启动 mock 服务：按路径与表单 body 分发，记录每次请求 URL 供签名断言。
fn start_server(log: Arc<Mutex<Vec<String>>>) -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let runner = server.clone();
    thread::spawn(move || {
        for mut request in runner.incoming_requests() {
            let url = request.url().to_string();
            let mut form = String::new();
            let _ = request.as_reader().read_to_string(&mut form);
            log.lock().expect("log").push(format!("{url} {form}"));
            let plain = if url.starts_with("/show/filter/condition/WEB/3.2") {
                CATEGORIES
            } else if url.starts_with("/show/filter/WEB/3.2") {
                if form.contains("start=20") { PAGE2 } else { LIST }
            } else if url.starts_with("/show/detail/WEB/3.2") {
                DETAIL
            } else if url.starts_with("/show/search/WEB/3.2") {
                SEARCH
            } else if url.starts_with("/show/play/info/WEB/3.2") {
                if form.contains("play_id_code=empty") { PLAY_EMPTY } else { PLAY }
            } else {
                r#"{}"#
            };
            respond_cipher(request, plain);
        }
    });
    base
}

fn setup() -> (String, Site, Arc<Mutex<Vec<String>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let base = start_server(log.clone());
    let site = site(&base);
    (base, site, log)
}

#[test]
fn dispatch_and_registry_recognise_nini() {
    assert_eq!(family_of("csp_NiNi"), Some("nini"));
    assert!(is_native_ready("csp_NiNi"));
    assert_eq!(blocked_reason("csp_NiNi"), None);
}

#[test]
fn categories_and_request_carry_fixed_params_and_sign() {
    let (_, site, log) = setup();
    let cats = list_categories(&cfg(), &site).expect("categories");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[1].type_id, "7");
    assert_eq!(cats[1].type_name, "频道七");
    let urls = log.lock().expect("log");
    let url = urls.last().expect("url");
    assert!(url.contains("_ts="), "{url}");
    assert!(url.contains("market_id=web_nivod"), "{url}");
    assert!(url.contains("oid="), "{url}");
    assert!(url.contains("&sign="), "{url}");
}

#[test]
fn list_videos_parses_rows() {
    let (_, site, _) = setup();
    let items = list_videos(&cfg(), &site, "1", 1).expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "s1");
    assert_eq!(items[0].cover.as_deref(), Some("http://i/1.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("更新至12集"));
    assert_eq!(items[1].remark, None);
}

#[test]
fn list_videos_page_two_uses_start_offset() {
    let (_, site, log) = setup();
    let items = list_videos(&cfg(), &site, "1", 2).expect("videos page2");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "s3");
    assert!(log.lock().expect("log").last().expect("url").contains("start=20"));
}

#[test]
fn detail_builds_episode_urls() {
    let (_, site, _) = setup();
    let detail = get_detail(&cfg(), &site, "42").expect("detail");
    assert_eq!(detail.name, "泥巴剧");
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "泥巴");
    let eps = &detail.episodes[0].items;
    assert_eq!(eps.len(), 2);
    assert_eq!(eps[0].url, "42_p1");
    assert_eq!(eps[1].url, "42_p2");
}

#[test]
fn search_maps_rows() {
    let (_, site, _) = setup();
    let items = search(&cfg(), &site, "泥巴", true).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "搜索剧");
}

#[test]
fn player_content_returns_direct_url_and_headers() {
    let (_, site, _) = setup();
    let value: Value = player_content(&cfg(), &site, "", "42_p1", &[]).expect("player");
    assert_eq!(value["parse"], json!(0));
    assert_eq!(value["url"], json!("http://cdn.test/v/abc.m3u8"));
    assert!(value["header"].as_object().expect("header").get("Referer").is_some());
}

#[test]
fn player_content_reports_parse_error_for_empty_url() {
    let (_, site, _) = setup();
    let err = player_content(&cfg(), &site, "", "42_empty", &[]).unwrap_err();
    assert!(format!("{err}").contains("csp_NiNi"), "{err}");
}

#[test]
fn player_content_rejects_bad_id_without_network() {
    let (_, site, log) = setup();
    let err = player_content(&cfg(), &site, "", "noseparator", &[]).unwrap_err();
    assert!(format!("{err}").contains("csp_NiNi"), "{err}");
    assert!(log.lock().expect("log").is_empty(), "非法 id 不应发请求");
}
