//! `csp_Jpys` 家族集成测试（mock 签名 API + 1 项真站回归）。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

const LIST_JSON: &str = r#"{"code":1,"msg":"success","data":{"list":[
  {"vodId":147973,"vodName":"杀或救","vodPic":"https://img.test/1.webp","vodVersion":"蓝光"},
  {"vodId":"114815","vodName":"海贼王","vodRemarks":"(1180集)"}
]}}"#;

const SEARCH_JSON: &str = r#"{"code":1,"msg":"success","data":{"result":{"list":[
  {"vodId":114815,"vodName":"海贼王","vodClass":"动画","vodRemarks":"(1180集)"},
  {"vodId":2,"vodName":"伦理片","vodClass":"伦理"},
  {"vodId":3,"vodName":"正常电影","vodClass":"剧情","vodRemarks":""}
]}}}"#;

const DETAIL_JSON: &str = r#"{"code":1,"msg":"success","data":{
  "vodName":"海贼王","vodPic":"https://img.test/op.webp","vodYear":"1999","vodArea":"日本",
  "vodActor":"田中真弓,冈村明美","vodDirector":"宇田钢之助",
  "vodContent":"<p>草帽一伙的冒险</p>",
  "episodeList":[{"nid":795979,"name":"1"},{"nid":1320715,"name":"1180"}]
}}"#;

const PLAY_JSON: &str = r#"{"code":1,"msg":"success","data":{"list":[
  {"url":"https://cdn.test/splitOut/a.m3u8?auth_key=1","resolution":1080}
]}}"#;

const PLAY_EMPTY: &str = r#"{"code":1,"msg":"success","data":{"list":[]}}"#;

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    sign: String,
    deviceid: String,
    ua: String,
}

fn header_value(request: &tiny_http::Request, name: &'static str) -> String {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
}

fn start_server() -> (String, Arc<Mutex<Vec<Seen>>>) {
    let server = Server::http("127.0.0.1:0").expect("server");
    let port = server.server_addr().to_ip().expect("port").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let path = request.url().to_string();
            seen_clone.lock().expect("lock").push(Seen {
                path: path.clone(),
                sign: header_value(&request, "sign"),
                deviceid: header_value(&request, "Deviceid"),
                ua: header_value(&request, "User-Agent"),
            });
            let body = if path.contains("/video/list") {
                LIST_JSON
            } else if path.contains("/video/searchByWord") {
                SEARCH_JSON
            } else if path.contains("/video/detail") {
                if path.contains("id=empty") {
                    r#"{"code":1,"msg":"success","data":{}}"#
                } else {
                    DETAIL_JSON
                }
            } else if path.contains("/v2/video/episode/url") {
                if path.contains("nid=0") {
                    PLAY_EMPTY
                } else {
                    PLAY_JSON
                }
            } else {
                r#"{"code":404,"msg":"not found"}"#
            };
            let response = Response::from_string(body).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                    .expect("header"),
            );
            let _ = request.respond(response);
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn site(base: &str) -> Site {
    Site {
        key: "jpys".to_string(),
        name: "精品影视".to_string(),
        api: "csp_Jpys".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
fn categories_are_fixed_four() {
    let cats = spider::list_categories(&cfg(), &site("http://127.0.0.1:1")).expect("categories");
    let ids: Vec<&str> = cats.iter().map(|c| c.type_id.as_str()).collect();
    assert_eq!(ids, ["1", "2", "4", "3"]);
    assert_eq!(cats[0].type_name, "电影");
}

#[test]
fn videos_sign_request_and_map_rows() {
    let (base, seen) = start_server();
    let items = spider::list_videos(&cfg(), &site(&base), "2", 1).expect("list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "147973");
    assert_eq!(items[0].name, "杀或救");
    assert_eq!(items[0].remark.as_deref(), Some("蓝光"));
    assert_eq!(items[1].id, "114815");
    assert_eq!(items[1].remark.as_deref(), Some("(1180集)"));

    let seen = seen.lock().expect("lock");
    let request = seen.last().expect("one request");
    assert!(request.path.contains("/api/mw-movie/anonymous/video/list?type1=2&pageNum=1"));
    assert_eq!(request.deviceid, "Deviceid");
    assert!(request.ua.contains("Chrome/117"));
    assert_eq!(request.sign.len(), 40, "sha1 hex length: {}", request.sign);
}

#[test]
fn search_skips_ethics_and_signs() {
    let (base, seen) = start_server();
    let items = spider::search(&cfg(), &site(&base), "海贼王", false).expect("search");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "114815");
    assert_eq!(items[1].id, "3");
    let seen = seen.lock().expect("lock");
    let request = seen.last().expect("one request");
    assert!(request.path.contains("/video/searchByWord?keyword="), "{}", request.path);
    assert!(request.path.contains("pageSize=8"));
    assert_eq!(request.sign.len(), 40);
}

#[test]
fn detail_builds_single_online_group() {
    let (base, _seen) = start_server();
    let detail = spider::get_detail(&cfg(), &site(&base), "114815").expect("detail");
    assert_eq!(detail.name, "海贼王");
    assert_eq!(detail.year.as_deref(), Some("1999"));
    assert_eq!(detail.area.as_deref(), Some("日本"));
    assert_eq!(detail.actor.as_deref(), Some("田中真弓,冈村明美"));
    assert_eq!(detail.director.as_deref(), Some("宇田钢之助"));
    assert_eq!(detail.desc.as_deref(), Some("草帽一伙的冒险"));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "在线播放");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[0].url, "114815@795979");
    assert_eq!(detail.episodes[0].items[1].title, "1180");
}

