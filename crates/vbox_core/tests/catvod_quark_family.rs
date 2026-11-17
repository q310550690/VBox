//! `csp_Quark` 家族集成测试（tiny_http mock + 真站 `#[ignore]`）。
//!
//! 覆盖：ext 配置（`quark_cookie`/`drive_base`）→ 分享 token → 分享目录
//! （递归子目录、过滤非视频）→ 详情剧集；播放 id → `file/download` 直链；
//! 分派入口（`sources::spider::{list_categories,get_detail}`）；匿名详情；
//! 非法分享链接报错。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server, StatusCode};

use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::catvod::families::quark as catvod_quark;

const TOKEN_JSON: &str = r#"{"status":200,"code":0,"message":"ok","data":{"stoken":"tok-abc","title":"测试分享","share_type":0,"expired_at":4102416000000}}"#;
const DETAIL_ROOT: &str = r#"{"status":200,"code":0,"message":"ok","data":{"list":[
{"fid":"DIR1","file_name":"S01","pdir_fid":"0","file_type":0,"size":0},
{"fid":"FILE1","file_name":"ep01.mp4","pdir_fid":"0","file_type":1,"size":1024,"format_type":"video/mp4"},
{"fid":"IMG1","file_name":"cover.jpg","pdir_fid":"0","file_type":1,"size":10,"format_type":"image/jpeg"}
]}}"#;
const DETAIL_SUB: &str = r#"{"status":200,"code":0,"message":"ok","data":{"list":[
{"fid":"FILE2","file_name":"ep02.mkv","pdir_fid":"DIR1","file_type":1,"size":2048,"format_type":"video/x-matroska"},
{"fid":"FILE3","file_name":"note.txt","pdir_fid":"DIR1","file_type":1,"size":3}
]}}"#;
const DOWNLOAD_JSON: &str = r#"{"status":200,"code":0,"message":"ok","data":[{"download_url":"https://cdn.test/quark/ep01.mp4"}]}"#;

#[derive(Default)]
struct State {
    token_bodies: Vec<String>,
    download_bodies: Vec<String>,
}

