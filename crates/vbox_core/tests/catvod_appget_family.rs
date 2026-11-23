//! `csp_AppGet` 家族集成测试（tiny_http mock，不访问真实网络）。
//!
//! 覆盖：ext JSON 解析、加密信封（AES-128-CBC + Base64）解密、首页分类过滤、
//! 分类列表分页、详情多线路与 `parse_api=` 播放串、搜索、直链播放、`vodParse` 签名解析、
//! 缺配置错误码。

use std::sync::{Arc, Mutex};
use std::thread;

use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use base64::Engine as _;
use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

const KEY: &str = "0123456789abcdef";
const IV: &str = "fedcba9876543210";

fn encrypt(text: &str) -> String {
    let cipher = cbc::Encryptor::<aes::Aes128>::new_from_slices(KEY.as_bytes(), IV.as_bytes())
        .expect("key/iv 长度固定 16 字节");
    let bytes = cipher.encrypt_padded_vec_mut::<Pkcs7>(text.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn envelope(value: &Value) -> String {
    format!("{{\"data\":\"{}\"}}", encrypt(&value.to_string()))
}

#[derive(Clone, Default)]
struct Recorded {
    body: String,
    headers: Vec<(String, String)>,
}

fn start_server() -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let server = Server::http("127.0.0.1:0").expect("mock server 可启动");
    let base = format!("http://{}", server.server_addr());
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let sink = recorded.clone();
    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            let headers = request
                .headers()
                .iter()
                .map(|h| (h.field.as_str().as_str().to_lowercase(), h.value.as_str().to_string()))
                .collect::<Vec<_>>();
            sink.lock().unwrap().push(Recorded { body, headers });

            let url = request.url().to_string();
            let payload = if url.starts_with("/api.php/getappapi.index/initV119") {
                envelope(&json!({
                    "type_list": [
                        {"type_id": 1, "type_name": "电影"},
                        {"type_id": 2, "type_name": "电视剧"},
                        {"type_id": 3, "type_name": "伦理"}
                    ],
                    "recommend_list": [
                        {"vod_id": 11, "vod_name": "测试影片", "vod_pic": "http://img.example/11.jpg", "vod_remarks": "更新至3集"}
                    ]
                }))
            } else if url.starts_with("/api.php/getappapi.index/typeFilterVodList") {
                envelope(&json!({
                    "recommend_list": [
                        {"vod_id": 21, "vod_name": "分类影片A", "vod_pic": "/pic/21.jpg", "vod_remarks": "HD"},
                        {"vod_id": 22, "vod_name": "分类影片B", "vod_pic": "https://img.example/22.jpg", "vod_remarks": ""}
                    ]
                }))
            } else if url.starts_with("/api.php/getappapi.index/vodDetail") {
                envelope(&json!({
                    "vod": {
                        "vod_name": "测试影片",
                        "vod_pic": "https://img.example/11.jpg",
                        "vod_remarks": "更新至3集",
                        "vod_content": "剧情简介",
                        "vod_actor": "演员甲",
                        "vod_director": "导演乙",
                        "vod_class": "动作"
                    },
                    "vod_play_list": [
                        {"player_info": {"show": "线路一", "parse": "1"},
                         "urls": [
                            {"name": "第1集", "url": "http://cdn.example/ep1.m3u8", "parse_api_url": "", "token": "tk1", "nid": "n1"},
                            {"name": "第2集", "url": "http://cdn.example/ep2.m3u8", "parse_api_url": "", "token": "tk2", "nid": "n2"}
                         ]},
                        {"player_info": {"show": "线路二", "parse": "0"},
                         "urls": [
                            {"name": "第1集", "url": "http://cdn.example/ep1.m3u8",
                             "parse_api_url": "https://jx.example/play?url=http://cdn.example/ep1.m3u8", "token": "", "nid": "n3"}
                         ]}
                    ]
                }))
            } else if url.starts_with("/api.php/getappapi.index/searchList") {
                envelope(&json!({
                    "search_list": [
                        {"vod_id": 31, "vod_name": "搜索片", "vod_pic": "https://img.example/31.jpg", "vod_remarks": "BD"}
                    ]
                }))
            } else if url.starts_with("/api.php/getappapi.index/vodParse") {
                envelope(&json!({ "json": "{\"url\":\"http://cdn.example/final.m3u8\"}" }))
            } else {
                "{\"data\":\"\"}".to_string()
            };

            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("header 合法");
            let _ = request.respond(Response::from_string(payload).with_header(header));
        }
    });
    (base, recorded)
}

