//! `csp_PikPakShare` 家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、内联 `list_source` 清单 → 分类（单层 + 分组）、
//! 分组分页、分享 URL 浏览根目录、目录文件展开为剧集、播放直链（分享口令 + 文件信息）、
//! 本地关键词搜索、缺少清单源时的 `catvod.result.parse` 错误。
//!
//! 上游差异：真实 ext 的清单源是 `./json/pikpak.txt` 相对路径，桌面端无配置
//! 目录无法解析；本测试用 ext JSON 钩子 `list_source` 内联清单，并用
//! `pikpak_base`/`pikpak_auth_base` 把 drive 根指到 mock（仅测试/镜像钩子）。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::families;
use vbox_core::spider::catvod::registry;

const SHARE: &str = "https://mypikpak.com/s/abc123";

/// 分享口令响应（`GET /share?...`）。
const SHARE_TOKEN: &str = r#"{"pass_code_token":"pct","title":"PikPak 分享","share_id":"abc123"}"#;

/// 分享目录一页（`GET /share/detail?...`）。
const DETAIL: &str = r#"{"files":[
    {"id":"f1","name":"ep1.mp4","kind":"file","category":"video","size":1024},
    {"id":"d1","name":"合集","kind":"folder","size":0}
],"next_page_token":""}"#;

/// 分享文件信息（`GET /share/file_info?...`），含直链。
const FILE_INFO: &str =
    r#"{"links":{"application/octet-stream":{"url":"https://cdn.test/ep1.mp4"}}}"#;

fn respond_json(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes("Content-Type", "application/json").expect("ctype");
    request
        .respond(Response::from_string(body.to_string()).with_header(header))
        .expect("respond json");
}

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("server");
    let base = format!("http://{}", server.server_addr());
    let ret = base.clone();
    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let url = request.url().to_string();
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            if url.contains("share/detail") {
                respond_json(request, DETAIL);
            } else if url.contains("share/file_info") {
                respond_json(request, FILE_INFO);
            } else if url.contains("share?") {
                respond_json(request, SHARE_TOKEN);
            } else {
                respond_json(request, "{}");
            }
        }
    });
    ret
}

fn config() -> TvConfig {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 5_000;
    cfg
}

fn site(base: &str) -> Site {
    Site {
        key: "pikpakshare".into(),
        name: "PikPak 分享".into(),
        api: "csp_PikPakShare".into(),
        kind: 3,
        ext: Some(json!(format!(
            "{{\"list_source\":\"abc123 剧集一\\ndef456 电影/剧集二\",\"pikpak_base\":\"{base}/\",\"pikpak_auth_base\":\"{base}/\"}}"
        ))),
        ..Default::default()
    }
}

#[test]
fn dispatch_and_registry_recognise_pikpakshare() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    assert!(registry::is_native_ready("csp_PikPakShare"));
    assert!(registry::blocked_reason("csp_PikPakShare").is_none());
    assert_eq!(families::family_of("csp_PikPakShare"), Some("pikpakshare"));
    assert_eq!(families::family_of("  csp_pikpakshare  "), Some("pikpakshare"));
    let cats = spider::list_categories(&cfg, &s).expect("categories");
    assert_eq!(cats.len(), 2, "{cats:?}");
    assert_eq!(cats[0].type_id, "https://mypikpak.com/s/abc123");
    assert_eq!(cats[0].type_name, "剧集一");
    assert_eq!(cats[1].type_id, "电影");
}

#[test]
fn videos_group_returns_entries() {
    let base = start_server();
    let items = spider::list_videos(&config(), &site(&base), "电影", 1).expect("videos");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].id, "https://mypikpak.com/s/def456");
    assert_eq!(items[0].name, "剧集二");
}

#[test]
fn videos_share_url_browses_root() {
    let base = start_server();
    let items = spider::list_videos(&config(), &site(&base), SHARE, 1).expect("videos");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].id, format!("{SHARE}###f1"));
    assert_eq!(items[0].name, "ep1.mp4");
    assert_eq!(items[0].remark, None);
    assert_eq!(items[1].id, format!("{SHARE}###d1"));
    assert_eq!(items[1].remark.as_deref(), Some("文件夹"));
}

#[test]
fn detail_expands_files_into_episodes() {
    let base = start_server();
    let detail = spider::get_detail(&config(), &site(&base), SHARE).expect("detail");
    assert_eq!(detail.episodes.len(), 1, "{:?}", detail.episodes);
    let group = &detail.episodes[0];
    assert_eq!(group.group, "PikPak分享");
    assert_eq!(group.items.len(), 1, "{:?}", group.items);
    assert_eq!(group.items[0].title, "ep1.mp4");
    assert_eq!(group.items[0].url, format!("{SHARE}###f1"));
}

#[test]
fn player_returns_direct_url() {
    let base = start_server();
    let s = site(&base);
    let value = spider::player_content(&config(), &s, "", &format!("{SHARE}###f1"), &[])
        .expect("player");
    assert_eq!(value.get("parse").and_then(|v| v.as_i64()), Some(0));
    assert_eq!(
        value.get("url").and_then(|v| v.as_str()),
        Some("https://cdn.test/ep1.mp4")
    );
    let header = value.get("header").expect("header");
    assert!(header.get("User-Agent").is_some());
    assert!(header.get("Referer").is_some());
}

#[test]
fn player_without_file_id_reports_parse_error() {
    let base = start_server();
    let err = spider::player_content(&config(), &site(&base), "", SHARE, &[]).expect_err("缺 fid");
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
    assert!(err.to_string().contains("fid"), "{err}");
}

#[test]
fn search_matches_manifest_names() {
    let base = start_server();
    let items = spider::search(&config(), &site(&base), "剧集二", false).expect("search");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].name, "剧集二");
    let none = spider::search(&config(), &site(&base), "不存在", false).expect("search");
    assert!(none.is_empty());
}

#[test]
fn missing_list_source_reports_parse_error() {
    let base = start_server();
    let mut s = site(&base);
    s.ext = Some(json!(format!("{{\"pikpak_base\":\"{base}/\"}}")));
    let err = spider::list_categories(&config(), &s).expect_err("无清单源");
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
    assert!(err.to_string().contains("清单源"), "{err}");
}