#[test]
fn detail_empty_reports_parse_error() {
    let (base, _seen) = start_server();
    let err = spider::get_detail(&cfg(), &site(&base), "empty").expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.result.parse"), "{err:?}");
}

#[test]
fn player_returns_m3u8_with_headers() {
    let (base, seen) = start_server();
    let value = spider::player_content(&cfg(), &site(&base), "", "114815@795979", &[]).expect("player");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn.test/splitOut/a.m3u8?auth_key=1");
    assert!(value["header"]["Referer"].as_str().unwrap_or("").starts_with("http://127.0.0.1"));
    let seen = seen.lock().expect("lock");
    let request = seen.last().expect("one request");
    assert!(request.path.contains("/v2/video/episode/url?id=114815&nid=795979"), "{}", request.path);
}

#[test]
fn player_empty_url_reports_host_unsupported() {
    let (base, _seen) = start_server();
    let err = spider::player_content(&cfg(), &site(&base), "", "114815@0", &[]).expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.host.unsupported"), "{err:?}");
}

#[test]
fn player_requires_at_separator() {
    let err = spider::player_content(&cfg(), &site("http://127.0.0.1:1"), "", "114815", &[]).expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.host.unsupported"), "{err:?}");
}

/* ============================================================
 * 真站回归（`cargo test -p vbox_core --test catvod_jpys_family -- --ignored`）
 * 站点点 ext 为空时使用默认基址 https://www.ghw9zwp5.com
 * ============================================================ */

fn live_site() -> Site {
    Site {
        key: "jpys-live".to_string(),
        name: "精品影视".to_string(),
        api: "csp_Jpys".to_string(),
        ..Default::default()
    }
}

#[test]
#[ignore = "需要访问 www.ghw9zwp5.com"]
fn live_list_detail_play_roundtrip() {
    let cats = spider::list_categories(&cfg(), &live_site()).expect("live categories");
    assert_eq!(cats.len(), 4, "{cats:?}");

    let items = spider::list_videos(&cfg(), &live_site(), "1", 1).expect("live list");
    assert!(items.len() >= 10, "live list too small: {}", items.len());
    let first = &items[0];
    assert!(!first.id.is_empty());
    println!("live list[0] = {first:?}");

    let detail = spider::get_detail(&cfg(), &live_site(), &first.id).expect("live detail");
    assert!(!detail.name.is_empty(), "empty name");
    assert!(!detail.episodes.is_empty(), "no episodes: {detail:?}");
    println!("live detail = {}  groups={} eps={}", detail.name, detail.episodes.len(), detail.episodes[0].items.len());

    let episode = &detail.episodes[0].items[0];
    let play = spider::player_content(&cfg(), &live_site(), "", &episode.url, &[]).expect("live player");
    let url = play["url"].as_str().unwrap_or_default();
    assert!(url.starts_with("http"), "bad play url: {url}");
    println!("live play = {url}");
}

#[test]
#[ignore = "需要访问 www.ghw9zwp5.com"]
fn live_search_roundtrip() {
    let items = spider::search(&cfg(), &live_site(), "海贼", false).expect("live search");
    assert!(!items.is_empty(), "live search empty");
    assert!(
        items.iter().any(|item| item.name.contains("海贼")),
        "no 海贼 in {:?}",
        items.iter().map(|i| &i.name).collect::<Vec<_>>()
    );
    println!("live search first = {:?}", items[0]);
}
