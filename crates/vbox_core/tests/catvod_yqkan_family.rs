//! csp_YQKan 家族集成测试：签名 App API 的 5 个入口全走 `sources::spider`。
//!
//! mock 校验：每个 POST body 必须带 `appId`/`reqDomain`/`requestId`/`udid` 与 32 位 `sign`
//! （签名公式见 `families/yqkan.rs` 单测的参考向量），且请求头是 Chrome/77 UA。

use std::io::Read;
use std::thread;

use serde_json::{json, Value};
use tiny_http::{Header, Request, Response, Server};
use vbox_core::model::{Site, TvConfig};

const HEADER_JSON: &str = r#"{"code":0,"data":{"channelList":[
  {"channelId":"12","channelName":"电影"},
  {"channelId":"13","channelName":"电视剧"}
]}}"#;

const LIST_JSON: &str = r#"{"code":0,"data":{"nextVal":"cursor-2","items":[
  {"vodId":"1001","vodName":"海贼王","coverImg":"https://img.test/a.jpg","watchingCountDesc":"12.3万人在看"},
  {"vodId":"1002","vodName":"火影忍者","coverImg":"https://img.test/b.jpg","watchingCountDesc":"8万人"}
]}}"#;

const SEARCH_JSON: &str = r#"{"code":0,"data":{"items":[
  {"vodId":"1001","vodName":"海贼王 4K","coverImg":"https://img.test/a.jpg","watchingCountDesc":"1万人在看"}
]}}"#;

const DETAIL_JSON: &str = r#"{"code":0,"data":{
  "vodName":"海贼王","coverImg":"https://img.test/a.jpg","year":"1999","areaName":"日本",
  "intro":"草帽一伙的冒险","playerList":[
    {"playerName":"线路一","epList":[{"epName":"第1集","epId":"E1"},{"epName":"第2集","epId":"E2"}]}
  ]
}}"#;

const EP_JSON: &str = r#"{"code":0,"data":{"resolutionItems":[
  {"vodResolution":"720"},{"vodResolution":"1080"}
]}}"#;

const PLAY_JSON: &str = r#"{"result":true,"data":"https://cdn.test/x.m3u8"}"#;
const PLAY_FAIL_JSON: &str = r#"{"result":false,"data":""}"#;

fn read_body(req: &mut Request) -> Value {
    let mut buf = String::new();
    let _ = req.as_reader().take(64 * 1024).read_to_string(&mut buf);
    serde_json::from_str(&buf).unwrap_or(Value::Null)
}

fn assert_signed(body: &Value) {
    assert_eq!(body["appId"], "e6ddefe09e0349739874563459f56c54");
    assert_eq!(body["reqDomain"], "m.yqktv888.com");
    let sign = body["sign"].as_str().unwrap_or("");
    assert_eq!(sign.len(), 32, "sign 长度应为 32: {sign:?}");
    assert!(sign.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert!(!body["requestId"].as_str().unwrap_or("").is_empty());
    assert!(!body["udid"].as_str().unwrap_or("").is_empty());
}

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    let base_for_thread = base.clone();
    thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let url = req.url().to_string();
            let body = read_body(&mut req);
            let ua = req
                .headers()
                .iter()
                .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("user-agent"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            let is_post = req.method().as_str() == "POST";
            let (payload, ctype) = if !is_post {
                (String::from("{\"code\":-1,\"data\":null}"), "application/json")
            } else if url.contains("/v1/api/home/header") {
                assert_signed(&body);
                (HEADER_JSON.to_string(), "application/json")
            } else if url.contains("/v1/api/search/queryNow") {
                assert_signed(&body);
                assert!(
                    body["queryValueJson"].as_str().unwrap_or("").contains("\"filerValue\":\"12\""),
                    "分类过滤应带 filerValue: {body}"
                );
                (LIST_JSON.to_string(), "application/json")
            } else if url.contains("/v1/api/search/search") {
                assert_signed(&body);
                assert_eq!(body["keyword"], "海贼");
                (SEARCH_JSON.to_string(), "application/json")
            } else if url.contains("/v1/api/vodInfo/detail") {
                assert_signed(&body);
                assert_eq!(body["vodId"], "1001");
                (DETAIL_JSON.to_string(), "application/json")
            } else if url.contains("/v1/api/vodInfo/getEpDetail") {
                assert_signed(&body);
                (EP_JSON.to_string(), "application/json")
            } else if url.contains("/v1/api/vodInfo/getPlayUrl") {
                assert_signed(&body);
                assert_eq!(body["vodResolution"], "1080", "应取最后一个清晰度");
                if body["epId"] == "BAD" {
                    (PLAY_FAIL_JSON.to_string(), "application/json")
                } else {
                    (PLAY_JSON.to_string(), "application/json")
                }
            } else {
                (String::from("{\"code\":-1}"), "application/json")
            };
            assert!(ua.contains("Chrome/77"), "UA 应为 Chrome/77，实际 {ua:?}");
            let _ = req.respond(
                Response::from_string(payload)
                    .with_header(Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap()),
            );
        }
    });
    base_for_thread
}

fn site(base: &str) -> Site {
    Site {
        key: "yqkan".to_string(),
        name: "一起看".to_string(),
        api: "csp_YQKan".to_string(),
        ext: Some(json!(base)),
        ..Site::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("分类");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "12");
    assert_eq!(cats[0].type_name, "电影");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn list_videos_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "12", 1).expect("列表");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "1001");
    assert_eq!(items[0].name, "海贼王");
    assert_eq!(items[0].cover.as_deref(), Some("https://img.test/a.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("12.3万人在看"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_via_dispatch() {
    let base = start_server();
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "海贼", false).expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "海贼王 4K");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_maps_metadata_and_groups() {
    let base = start_server();
    let cfg = TvConfig::default();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "1001").expect("详情");
    assert_eq!(detail.name, "海贼王");
    assert_eq!(detail.year.as_deref(), Some("1999"));
    assert_eq!(detail.area.as_deref(), Some("日本"));
    assert_eq!(detail.desc.as_deref(), Some("草帽一伙的冒险"));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items[1].url, "E2");
    assert_eq!(detail.episodes[0].items[1].title, "第2集");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_resolves_resolution_then_url() {
    let base = start_server();
    let cfg = TvConfig::default();
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "线路一",
        "E1",
        &[],
    )
    .expect("播放");
    assert_eq!(value["url"], "https://cdn.test/x.m3u8");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["header"]["User-Agent"].as_str().unwrap().contains("Chrome/77"), true);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn play_url_failure_reports_host_api_error() {
    let base = start_server();
    let cfg = TvConfig::default();
    let err = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "线路一",
        "BAD",
        &[],
    )
    .expect_err("播放失败应报错");
    assert!(format!("{err:?}").contains("catvod.host.unsupported"), "实际: {err:?}");
}
