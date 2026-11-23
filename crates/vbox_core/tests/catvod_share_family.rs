//! `csp_QuarkShare` / `csp_UCShare` 家族集成测试（tiny_http mock + 真站 `#[ignore]`）。
//!
//! 覆盖：清单分类、分享浏览出视频、详情展开剧集、播放直链、搜索过滤、
//! 第二页空列表、UC 分支链接前缀。网盘浏览接口用 mock 顶替。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const TOKEN_JSON: &str = r#"{"status":200,"code":0,"data":{"stoken":"mockstoken","title":"测试分享","share_type":0,"expired_at":0}}"#;
const DETAIL_JSON: &str = r#"{"status":200,"code":0,"data":{"list":[
{"fid":"f1","fid_token":"ft1","file_name":"第1集.mp4","pdir_fid":"0","file_type":1,"size":1048576,"format_type":"video/mp4","category":1},
{"fid":"f2","fid_token":"ft2","file_name":"花絮.jpg","pdir_fid":"0","file_type":0,"size":2048,"format_type":"image/jpeg","category":3}
],"count":2,"total":2}}"#;
const DOWNLOAD_JSON: &str = r#"{"status":200,"code":0,"data":[{"download_url":"https://cdn.test/q.m3u8"}]}"#;
const PLAY_JSON: &str = r#"{"status":400,"code":1,"message":"no play"}"#;

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn start_server() -> (Arc<Server>, String) {
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
    let base = format!("http://{}", server.server_addr());
    let srv = server.clone();
    thread::spawn(move || loop {
        let request = match srv.recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        let url = request.url().to_string();
        let body = if url.contains("share/sharepage/token") {
            TOKEN_JSON
        } else if url.contains("share/sharepage/detail") {
            DETAIL_JSON
        } else if url.contains("file/download") {
            DOWNLOAD_JSON
        } else if url.contains("file/v2/play") {
            PLAY_JSON
        } else {
            "{}"
        };
        let json_header = header("Content-Type", "application/json");
        let _ = request.respond(Response::from_string(body).with_header(json_header));
    });
    (server, base)
}

fn site(base: &str, api: &str, share_id: &str) -> Site {
    let ext = format!(
        "{{\"drive_base\":\"{base}/1/clouddrive/\",\"uc_base\":\"{base}/1/clouddrive/\",\"cookie\":\"mock-cookie\"}}$$$[{{\"id\":\"{share_id}\",\"name\":\"测试剧集\"}}]"
    );
    let mut s = Site::default();
    s.key = "share".into();
    s.name = "网盘分享".into();
    s.api = api.into();
    s.ext = Some(json!(ext));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_return_manifest_entries() {
    let (_server, base) = start_server();
    let site = site(&base, "csp_QuarkShare", "s1");
    let cats = vbox_core::sources::spider::list_categories(&cfg(), &site).unwrap();
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_name, "测试剧集");
    assert_eq!(cats[0].type_id, "https://pan.quark.cn/s/s1");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn videos_browse_share_files() {
    let (_server, base) = start_server();
    let site = site(&base, "csp_QuarkShare", "s1");
    let items = vbox_core::sources::spider::list_videos(
        &cfg(),
        &site,
        "https://pan.quark.cn/s/s1",
        1,
    )
    .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "第1集.mp4");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/s1###f1");
    assert_eq!(items[0].remark.as_deref(), Some("1.0MB"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_expands_episodes() {
    let (_server, base) = start_server();
    let site = site(&base, "csp_QuarkShare", "s1");
    let detail = vbox_core::sources::spider::get_detail(
        &cfg(),
        &site,
        "https://pan.quark.cn/s/s1###f1",
    )
    .unwrap();
    assert_eq!(detail.name, "测试剧集");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "网盘");
    assert_eq!(detail.episodes[0].items.len(), 1);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_filters_manifest_names() {
    let (_server, base) = start_server();
    let site = site(&base, "csp_QuarkShare", "s1");
    let items = vbox_core::sources::spider::search(&cfg(), &site, "测试", false).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "测试剧集");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_returns_direct_url() {
    let (_server, base) = start_server();
    let site = site(&base, "csp_QuarkShare", "s1");
    let value = vbox_core::sources::spider::player_content(
        &cfg(),
        &site,
        "",
        "https://pan.quark.cn/s/s1###f1",
        &[],
    )
    .unwrap();
    assert_eq!(value["url"], "https://cdn.test/q.m3u8");
    assert_eq!(value["parse"], 0);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn uc_api_uses_uc_prefix() {
    let (_server, base) = start_server();
    let site = site(&base, "csp_UCShare", "u1");
    let cats = vbox_core::sources::spider::list_categories(&cfg(), &site).unwrap();
    assert_eq!(cats[0].type_id, "https://drive.uc.cn/s/u1");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn second_page_is_empty() {
    let (_server, base) = start_server();
    let site = site(&base, "csp_QuarkShare", "s1");
    let items = vbox_core::sources::spider::list_videos(
        &cfg(),
        &site,
        "https://pan.quark.cn/s/s1",
        2,
    )
    .unwrap();
    assert!(items.is_empty());
}

#[test]
#[ignore = "needs network: public share browsing"]
fn live_share_categories_offline_placeholder() {
    let site = site("https://drive.quark.cn/1/clouddrive", "csp_QuarkShare", "ffffffffffff");
    let cats = vbox_core::sources::spider::list_categories(&cfg(), &site).unwrap();
    assert_eq!(cats.len(), 1);
}