fn site(base: &str) -> Site {
    let ext = json!({
        "url": base,
        "dataKey": KEY,
        "dataIv": IV,
        "deviceId": "device-1",
        "version": "9",
        "ua": "okhttp/3.14.9",
        "token": "tok-1"
    })
    .to_string();
    let mut site = Site {
        key: "appget".into(),
        name: "App接口站".into(),
        api: "csp_AppGet".into(),
        ..Default::default()
    };
    site.ext = Some(Value::String(ext));
    site
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_filter_skipped_names() {
    let (base, _recorded) = start_server();
    let cats = spider::list_categories(&cfg(), &site(&base)).expect("分类可解析");
    assert_eq!(cats.len(), 2, "伦理 应被过滤");
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[1].type_name, "电视剧");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn videos_use_filtered_endpoint_and_page() {
    let (base, recorded) = start_server();
    let items = spider::list_videos(&cfg(), &site(&base), "1", 2).expect("列表可解析");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "21");
    assert_eq!(items[0].name, "分类影片A");
    assert_eq!(items[1].cover.as_deref(), Some("https://img.example/22.jpg"));
    let recorded = recorded.lock().unwrap();
    let category = recorded
        .iter()
        .find(|r| r.body.contains("\"type_id\""))
        .expect("应发出分类请求");
    assert!(category.body.contains("\"type_id\":\"1\""), "body={}", category.body);
    assert!(category
        .headers
        .iter()
        .any(|(name, value)| name == "app-user-device-id" && value == "device-1"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_builds_groups_and_play_entries() {
    let (base, _recorded) = start_server();
    let detail = spider::get_detail(&cfg(), &site(&base), "11").expect("详情可解析");
    assert_eq!(detail.name, "测试影片");
    assert_eq!(detail.actor.as_deref(), Some("演员甲"));
    assert_eq!(detail.director.as_deref(), Some("导演乙"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items.len(), 2);
    let first = &detail.episodes[0].items[0].url;
    assert!(first.starts_with("第1集$parse_api=1&url="), "url={first}");
    assert!(first.ends_with("&token=tk1|测试影片|n1"), "url={first}");
    assert_eq!(
        detail.episodes[1].items[0].url,
        "第1集$https://jx.example/play?url=http://cdn.example/ep1.m3u8|测试影片|n3"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_returns_rows() {
    let (base, _recorded) = start_server();
    let items = spider::search(&cfg(), &site(&base), "搜索词", false).expect("搜索可解析");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "31");
    assert_eq!(items[0].name, "搜索片");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_returns_direct_link_for_hls() {
    let (base, _recorded) = start_server();
    let value = spider::player_content(
        &cfg(),
        &site(&base),
        "",
        "第1集$http://cdn.example/ep1.m3u8|测试影片|n1",
        &[],
    )
    .expect("直链可播放");
    assert_eq!(value["url"], "http://cdn.example/ep1.m3u8");
    assert_eq!(value["parse"], 0);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_calls_vod_parse_with_signature() {
    let (base, recorded) = start_server();
    let encrypted = encrypt("http://cdn.example/ep9.m3u8");
    let value = spider::player_content(
        &cfg(),
        &site(&base),
        "",
        &format!("第9集$parse_api=1&url={encrypted}&token=tk9|测试影片|n9"),
        &[],
    )
    .expect("解析接口可播放");
    assert_eq!(value["url"], "http://cdn.example/final.m3u8");
    let recorded = recorded.lock().unwrap();
    let parse_request = recorded
        .iter()
        .find(|r| r.headers.iter().any(|(n, _)| n == "app-api-verify-sign"))
        .expect("应调用 vodParse 并带签名头");
    assert!(parse_request.body.contains("parse_api=1"), "body={}", parse_request.body);
    let sign = parse_request
        .headers
        .iter()
        .find(|(n, _)| n == "app-api-verify-sign")
        .map(|(_, v)| v.clone())
        .unwrap();
    assert!(!sign.is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn missing_url_reports_parse_error() {
    let mut site = site("http://127.0.0.1:1");
    site.ext = Some(Value::String("{}".to_string()));
    let error = spider::list_categories(&cfg(), &site).expect_err("缺 url 应报错");
    let text = format!("{error:?}");
    assert!(text.contains("catvod.result.parse"), "text={text}");
}