fn respond_json(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
        .expect("header");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

fn start_server() -> (String, Arc<Mutex<State>>) {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let state = Arc::new(Mutex::new(State::default()));
    let handle = Arc::clone(&server);
    let state_handle = Arc::clone(&state);
    thread::spawn(move || loop {
        let Ok(mut request) = handle.recv() else {
            return;
        };
        let url = request.url().to_string();
        let method = request.method().as_str().to_string();
        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);
        let mut st = state_handle.lock().unwrap();
        if url.contains("share/sharepage/token") {
            st.token_bodies.push(body);
            drop(st);
            respond_json(request, TOKEN_JSON);
        } else if url.contains("share/sharepage/detail") {
            let payload = if url.contains("pdir_fid=DIR1") {
                DETAIL_SUB
            } else {
                DETAIL_ROOT
            };
            drop(st);
            respond_json(request, payload);
        } else if url.contains("file/download") {
            st.download_bodies.push(body);
            drop(st);
            respond_json(request, DOWNLOAD_JSON);
        } else if url.contains("file/v2/play") {
            drop(st);
            respond_json(
                request,
                r#"{"status":200,"code":0,"data":{"video":{"play_url":"https://cdn.test/quark/play.m3u8"}}}"#,
            );
        } else {
            drop(st);
            let _ = request.respond(
                Response::from_string(r#"{"status":404,"code":404,"message":"not found"}"#)
                    .with_status_code(StatusCode(404)),
            );
        }
        let _ = method;
    });
    (base, state)
}

fn site(base: &str, cookie: bool) -> Site {
    let ext = if cookie {
        json!(format!(
            r#"{{"quark_cookie":"ck=identity","drive_base":"{base}/1/clouddrive/"}}"#
        ))
    } else {
        json!(format!(r#"{{"drive_base":"{base}/1/clouddrive/"}}"#))
    };
    let mut s = Site::default();
    s.key = "quark".into();
    s.name = "夸克网盘".into();
    s.api = catvod_quark::API.into();
    s.ext = Some(ext);
    s
}

const SHARE_URL: &str = "https://pan.quark.cn/s/24f565ff8b77";

#[test]
fn detail_lists_share_videos_recursively() {
    let (base, _state) = start_server();
    let cfg = TvConfig::default();
    let detail = catvod_quark::detail(&cfg, &site(&base, true), SHARE_URL).expect("detail");
    assert_eq!(detail.name, "测试分享");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "夸克网盘");
    let names: Vec<&str> = detail.episodes[0]
        .items
        .iter()
        .map(|e| e.title.as_str())
        .collect();
    assert!(names.contains(&"ep01.mp4"), "{names:?}");
    assert!(names.contains(&"ep02.mkv"), "{names:?}");
    assert!(!names.contains(&"cover.jpg"), "{names:?}");
    assert!(!names.contains(&"note.txt"), "{names:?}");
    // 播放 id = `{share_id}+{passcode}+{fid}`（passcode 为空时中间段为空）
    let ep01 = detail.episodes[0]
        .items
        .iter()
        .find(|e| e.title == "ep01.mp4")
        .expect("ep01");
    assert_eq!(ep01.url, "24f565ff8b77++FILE1");
}

#[test]
fn player_returns_direct_download_url() {
    let (base, state) = start_server();
    let cfg = TvConfig::default();
    let s = site(&base, true);
    let detail = catvod_quark::detail(&cfg, &s, SHARE_URL).expect("detail");
    let ep01 = detail.episodes[0]
        .items
        .iter()
        .find(|e| e.title == "ep01.mp4")
        .expect("ep01");
    let play = catvod_quark::player_content(&cfg, &s, "", &ep01.url).expect("player");
    assert_eq!(play["url"], "https://cdn.test/quark/ep01.mp4");
    assert_eq!(play["parse"], 0);
    let bodies = state.lock().unwrap().download_bodies.clone();
    assert_eq!(bodies.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&bodies[0]).expect("json");
    assert_eq!(body["fids"][0], "FILE1");
}

#[test]
fn dispatch_routes_quark_family() {
    let (base, state) = start_server();
    let cfg = TvConfig::default();
    let s = site(&base, true);
    let cats = vbox_core::sources::spider::list_categories(&cfg, &s).expect("categories");
    assert!(cats.is_empty());
    let detail = vbox_core::sources::spider::get_detail(&cfg, &s, SHARE_URL).expect("detail");
    assert_eq!(detail.name, "测试分享");
    // token 请求体应带 pwd_id
    let bodies = state.lock().unwrap().token_bodies.clone();
    assert_eq!(bodies.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&bodies[0]).expect("json");
    assert_eq!(body["pwd_id"], "24f565ff8b77");
}

#[test]
fn anonymous_detail_works_without_cookie() {
    let (base, _state) = start_server();
    let cfg = TvConfig::default();
    let detail = catvod_quark::detail(&cfg, &site(&base, false), SHARE_URL).expect("detail");
    assert_eq!(detail.name, "测试分享");
    assert!(!detail.episodes.is_empty());
}

#[test]
fn bad_share_link_is_rejected() {
    let (base, _state) = start_server();
    let cfg = TvConfig::default();
    let err = catvod_quark::detail(&cfg, &site(&base, true), "https://example.com/x").unwrap_err();
    assert!(err.to_string().contains("夸克分享链接"), "{err}");
}

#[test]
fn player_without_cookie_reports_config_error() {
    let (base, _state) = start_server();
    let cfg = TvConfig::default();
    let err = catvod_quark::player_content(&cfg, &site(&base, false), "", "24f565ff8b77++FILE1")
        .unwrap_err();
    assert!(err.to_string().contains("Cookie"), "{err}");
}

#[test]
#[ignore = "依赖真站：公开分享匿名 token/detail（可能被风控或失效）"]
fn live_detail_public_share() {
    let cfg = TvConfig::default();
    let s = site("https://drive.quark.cn", false);
    let detail = catvod_quark::detail(&cfg, &s, "https://pan.quark.cn/s/24f565ff8b77")
        .expect("live detail");
    assert!(!detail.name.trim().is_empty());
}
