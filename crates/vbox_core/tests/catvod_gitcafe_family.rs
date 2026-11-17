//! `csp_Gitcafe` 家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、搜索（`action=get_token` + `action=search`，
//! 逐行阿里分享有效性校验）、空关键词不发请求、分类/分类内容为空（搜索型
//! 蜘蛛语义）、详情（阿里分享 → 「阿里云盘」线路剧集 `URL###fid`）、播放
//! （`###` 走网盘引擎取原码流直链）。
//!
//! 上游现状：`gitcafe.net` 在 2026-11 已不可用，故无真站 `#[ignore]` 用例；
//! `ext` JSON 的 `search_base` / `ali_base` 是仅用于测试/镜像的基址钩子
//! （原类 ext 为 Ali proxy token 文本）。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;
use vbox_core::spider::catvod::families;
use vbox_core::spider::catvod::registry;

const SHARE_PREFIX: &str = "https://www.aliyundrive.com/s/";

const SEARCH_JSON: &str = r#"{"success":true,"data":[
    {"title":"三体 全集","alikey":"k1","cat":"电影"},
    {"title":"坏分享条目","alikey":"kbad","cat":"电影"},
    {"title":"繁花","alikey":"k2","cat":"剧集"}
]}"#;

const LIST_JSON: &str = r#"{"items":[
    {"file_id":"f1","file_name":"S01E01.mp4","type":"file","size":1024}
],"next_marker":""}"#;

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
                if body.contains("kbad") {
                    respond_json(
                        request,
                        r#"{"code":"ShareLink.Forbidden","message":"share_link is invalid"}"#,
                    );
                } else {
                    respond_json(
                        request,
                        r#"{"share_name":"测试分享","share_id":"k1","file_infos":[]}"#,
                    );
                }
            } else if url.contains("get_share_token") {
                respond_json(request, r#"{"share_token":"stok","expire_time":4102416000}"#);
            } else if url.contains("file/list") {
                respond_json(request, LIST_JSON);
            } else if url.contains("get_share_link_download_url") {
                respond_json(request, r#"{"url":"https://cdn.test/f1.mp4"}"#);
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
        key: "csp_Gitcafe".into(),
        name: "Gitcafe".into(),
        api: "csp_Gitcafe".into(),
        kind: 3,
        ext: Some(json!({
            "search_base": format!("{base}/alipaper/"),
            "ali_base": format!("{base}/ali/")
        })),
        ..Default::default()
    }
}

#[test]
fn dispatch_and_registry_recognise_gitcafe() {
    let base = start_server();
    let cfg = config();
    let s = site(&base);
    assert!(registry::is_native_ready("csp_Gitcafe"));
    assert!(registry::blocked_reason("csp_Gitcafe").is_none());
    assert_eq!(families::family_of("csp_Gitcafe"), Some("gitcafe"));
    assert!(spider::list_categories(&cfg, &s).expect("cats").is_empty());
}

#[test]
fn search_returns_only_valid_shares() {
    let base = start_server();
    let items = spider::search(&config(), &site(&base), "三体", true).expect("search");
    assert_eq!(items.len(), 2, "无效分享应被过滤: {items:?}");
    assert_eq!(items[0].id, format!("{SHARE_PREFIX}k1"));
    assert_eq!(items[0].name, "三体 全集");
    assert_eq!(items[0].remark.as_deref(), Some("√电影"));
    assert!(
        items[0].cover.as_deref().expect("cover").starts_with("https://"),
        "封面固定图"
    );
    assert_eq!(items[1].id, format!("{SHARE_PREFIX}k2"));
}

#[test]
fn search_with_blank_keyword_returns_empty() {
    let base = start_server();
    let items = spider::search(&config(), &site(&base), "   ", true).expect("search");
    assert!(items.is_empty());
}

#[test]
fn detail_expands_ali_share_into_ali_line() {
    let base = start_server();
    let s = site(&base);
    let id = format!("{SHARE_PREFIX}k1");
    let detail = spider::get_detail(&config(), &s, &id).expect("detail");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "阿里云盘");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(
        detail.episodes[0].items[0].url,
        format!("{id}###f1"),
        "阿里线路剧集 id = URL###fid"
    );
}

#[test]
fn player_resolves_ali_direct_url() {
    let base = start_server();
    let s = site(&base);
    let id = format!("{SHARE_PREFIX}k1###f1");
    let value = spider::player_content(&config(), &s, "阿里云盘", &id, &[]).expect("play");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn.test/f1.mp4");
}

#[test]
fn detail_rejects_blank_id() {
    let base = start_server();
    let err = spider::get_detail(&config(), &site(&base), "  ").expect_err("blank id");
    assert!(format!("{err}").contains("catvod"), "错误码: {err}");
}
