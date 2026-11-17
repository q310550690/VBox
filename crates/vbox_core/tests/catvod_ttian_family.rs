//! `csp_TTian`（天天影视）家族集成测试（tiny_http mock，离线）。
//!
//! 上游三个 ext 主机（op.ysdqjs.cn / app.kzjtv.com）在 2026-11 已全部不可达，
//! 因此本文件全部用 mock 验收：四个 POST 端点的 multipart 请求体、JSON 映射，
//! 以及 playerContent 的三条分支（解析站换链 / m3u8 直链 / 未知站点报错）。
//! 特意不用 `#[ignore]` 真站用例——上游已死，真站用例只会制造噪音。

use std::io::Read;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Method, Response, Server};

use vbox_core::model::{Site, TvConfig};

const TOP_TYPE: &str = r#"{"code":0,"data":{"list":[
  {"type_id":"1","type_name":"电影"},
  {"type_id":"2","type_name":"电视剧"}
]}}"#;

const TJ_VOD: &str = r#"{"code":0,"data":{"cai":[
  {"vod_id":11,"vod_name":"首页推荐","vod_pic_thumb":"http://img/a.jpg","vod_remarks":"更新至3集"},
  {"vod_id":12,"vod_name":"无封面","vod_remarks":"HD"}
]}}"#;

const TYPE_SEARCH: &str = r#"{"code":0,"data":{"list":[
  {"vod_id":21,"vod_name":"分类条目","vod_pic":"http://img/b.jpg","vod_remarks":"全12集"},
  {"vod_id":22,"vod_name":"条目二","vod_pic":"","vod_pic_thumb":"http://img/c.jpg","vod_remarks":""}
]}}"#;

const VOD_DETAILS: &str = r#"{"code":0,"data":{
  "vod_id":31,"vod_name":"详情条目","vod_pic":"http://img/d.jpg",
  "vod_class":"国产剧","vod_year":"2024","vod_area":"大陆",
  "vod_actor":"演员甲,演员乙","vod_director":"导演丙","vod_content":"剧情简介。",
  "vod_play_list":[
    {"title":"量子线路","parse_urls":["http://parse/u="],"urls":[
      {"name":"第1集","url":"http://cdn/1.m3u8"},
      {"name":"第2集","url":"http://cdn/2.m3u8"}]},
    {"title":"备用线路","parse_urls":[],"urls":[
      {"name":"正片","url":"http://cdn/3.m3u8"}]},
    {"title":"空线路","parse_urls":[],"urls":[]}
  ]
}}"#;

const SEARCH_LIST: &str = r#"{"code":0,"data":{"list":[
  {"vod_id":41,"vod_name":"搜索命中","vod_pic":"http://img/e.jpg","vod_remarks":"完结"}
]}}"#;

const PLAY_RESOLVED: &str = r#"{"url":"https://cdn/resolved.m3u8"}"#;

fn respond_json(request: tiny_http::Request, body: &'static str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("h");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

/// 读取请求体（multipart），供断言使用。
fn read_body(request: &mut tiny_http::Request) -> String {
    let mut text = String::new();
    let _ = request
        .as_reader()
        .take(64 * 1024)
        .read_to_string(&mut text);
    text
}

fn start_server() -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || loop {
        let Ok(mut request) = server.recv() else {
            return;
        };
        let url = request.url().to_string();
        let is_post = matches!(request.method(), Method::Post);
        if is_post {
            let body = read_body(&mut request);
            if url.starts_with("/v2/type/top_type") {
                // 断言 formUpload 的 multipart 结构与签名/时间戳字段。
                assert!(body.contains("name=\"sign\""), "body={body}");
                assert!(body.contains("name=\"timestamp\""), "body={body}");
                respond_json(request, TOP_TYPE);
            } else if url.starts_with("/v2/type/tj_vod") {
                respond_json(request, TJ_VOD);
            } else if url.starts_with("/v2/home/type_search") {
                assert!(body.contains("--4b8a0b47-84bc-4b6e-b252-db25b4b9abd2"));
                assert!(body.contains("name=\"limit\""));
                assert!(body.contains("type_id"));
                respond_json(request, TYPE_SEARCH);
            } else if url.starts_with("/v2/home/vod_details") {
                assert!(body.contains("vod_id"));
                respond_json(request, VOD_DETAILS);
            } else if url.starts_with("/v2/home/search") {
                assert!(body.contains("keyword"));
                respond_json(request, SEARCH_LIST);
            } else if url.starts_with("/v2/home/missing") {
                respond_json(request, r#"{"code":0}"#);
            } else {
                respond_json(request, r#"{"code":404}"#);
            }
        } else if url.starts_with("/parse") {
            assert!(url.contains("sign="), "url={url}");
            respond_json(request, PLAY_RESOLVED);
        } else {
            respond_json(request, r#"{"code":404}"#);
        }
    });
    base
}

fn cfg() -> TvConfig {
    let mut c = TvConfig::default();
    c.http.timeout = 15_000;
    c
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "ttian".into(),
        name: "天天影视".into(),
        api: "csp_TTian".into(),
        ..Default::default()
    };
    s.ext = Some(json!(format!("{base}/$$$null$$$1")));
    s
}

#[test]
fn categories_come_from_top_type() {
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg(), &site(&base)).expect("cats");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[1].type_name, "电视剧");
}

