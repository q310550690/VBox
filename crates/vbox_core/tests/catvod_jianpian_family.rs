//! csp_Jianpian 家族集成测试：走 `sources::spider` 的 5 个入口。
//!
//! mock 覆盖：固定 5 分类、`/api/crumb/list` 列表、`/api/video/search` 搜索、
//! `/api/node/detail` 详情（含海报 catvod 头标记清理、ftp→tvbox-xg:ftp 重写）、
//! `playerContent` 直出 + `JPAUTH` 头，以及空 detail 的 `catvod.result.parse`。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};

const LIST_JSON: &str = r#"{"data":[
  {"jump_id":"42","title":"流浪地球","thumbnail":"https://img.test/1.jpg@Referer=www.jianpianapp.com@User-Agent=jianpian-version362","mask":"已完结"},
  {"id":"43","title":"满江红","path":"https://img.test/2.jpg","playlist":{"title":"更新至10集"}}
]}"#;

const SEARCH_JSON: &str = r#"{"data":[
  {"jump_id":"42","title":"流浪地球 4K","thumbnail":"https://img.test/1.jpg"}
]}"#;

const DETAIL_JSON: &str = r#"{"data":{
  "jump_id":"42",
  "title":"流浪地球",
  "thumbnail":"https://img.test/1.jpg@Referer=www.jianpianapp.com@User-Agent=jianpian-version362",
  "description":"太阳即将毁灭　人类开启流浪计划",
  "year":{"title":"2021"},
  "area":{"title":"大陆"},
  "actors":[{"name":"吴京"},{"name":"刘德华"}],
  "directors":[{"name":"郭帆"}],
  "btbo_downlist":[{"val":"https://cdn.test/1.m3u8"},{"val":"ftp://cdn.test/2.mkv"}]
}}"#;

const EMPTY_DETAIL_JSON: &str = r#"{"data":null}"#;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    let base_for_thread = base.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let (body, ctype) = if url.contains("/api/crumb/list") {
                (LIST_JSON.to_string(), "application/json")
            } else if url.contains("/api/video/search") {
                (SEARCH_JSON.to_string(), "application/json")
            } else if url.contains("/api/node/detail") && url.contains("id=bad") {
                (EMPTY_DETAIL_JSON.to_string(), "application/json")
            } else if url.contains("/api/node/detail") {
                (DETAIL_JSON.to_string(), "application/json")
            } else {
                (String::from("not found"), "text/plain")
            };
            let _ = req.respond(
                Response::from_string(body)
                    .with_header(Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap()),
            );
        }
    });
    base_for_thread
}

fn site(base: &str) -> Site {
    Site {
        key: "jianpian".to_string(),
        name: "荐片".to_string(),
        api: "csp_Jianpian".to_string(),
        ext: Some(json!(base)),
        ..Site::default()
    }
}

#[test]
fn categories_are_fixed_five() {
    let base = start_server();
    let cfg = TvConfig::default();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("分类");
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(names, vec!["全部", "电影", "电视剧", "动漫", "综艺"]);
    assert_eq!(cats[1].type_id, "1");
}

#[test]
fn list_videos_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "1", 1).expect("列表");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].id, "42");
    assert_eq!(items[0].name, "流浪地球");
    assert_eq!(items[0].cover.as_deref(), Some("https://img.test/1.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("已完结"));
    assert_eq!(items[1].remark.as_deref(), Some("更新至10集"));
}

#[test]
fn search_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items =
        vbox_core::sources::spider::search(&cfg, &site(&base), "流浪地球", false).expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "流浪地球 4K");
}

#[test]
fn detail_maps_metadata_and_episodes() {
    let base = start_server();
    let cfg = TvConfig::default();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "42").expect("详情");
    assert_eq!(detail.name, "流浪地球");
    assert_eq!(detail.year.as_deref(), Some("2021"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.actor.as_deref(), Some("吴京 刘德华"));
    assert_eq!(detail.director.as_deref(), Some("郭帆"));
    assert_eq!(detail.desc.as_deref(), Some("太阳即将毁灭人类开启流浪计划"));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "Jianpian");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(
        detail.episodes[0].items[1].url,
        "tvbox-xg:ftp://cdn.test/2.mkv"
    );
}

#[test]
fn player_returns_direct_url_with_jpauth() {
    let base = start_server();
    let cfg = TvConfig::default();
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "Jianpian",
        "https://cdn.test/1.m3u8",
        &[],
    )
    .expect("播放");
    assert_eq!(value["url"], "https://cdn.test/1.m3u8");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["header"]["JPAUTH"], "y261ow7kF2dtzlxh1GS9EB8nbTxNmaK/QQIAjctlKiEv");
}

#[test]
fn empty_detail_reports_parse_error() {
    let base = start_server();
    let cfg = TvConfig::default();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "bad")
        .expect_err("空 detail 应报解析错");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.result.parse"), "实际: {msg}");
}
