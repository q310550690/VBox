//! `csp_AliShare` 家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、内联列表源 → 分类、浏览分享根目录、目录展开为
//! 剧集、文件单集、播放（转码直链优先 / 下载直链回退）、搜索恒空、相对列表源
//! 的桌面端偏差错误。
//!
//! 上游现状：真实站点 ext 的列表源是 `./json/alishare.txt` 相对路径，桌面端
//! 无法解析宿主 config 基址；`ext` JSON 的 `ali_base` 是测试/镜像基址钩子。

use std::io::Read;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::families;
use vbox_core::spider::catvod::registry;

const SHARE: &str = "https://www.alipan.com/s/abc123";

const SHARE_INFO: &str = r#"{"share_name":"测试分享","share_id":"abc123","file_infos":[]}"#;
const TOKEN: &str = r#"{"share_token":"stok","expire_time":4102416000}"#;
const LIST_ROOT: &str = r#"{"items":[
    {"file_id":"d1","file_name":"合集","type":"folder","size":0,"thumbnail":""},
    {"file_id":"f9","file_name":"花絮.mp4","type":"file","size":1024,"thumbnail":"https://img.test/f9.jpg"}
],"next_marker":""}"#;
const LIST_DIR: &str = r#"{"items":[
    {"file_id":"f1","file_name":"ep1.mp4","type":"file","size":2048,"thumbnail":""},
    {"file_id":"f2","file_name":"ep2.mkv","type":"file","size":4096,"thumbnail":""}
],"next_marker":""}"#;
const PLAY_INFO: &str = r#"{"video_preview_play_info":{"live_transcoding_task_list":[
    {"template_id":"FHD","url":"https://cdn.test/fhd.m3u8"},
    {"template_id":"UHD","url":"https://cdn.test/uhd.m3u8"}
]}}"#;
const DOWNLOAD: &str = r#"{"url":"https://cdn.test/dl.mp4"}"#;

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
            if url.contains("get_share_by_anonymous") {
                respond_json(request, SHARE_INFO);
            } else if url.contains("get_share_token") {
                respond_json(request, TOKEN);
            } else if url.contains("/file/list") {
                if body.contains("\"parent_file_id\":\"d1\"") {
                    respond_json(request, LIST_DIR);
                } else {
                    respond_json(request, LIST_ROOT);
                }
            } else if url.contains("video_preview_play_info") {
                if body.contains("\"file_id\":\"f9\"") {
                    respond_json(request, r#"{"video_preview_play_info":{"live_transcoding_task_list":[]}}"#);
                } else {
                    respond_json(request, PLAY_INFO);
                }
            } else if url.contains("get_share_link_download_url") {
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
        key: "csp_AliShare".into(),
        name: "影视分享".into(),
        api: "csp_AliShare".into(),
        kind: 3,
        ext: Some(json!(format!(
            "t$$$[{{\"type_id\":\"abc123\",\"type_name\":\"影视\"}}]$$${{\"ali_base\":\"{base}/\"}}$$$1"
        ))),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_and_registry_recognise_alishare() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    assert!(registry::is_native_ready("csp_AliShare"));
    assert!(registry::blocked_reason("csp_AliShare").is_none());
    assert_eq!(families::family_of("csp_AliShare"), Some("alishare"));
    assert_eq!(families::family_of("  csp_alishare  "), Some("alishare"));
    let cats = spider::list_categories(&cfg, &s).expect("categories");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, SHARE);
    assert_eq!(cats[0].type_name, "影视");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_list_share_root_entries() {
    let base = start_server();
    let items = spider::list_videos(&config(), &site(&base), SHARE, 1).expect("videos");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].name, "📁合集");
    assert_eq!(items[1].name, "花絮.mp4");
    assert_eq!(items[1].remark.as_deref(), Some("1.0 KB"));
    assert_eq!(items[1].cover.as_deref(), Some("https://img.test/f9.jpg"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_expands_folder_into_episodes() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    let items = spider::list_videos(&cfg, &s, SHARE, 1).expect("videos");
    let detail = spider::get_detail(&cfg, &s, &items[0].id).expect("detail");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "阿里云盘");
    let titles: Vec<&str> = detail.episodes[0]
        .items
        .iter()
        .map(|e| e.title.as_str())
        .collect();
    assert_eq!(titles, vec!["ep1.mp4", "ep2.mkv"]);
    assert!(detail.episodes[0].items[0].url.starts_with(&format!("{SHARE}#")));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_file_is_single_episode() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    let items = spider::list_videos(&cfg, &s, SHARE, 1).expect("videos");
    let detail = spider::get_detail(&cfg, &s, &items[1].id).expect("detail");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(detail.episodes[0].items[0].title, "花絮.mp4");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_prefers_transcode_url() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    let items = spider::list_videos(&cfg, &s, SHARE, 1).expect("videos");
    let detail = spider::get_detail(&cfg, &s, &items[0].id).expect("detail");
    let episode = &detail.episodes[0].items[0];
    let value = spider::player_content(&cfg, &s, "阿里云盘", &episode.url, &[]).expect("play");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn.test/uhd.m3u8", "UHD 优先: {value}");
    assert!(
        value["header"]["User-Agent"]
            .as_str()
            .expect("ua")
            .contains("Chrome"),
        "{value}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_falls_back_to_download_url() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    let items = spider::list_videos(&cfg, &s, SHARE, 1).expect("videos");
    let detail = spider::get_detail(&cfg, &s, &items[1].id).expect("detail");
    let episode = &detail.episodes[0].items[0];
    let value = spider::player_content(&cfg, &s, "阿里云盘", &episode.url, &[]).expect("play");
    assert_eq!(value["url"], "https://cdn.test/dl.mp4", "{value}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_is_disabled() {
    let base = start_server();
    let items = spider::search(&config(), &site(&base), "三体", true).expect("search");
    assert!(items.is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn relative_list_source_reports_deviation() {
    let s = Site {
        key: "csp_AliShare".into(),
        name: "影视分享".into(),
        api: "csp_AliShare".into(),
        kind: 3,
        ext: Some(json!("./lib/token.json$$$./json/alishare.txt$$$db$$$1")),
        ..Default::default()
    };
    let err = spider::list_categories(&config(), &s).expect_err("must fail");
    let text = format!("{err}");
    assert!(text.contains("catvod"), "err={text}");
    assert!(text.contains("list_source"), "err={text}");
}
