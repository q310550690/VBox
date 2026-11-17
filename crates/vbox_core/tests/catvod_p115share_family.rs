//! `csp_P115Share` 家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、内联 `list_source` 清单 → 分类（单层 + 分组）、
//! 分组分页、分享 URL 浏览根目录、目录文件展开为剧集、播放直链（需 Cookie）、
//! 本地关键词搜索、缺少清单源时的 `catvod.result.parse` 错误。
//!
//! 上游差异：真实 ext 的清单源是 `./json/115share.txt` 相对路径，桌面端无配置
//! 目录无法解析；本测试用 ext JSON 钩子 `list_source` 内联清单，并用 `p115_base`
//! 把 115 webapi 指到 mock（`p115_base` 同样只是测试/镜像钩子）。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::families;
use vbox_core::spider::catvod::registry;

const SHARE: &str = "https://115.com/s/abc123";

/// `share/snap` 快照：一个文件 + 一个目录（parent 为空即根目录）。
const SNAP: &str = r#"{"state":true,"data":{"count":2,"list":[
    {"cid":"f1","n":"ep1.mp4","s":1024,"pc":"0"},
    {"cid":"d1","n":"合集","pc":"0","is_dir":true}
]}}"#;

const DOWNLOAD: &str = r#"{"state":true,"data":{"url":"https://cdn.test/ep1.mp4"}}"#;

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
            if url.contains("share/snap") {
                respond_json(request, SNAP);
            } else if url.contains("share/download") {
                respond_json(request, DOWNLOAD);
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
        key: "p115share".into(),
        name: "115 分享".into(),
        api: "csp_P115Share".into(),
        kind: 3,
        ext: Some(json!(format!(
            "{{\"list_source\":\"abc123 剧集一\\ndef456 电影/剧集二\",\"p115_base\":\"{base}/\",\"cookie\":\"UID=1_A1_1\"}}"
        ))),
        ..Default::default()
    }
}

#[test]
fn dispatch_and_registry_recognise_p115share() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    assert!(registry::is_native_ready("csp_P115Share"));
    assert!(registry::blocked_reason("csp_P115Share").is_none());
    assert_eq!(families::family_of("csp_P115Share"), Some("p115share"));
    assert_eq!(families::family_of("  csp_p115share  "), Some("p115share"));
    let cats = spider::list_categories(&cfg, &s).expect("categories");
    assert_eq!(cats.len(), 2, "{cats:?}");
    assert_eq!(cats[0].type_id, SHARE);
    assert_eq!(cats[0].type_name, "剧集一");
    assert_eq!(cats[1].type_id, "电影");
    assert_eq!(cats[1].type_name, "电影");
}

#[test]
fn videos_group_returns_entries() {
    let base = start_server();
    let items = spider::list_videos(&config(), &site(&base), "电影", 1).expect("videos");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].id, "https://115.com/s/def456");
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
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "115分享");
    let titles: Vec<&str> = detail.episodes[0]
        .items
        .iter()
        .map(|e| e.title.as_str())
        .collect();
    assert_eq!(titles, vec!["ep1.mp4"]);
    assert!(detail.episodes[0].items[0].url.starts_with(&format!("{SHARE}###")));
}

#[test]
fn player_returns_direct_url_with_cookie() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    let detail = spider::get_detail(&cfg, &s, SHARE).expect("detail");
    let episode = &detail.episodes[0].items[0];
    let value = spider::player_content(&cfg, &s, "115分享", &episode.url, &[]).expect("play");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn.test/ep1.mp4", "{value}");
    assert!(
        value["header"]["User-Agent"]
            .as_str()
            .expect("ua")
            .contains("Chrome"),
        "{value}"
    );
    assert_eq!(value["header"]["Referer"], "https://115.com/");
}

#[test]
fn player_without_file_id_reports_parse_error() {
    let base = start_server();
    let err = spider::player_content(&config(), &site(&base), "", SHARE, &[]).expect_err("no fid");
    assert!(err.to_string().contains("catvod.result.parse"), "{err}");
}

#[test]
fn search_matches_manifest_names() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    let items = spider::search(&cfg, &s, "剧集", false).expect("search");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].name, "剧集一");
    assert_eq!(items[1].name, "剧集二");
    let none = spider::search(&cfg, &s, "战狼", false).expect("search none");
    assert!(none.is_empty(), "{none:?}");
    let empty = spider::search(&cfg, &s, "   ", false).expect("search empty");
    assert!(empty.is_empty(), "{empty:?}");
}

#[test]
fn missing_list_source_reports_parse_error() {
    let _base = start_server();
    let s = Site {
        api: "csp_P115Share".into(),
        ext: Some(json!("")),
        ..Default::default()
    };
    let err = spider::list_categories(&config(), &s).expect_err("missing manifest");
    let text = err.to_string();
    assert!(text.contains("catvod.result.parse"), "{text}");
    assert!(text.contains("清单源"), "{text}");
}
