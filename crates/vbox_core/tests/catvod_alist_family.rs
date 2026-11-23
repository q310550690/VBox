//! `csp_Alist` 家族集成测试（tiny_http mock + 真站 `#[ignore]`）。
//!
//! 覆盖：平铺 ext → 分类、根目录目录/文件排序、嵌套路径、详情集、播放直链、
//! 搜索、`Guest user is disabled` / HTTP 401 的登录重试、非法 id 报错。
//!
//! 真站用例默认 `#[ignore]`：dc 仓配置的 19 台 AList 服务器多数已失效，
//! 只对公开实例 `al.chirmyram.com` 做冒烟（其 `/api/fs/list` 允许匿名调用）。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server, StatusCode};

use vbox_core::model::{Site, TvConfig};

const LIST_ROOT: &str = r#"{"code":200,"message":"success","data":{"content":[
{"name":"Movies","is_dir":true,"type":1,"size":0},
{"name":"a.mp4","is_dir":false,"type":0,"size":5242880,"thumb":"https://cdn.test/thumb.jpg"},
{"name":"b.mkv","is_dir":false,"type":0,"size":1024}
],"total":3}}"#;

const LIST_MOVIES: &str = r#"{"code":200,"message":"success","data":{"content":[
{"name":"Sub","is_dir":true,"type":1,"size":0},
{"name":"cover.jpg","is_dir":false,"type":0,"size":10},
{"name":"x.mp4","is_dir":false,"type":0,"size":2048,"thumb":"https://cdn.test/x.jpg"},
{"name":"y.mkv","is_dir":false,"type":0,"size":4096}
],"total":4}}"#;

const GET_JSON: &str = r#"{"code":200,"message":"success","data":{"name":"x.mp4","size":2048,"raw_url":"https://cdn.test/video.mp4"}}"#;

const SEARCH_JSON: &str = r#"{"code":200,"message":"success","data":{"content":[
{"name":"x.mp4","is_dir":false,"type":0,"size":2048,"path":"/Movies","thumb":"https://cdn.test/t.jpg"},
{"name":"cover.jpg","is_dir":false,"type":0,"size":10,"path":"/Movies"},
{"name":"Sub","is_dir":true,"type":1,"path":"/"}
]}}"#;

const EMPTY_LIST: &str =
    r#"{"code":200,"message":"success","data":{"content":[],"total":0}}"#;

const LOGIN_JSON: &str = r#"{"code":200,"message":"success","data":{"token":"tok-123"}}"#;
const GUEST_JSON: &str = r#"{"code":401,"message":"Guest user is disabled"}"#;

#[derive(Default)]
struct State {
    /// 模拟需要登录的服务器。
    require_login: bool,
    /// 已发出的 `/api/fs/list` 请求的 Authorization 头。
    list_auth: Vec<Option<String>>,
    /// `/api/auth/login` 的调用次数。
    login_calls: usize,
}

fn respond_json(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
        .expect("header");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

fn respond_guest_disabled(request: tiny_http::Request) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
        .expect("header");
    let _ = request.respond(
        Response::from_string(GUEST_JSON)
            .with_status_code(StatusCode(401))
            .with_header(header),
    );
}

fn start_server(require_login: bool) -> (String, Arc<Mutex<State>>) {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let state = Arc::new(Mutex::new(State {
        require_login,
        ..Default::default()
    }));
    let handle = Arc::clone(&server);
    let state_handle = Arc::clone(&state);
    thread::spawn(move || loop {
        let Ok(mut request) = handle.recv() else {
            return;
        };
        let url = request.url().to_string();
        let auth = request
            .headers()
            .iter()
            .find(|h| format!("{}", h.field).eq_ignore_ascii_case("authorization"))
            .map(|h| h.value.as_str().to_string());
        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);
        let path = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("path")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        if url.starts_with("/api/fs/list") {
            let mut guard = state_handle.lock().unwrap();
            guard.list_auth.push(auth.clone());
            let need_login = guard.require_login && auth.is_none();
            drop(guard);
            if need_login {
                respond_guest_disabled(request);
                continue;
            }
            if path == "/Empty" {
                respond_json(request, EMPTY_LIST);
            } else if path.starts_with("/Movies") {
                respond_json(request, LIST_MOVIES);
            } else {
                respond_json(request, LIST_ROOT);
            }
        } else if url.starts_with("/api/auth/login") {
            state_handle.lock().unwrap().login_calls += 1;
            respond_json(request, LOGIN_JSON);
        } else if url.starts_with("/api/fs/get") {
            respond_json(request, GET_JSON);
        } else if url.starts_with("/api/fs/search") {
            respond_json(request, SEARCH_JSON);
        } else {
            respond_json(request, "{}");
        }
    });
    (base, state)
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "alist".into(),
        name: "AList".into(),
        api: "csp_Alist".into(),
        ..Default::default()
    };
    s.ext = Some(json!({ "Test": base }));
    s
}

