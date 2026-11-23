//! `csp_XiaoZhiTiao` 家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、搜索（`action=get_token` + `action=search`）、
//! 空关键词不发请求、详情（阿里分享 token → 目录树 → 剧集）、播放（转码直链、
//! 无转码时回退原码流直链）、以及分类/分类内容为空（搜索型蜘蛛语义）。
//!
//! 上游现状：`gitcafe.net/tool/alipaper/` 在 2026-11 已不可用，阿里分享接口
//! 也需要有效分享，故本家族没有真站 `#[ignore]` 用例；`ext` JSON 的
//! `search_base` / `ali_base` 是仅用于测试/镜像的基址钩子（原类 ext 为
//! Ali proxy token 文本）。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

const SHARE_PREFIX: &str = "https://www.aliyundrive.com/s/";

const SEARCH_JSON: &str = r#"{"success":true,"data":[
    {"alikey":"share1","title":"三体 全集","creatime":"2024-05-01 12:00:00"},
    {"alikey":"","title":"坏条目","creatime":"2024-01-01"},
    {"alikey":"share2","title":"无时间条目"}
]}"#;

const ROOT_LIST: &str = r#"{"items":[
    {"file_id":"dir1","file_name":"第1季","type":"folder","size":0}
],"next_marker":""}"#;

const DIR_LIST: &str = r#"{"items":[
    {"file_id":"f1","file_name":"S01E01.mp4","type":"file","size":1024,"thumbnail":"https://thumb.test/1.jpg"},
    {"file_id":"f3","file_name":"S01E02.mp4","type":"file","size":2048}
],"next_marker":""}"#;

const PLAY_JSON: &str = r#"{"video_preview_play_info":{"live_transcoding_task_list":[
    {"template_id":"LD","url":"https://cdn.test/ld.m3u8"},
    {"template_id":"FHD","url":"https://cdn.test/fhd.m3u8"},
    {"template_id":"UHD","url":"https://cdn.test/uhd.m3u8"}
]}}"#;

const PLAY_EMPTY_JSON: &str = r#"{"video_preview_play_info":{}}"#;

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
            if url.contains("/alipaper/") {
                if body.contains("action=get_token") {
                    respond_json(request, r#"{"success":true,"data":"tok-test"}"#);
                } else if body.contains("action=search") {
                    respond_json(request, SEARCH_JSON);
                } else {
                    respond_json(request, r#"{"success":false}"#);
                }
            } else if url.contains("get_share_by_anonymous") {
                respond_json(
                    request,
                    r#"{"share_name":"三体 全集分享","share_id":"share1","file_infos":[]}"#,
                );
            } else if url.contains("get_share_token") {
                respond_json(request, r#"{"share_token":"stok","expire_time":4102416000}"#);
            } else if url.contains("file/list") {
                if body.contains("\"parent_file_id\":\"root\"") {
                    respond_json(request, ROOT_LIST);
                } else {
                    respond_json(request, DIR_LIST);
                }
            } else if url.contains("video_preview_play_info") {
                if body.contains("f3") {
                    respond_json(request, PLAY_EMPTY_JSON);
                } else {
                    respond_json(request, PLAY_JSON);
                }
            } else if url.contains("get_share_link_download_url") {
                respond_json(request, r#"{"url":"https://cdn.test/download/f3.mp4"}"#);
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
        key: "xiaozhitiao".into(),
        name: "阿里 | 小紙條[jar]".into(),
        api: "csp_XiaoZhiTiao".into(),
        kind: 3,
        ext: Some(json!({
            "search_base": format!("{base}/alipaper/"),
            "ali_base": format!("{base}/ali/")
        })),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_and_registry_recognise_xiaozhitiao() {
    assert_eq!(
        vbox_core::spider::catvod::families::family_of("csp_XiaoZhiTiao"),
        Some("xiaozhitiao")
    );
    assert!(vbox_core::spider::catvod::registry::is_native_ready(
        "csp_XiaoZhiTiao"
    ));
    assert!(vbox_core::spider::catvod::registry::blocked_reason("csp_XiaoZhiTiao").is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_and_videos_are_empty_for_search_only_spider() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    assert!(spider::list_categories(&cfg, &s).expect("分类").is_empty());
    assert!(spider::list_videos(&cfg, &s, "1", 1).expect("列表").is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_returns_ali_share_links() {
    let base = start_server();
    let cfg = config();
    let items = spider::search(&cfg, &site(&base), "三体", true).expect("搜索");
    assert_eq!(items.len(), 2, "空 alikey 的行应被过滤");
    assert_eq!(items[0].id, format!("{SHARE_PREFIX}share1"));
    assert_eq!(items[0].name, "三体 全集");
    assert_eq!(items[0].remark.as_deref(), Some("2024-05-01"));
    assert_eq!(items[1].id, format!("{SHARE_PREFIX}share2"));
    assert_eq!(items[1].remark, None);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_empty_keyword_returns_empty_without_request() {
    let base = start_server();
    let cfg = config();
    assert!(spider::search(&cfg, &site(&base), "   ", true)
        .expect("空关键词")
        .is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_expands_share_directory_into_episodes() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    let detail = spider::get_detail(&cfg, &s, &format!("{SHARE_PREFIX}share1")).expect("详情");
    assert_eq!(detail.name, "三体 全集分享");
    assert_eq!(detail.episodes.len(), 1);
    let group = &detail.episodes[0];
    assert_eq!(group.group, "阿里");
    assert_eq!(group.items.len(), 2);
    assert_eq!(group.items[0].title, "S01E01.mp4");
    assert_eq!(group.items[0].url, "share1|f1");
    assert_eq!(group.items[1].url, "share1|f3");
    assert_eq!(detail.cover.as_deref(), Some("https://thumb.test/1.jpg"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_rejects_non_ali_share() {
    let base = start_server();
    let cfg = config();
    let err = spider::get_detail(&cfg, &site(&base), "https://pan.quark.cn/s/abc")
        .expect_err("非阿里分享应报错");
    let text = format!("{err}");
    assert!(text.contains("catvod."), "应为 Catvod 错误码: {text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_returns_transcoded_url_with_headers() {
    let base = start_server();
    let cfg = config();
    let value = spider::player_content(&cfg, &site(&base), "阿里", "share1|f1", &[]).expect("播放");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn.test/uhd.m3u8", "UHD 优先");
    assert!(value["header"]["User-Agent"]
        .as_str()
        .unwrap_or("")
        .contains("Chrome/122"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_falls_back_to_download_url() {
    let base = start_server();
    let cfg = config();
    let value =
        spider::player_content(&cfg, &site(&base), "阿里", "share1|f3", &[]).expect("播放回退");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn.test/download/f3.mp4");
}