#[test]
fn home_videos_parse_cai_items() {
    let base = start_server();
    // 首页推荐不走 `sources::spider` 的五个入口，直接调家族公开函数。
    let items = vbox_core::spider::catvod::families::ttian::home_videos(&cfg(), &site(&base))
        .expect("v");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "11");
    assert_eq!(items[0].cover.as_deref(), Some("http://img/a.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("更新至3集"));
    // 第二项没有 vod_pic，只有 thumb 之外为空 → 封面 None。
    assert!(items[1].cover.is_none());
}

#[test]
fn type_search_maps_vod_fields() {
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg(), &site(&base), "2", 3).expect("v");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "21");
    assert_eq!(items[1].cover.as_deref(), Some("http://img/c.jpg"));
}

#[test]
fn detail_maps_lines_flags_and_metadata() {
    let base = start_server();
    let detail =
        vbox_core::sources::spider::get_detail(&cfg(), &site(&base), "31").expect("detail");
    assert_eq!(detail.name, "详情条目");
    assert_eq!(detail.cover.as_deref(), Some("http://img/d.jpg"));
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.director.as_deref(), Some("导演丙"));
    assert_eq!(detail.actor.as_deref(), Some("演员甲,演员乙"));
    assert_eq!(detail.desc.as_deref(), Some("[国产剧] 剧情简介。"));
    // 第三条线路没有剧集 → 被丢弃；线路名保留原名。
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "量子线路");
    assert_eq!(detail.episodes[0].items[0].title, "第1集");
    // parse_urls 有 1 个 → 该 URL 作 flag，附加在剧集 URL 后面。
    assert_eq!(detail.episodes[0].items[0].url, "http://cdn/1.m3u8|http://parse/u=");
    // parse_urls 为空 → flag 为 "no"（原类语义）。
    assert_eq!(detail.episodes[1].group, "备用线路");
    assert_eq!(detail.episodes[1].items[0].url, "http://cdn/3.m3u8|no");
}

#[test]
fn player_passes_through_when_flag_is_short() {
    let base = start_server();
    let value = vbox_core::sources::spider::player_content(
        &cfg(),
        &site(&base),
        "",
        "http://cdn/raw.mp4",
        &[],
    )
    .expect("player");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "http://cdn/raw.mp4");
}

#[test]
fn player_resolves_parse_flag_through_signed_get() {
    let base = start_server();
    let id = format!("http://cdn/need.m3u8|{base}/parse?u=");
    let value =
        vbox_core::sources::spider::player_content(&cfg(), &site(&base), "", &id, &[]).expect("p");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "https://cdn/resolved.m3u8");
}

#[test]
fn player_rejects_unknown_site() {
    let base = start_server();
    let err = vbox_core::sources::spider::player_content(
        &cfg(),
        &site(&base),
        "",
        "https://unknown.example/video|abc",
        &[],
    )
    .expect_err("should fail");
    let message = err.to_string();
    assert!(message.contains("catvod.play.parse"), "message={message}");
}

#[test]
fn empty_type_id_is_a_parse_error() {
    let base = start_server();
    let err = vbox_core::sources::spider::list_videos(&cfg(), &site(&base), "  ", 1)
        .expect_err("should fail");
    let message = err.to_string();
    assert!(message.contains("catvod.result.parse"), "message={message}");
}

#[test]
fn categories_fall_back_when_upstream_has_no_list() {
    let base = start_server();
    // 给基址加前缀，mock 返回 {"code":404} → 没有 data.list → 回退硬编码 4 类。
    let dead = format!("{base}/dead");
    let cats = vbox_core::sources::spider::list_categories(&cfg(), &site(&dead)).expect("cats");
    assert_eq!(cats.len(), 4);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[2].type_name, "动漫");
}