fn login_site(base: &str) -> Site {
    let mut s = Site {
        key: "alist-login".into(),
        name: "AList 登录".into(),
        api: "csp_Alist".into(),
        ..Default::default()
    };
    s.ext = Some(json!({
        "drives": [{
            "name": "Test",
            "server": base,
            "login": { "username": "user", "password": "pass" }
        }]
    }));
    s
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_come_from_flat_ext_map() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "Test");
    assert_eq!(cats[0].type_name, "Test");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_list_dirs_before_files_and_keep_sizes() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "Test", 1).expect("list");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].id, "Test/Movies");
    assert_eq!(items[0].remark.as_deref(), Some("文件夹"));
    assert_eq!(items[1].id, "Test/a.mp4");
    assert_eq!(items[1].name, "a.mp4");
    assert_eq!(items[1].cover.as_deref(), Some("https://cdn.test/thumb.jpg"));
    assert_eq!(items[1].remark.as_deref(), Some("5.0 MB"));
    assert_eq!(items[2].id, "Test/b.mkv");
    assert_eq!(items[2].remark.as_deref(), Some("1.0 KB"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_nested_category_uses_alist_path() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "Test/Movies", 1).expect("list");
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].id, "Test/Movies/Sub");
    assert_eq!(items[0].remark.as_deref(), Some("文件夹"));
    assert_eq!(items[1].id, "Test/Movies/cover.jpg");
    assert_eq!(items[2].id, "Test/Movies/x.mp4");
    assert_eq!(items[3].id, "Test/Movies/y.mkv");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_page_two_is_empty() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "Test", 2).expect("list");
    assert!(items.is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_builds_single_group_from_parent_dir() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let detail =
        vbox_core::sources::spider::get_detail(&cfg, &site(&base), "Test/Movies/x.mp4").expect("detail");
    assert_eq!(detail.name, "Movies");
    assert_eq!(detail.id, "Test/Movies/x.mp4");
    assert_eq!(detail.cover.as_deref(), Some(vbox_core::spider::catvod::families::alist::DEFAULT_VOD_PIC));
    assert!(detail.desc.as_deref().unwrap_or_default().contains("AList"));
    assert_eq!(detail.episodes.len(), 1);
    let group = &detail.episodes[0];
    assert_eq!(group.group, "AList");
    // cover.jpg 被扩展名过滤；集按文件名升序。
    assert_eq!(group.items.len(), 2);
    assert_eq!(group.items[0].title, "x");
    assert_eq!(group.items[0].url, "Test/Movies/x.mp4");
    assert_eq!(group.items[1].title, "y");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_without_video_files_returns_empty_episodes() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let detail =
        vbox_core::sources::spider::get_detail(&cfg, &site(&base), "Test/Empty/readme.txt").expect("detail");
    assert_eq!(detail.name, "Empty");
    assert!(detail.episodes.is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_resolves_raw_url() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let value =
        vbox_core::sources::spider::player_content(&cfg, &site(&base), "flag", "Test/Movies/x.mp4", &[])
            .expect("player");
    assert_eq!(value.get("url").and_then(|v| v.as_str()), Some("https://cdn.test/video.mp4"));
    assert_eq!(value.get("parse").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(value.get("jx").and_then(|v| v.as_u64()), Some(0));
    let ua = value
        .get("header")
        .and_then(|h| h.get("User-Agent"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(ua.contains("Chrome/122"), "ua={ua}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_filters_dirs_and_non_video_files() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "x", true).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "Test/Movies/x.mp4");
    assert_eq!(items[0].name, "x.mp4");
    assert_eq!(items[0].cover.as_deref(), Some("https://cdn.test/t.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("Test"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn invalid_id_surfaces_parse_error() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "Other/x").unwrap_err();
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn guest_disabled_login_retry_with_401() {
    let cfg = cfg();
    let (base, state) = start_server(true);
    let items = vbox_core::sources::spider::list_videos(&cfg, &login_site(&base), "Test", 1)
        .expect("list after login");
    assert_eq!(items.len(), 3);
    let guard = state.lock().unwrap();
    assert_eq!(guard.login_calls, 1);
    assert_eq!(guard.list_auth.len(), 2);
    assert!(guard.list_auth[0].is_none());
    assert_eq!(guard.list_auth[1].as_deref(), Some("tok-123"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn hidden_drive_is_not_a_category() {
    let cfg = cfg();
    let (base, _) = start_server(false);
    let mut s = site(&base);
    s.ext = Some(json!({
        "drives": [
            { "name": "Visible", "server": base },
            { "name": "Hidden", "server": base, "hidden": true }
        ]
    }));
    let cats = vbox_core::sources::spider::list_categories(&cfg, &s).expect("categories");
    // 同 server 去重，只保留第一台。
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "Visible");
}

#[test]
#[ignore]
fn live_public_alist_instance_smoke() {
    let cfg = cfg();
    let mut s = Site {
        key: "alist-live".into(),
        name: "AList 公开实例".into(),
        api: "csp_Alist".into(),
        ..Default::default()
    };
    s.ext = Some(json!({ "al": "https://al.chirmyram.com" }));
    let cats = vbox_core::sources::spider::list_categories(&cfg, &s).expect("categories");
    assert_eq!(cats.len(), 1);
    let items = vbox_core::sources::spider::list_videos(&cfg, &s, "al", 1).expect("list root");
    assert!(!items.is_empty(), "公开实例根目录应有内容");
}
