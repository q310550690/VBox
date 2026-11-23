//! `csp_SP360` 家族集成测试（tiny_http mock + 1 项真站）。
//!
//! 覆盖：固定 4 分类、列表（`data.movies[]`、`cdncover` 补 `https:`、`幕后记录` 过滤、
//! 剧集备注「更新至N集/已完结」）、详情三分支（`allepidetail` 分批取集、
//! `playlinksdetail` 电影「正片」）、搜索（`api.so.360kan.com` 行）、播放
//! `parse=1/jx=1`、错误的 `vod_id` 报 `catvod.result.parse`。真站
//! `api.web.360kan.com` / `api.so.360kan.com` 均在线（列表/详情/搜索可复验）。

use std::sync::Arc;
use std::thread;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const TV_LIST: &str = r#"{"data":{"current_page":1,"movies":[
  {"id":"TV1","title":"兰香如故","cdncover":"//p5.qhimg.com/tv1.jpg","upinfo":"12","total":"24","tag":""},
  {"id":"TV2","title":"蜂鸟行动幕后记录","cdncover":"//p5.qhimg.com/x.jpg"},
  {"id":"TV3","title":"已完结剧","cdncover":"//p5.qhimg.com/tv3.jpg","upinfo":"24","total":"24"}
]},"msg":"ok"}"#;

const MOVIE_LIST: &str = r#"{"data":{"movies":[
  {"id":"MV1","title":"孤岛惊魂","cdncover":"//p5.qhimg.com/mv1.jpg","upinfo":"","total":"","tag":"动作"}
]},"msg":"ok"}"#;

const TV_DETAIL: &str = r#"{"data":{"id":"1","title":"兰香如故","cdncover":"https://p5.qhimg.com/tv1.jpg",
  "moviecategory":["剧情","动作"],"area":["大陆"],"actor":["甲","乙"],"director":["丙"],
  "pubdate":"2026-09-01","description":"一段简介",
  "playlink_sites":["qq"],"allupinfo":{"qq":"3"},
  "allepidetail":{"qq":[{"playlink_num":"1","url":"https://v.qq.com/1.html"}]}},"msg":"Success"}"#;

const TV_EPISODES: &str = r#"{"data":{"allepidetail":{"qq":[
  {"playlink_num":"1","url":"https://v.qq.com/1.html"},
  {"playlink_num":"2","url":"https://v.qq.com/2.html"},
  {"playlink_num":"3","url":"https://v.qq.com/3.html"}]}},"msg":"Success"}"#;

const MOVIE_DETAIL: &str = r#"{"data":{"id":"2","title":"孤岛惊魂","cdncover":"https://p5.qhimg.com/mv1.jpg",
  "moviecategory":["惊悚"],"area":["大陆"],"actor":[],"director":[],"pubdate":"2011-01-01",
  "description":"电影简介","playlink_sites":["qiyi"],
  "playlinksdetail":{"qiyi":{"default_url":"http://www.iqiyi.com/v_1.html"}}},"msg":"Success"}"#;

const SEARCH_JSON: &str = r#"{"code":0,"msg":"ok","data":{"longData":{"rows":[
  {"cat_id":"1","en_id":"EN1","titleTxt":"海贼大冒险","cover":"https://img/1.jpg","cat_name":"电影","url":"http://www.360kan.com/m/EN1.html"}
]}}}"#;

fn header(content_type: &str) -> Header {
    Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).expect("header")
}

fn respond(request: tiny_http::Request, body: &str) {
    let _ = request.respond(
        Response::from_string(body).with_header(header("application/json; charset=utf-8")),
    );
}

fn start_server() -> Arc<Server> {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let handle = Arc::clone(&server);
    thread::spawn(move || loop {
        let Ok(request) = handle.recv() else { return };
        let url = request.url().to_string();
        if url.starts_with("/v1/filter/list") {
            if url.contains("catid=1") {
                respond(request, MOVIE_LIST);
            } else {
                respond(request, TV_LIST);
            }
        } else if url.starts_with("/v1/detail") {
            if url.contains("site=qq") {
                respond(request, TV_EPISODES);
            } else if url.contains("cat=1") {
                respond(request, MOVIE_DETAIL);
            } else {
                respond(request, TV_DETAIL);
            }
        } else if url.starts_with("/index") {
            respond(request, SEARCH_JSON);
        } else {
            respond(request, "{}");
        }
    });
    server
}

fn base_of(server: &Arc<Server>) -> String {
    format!("http://{}", server.server_addr())
}

fn site(base: &str) -> Site {
    let mut s = Site::default();
    s.key = "360".to_string();
    s.name = "360".to_string();
    s.api = "csp_SP360".to_string();
    s.ext = Some(json!(base));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_are_fixed_four() {
    let cats = vbox_core::sources::spider::list_categories(&cfg(), &site("http://127.0.0.1:1"))
        .expect("categories");
    assert_eq!(cats.len(), 4);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[3].type_name, "动漫");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn tv_list_filters_and_remarks() {
    let server = start_server();
    let base = base_of(&server);
    let rows = vbox_core::sources::spider::list_videos(&cfg(), &site(&base), "2", 1).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "兰香如故");
    assert_eq!(rows[0].cover.as_deref(), Some("https://p5.qhimg.com/tv1.jpg"));
    assert_eq!(rows[0].remark.as_deref(), Some("更新至12集"));
    assert_eq!(rows[1].name, "已完结剧");
    assert_eq!(rows[1].remark.as_deref(), Some("已完结"));
    let id: Value = serde_json::from_str(&rows[0].id).expect("id json");
    assert!(id
        .get("detailUrl")
        .and_then(Value::as_str)
        .unwrap()
        .contains("/v1/detail?cat=2&id=TV1&callback="));
    assert_eq!(
        id.get("detailReferer").and_then(Value::as_str).unwrap(),
        "https://www.360kan.com/tv/TV1.html"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn tv_detail_collects_all_episodes() {
    let server = start_server();
    let base = base_of(&server);
    let rows = vbox_core::sources::spider::list_videos(&cfg(), &site(&base), "2", 1).expect("list");
    let d = vbox_core::sources::spider::get_detail(&cfg(), &site(&base), &rows[0].id).expect("detail");
    assert_eq!(d.name, "兰香如故");
    assert_eq!(d.year.as_deref(), Some("2026-09-01"));
    assert_eq!(d.area.as_deref(), Some("大陆"));
    assert_eq!(d.actor.as_deref(), Some("甲,乙"));
    assert_eq!(d.director.as_deref(), Some("丙"));
    assert_eq!(d.desc.as_deref(), Some("一段简介"));
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "qq");
    assert_eq!(d.episodes[0].items.len(), 3);
    assert_eq!(d.episodes[0].items[0].title, "第1集");
    assert_eq!(d.episodes[0].items[2].url, "https://v.qq.com/3.html");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn movie_detail_uses_default_url() {
    let server = start_server();
    let base = base_of(&server);
    let rows = vbox_core::sources::spider::list_videos(&cfg(), &site(&base), "1", 1).expect("list");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].remark.is_none());
    let d = vbox_core::sources::spider::get_detail(&cfg(), &site(&base), &rows[0].id).expect("detail");
    assert_eq!(d.name, "孤岛惊魂");
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "qiyi");
    assert_eq!(d.episodes[0].items[0].title, "正片");
    assert_eq!(d.episodes[0].items[0].url, "http://www.iqiyi.com/v_1.html");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_rows_map_to_detail_json() {
    let server = start_server();
    let base = base_of(&server);
    let rows = vbox_core::sources::spider::search(&cfg(), &site(&base), "海贼", false).expect("search");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "海贼大冒险");
    assert_eq!(rows[0].remark.as_deref(), Some("电影"));
    let id: Value = serde_json::from_str(&rows[0].id).expect("id json");
    assert_eq!(
        id.get("detailReferer").and_then(Value::as_str).unwrap(),
        "http://www.360kan.com/m/EN1.html"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_marks_parse_and_jx() {
    let out = vbox_core::sources::spider::player_content(
        &cfg(),
        &site("http://127.0.0.1:1"),
        "",
        "https://v.qq.com/x.html",
        &[],
    )
    .expect("player");
    assert_eq!(out.get("parse").and_then(Value::as_i64), Some(1));
    assert_eq!(out.get("jx").and_then(Value::as_i64), Some(1));
    assert!(out.get("url").and_then(Value::as_str).unwrap().starts_with("https://v.qq.com"));
    let header = out.get("header").and_then(Value::as_object).unwrap();
    assert!(header.get("User-Agent").and_then(Value::as_str).unwrap().contains("Chrome/39"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn sp33_variant_uses_plain_ids_and_string_jx() {
    let server = start_server();
    let base = base_of(&server);
    let mut sp = site(&base);
    sp.api = "csp_SP33".to_string();

    let cats = vbox_core::sources::spider::list_categories(&cfg(), &sp).expect("categories");
    assert_eq!(cats.len(), 4);

    let rows = vbox_core::sources::spider::list_videos(&cfg(), &sp, "2", 1).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "2_TV1");
    assert_eq!(rows[0].name, "兰香如故");
    assert_eq!(rows[0].remark.as_deref(), Some("更新至12集"));

    let d = vbox_core::sources::spider::get_detail(&cfg(), &sp, &rows[0].id).expect("detail");
    assert_eq!(d.name, "兰香如故");
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "qq");

    let hits = vbox_core::sources::spider::search(&cfg(), &sp, "海贼", false).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "1_EN1");
    assert_eq!(hits[0].name, "海贼大冒险");

    let out = vbox_core::sources::spider::player_content(&cfg(), &sp, "", "1_EN1", &[]).expect("player");
    assert_eq!(out.get("jx").and_then(Value::as_str), Some("1"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn bad_vod_id_reports_parse_error() {
    let server = start_server();
    let base = base_of(&server);
    let err = vbox_core::sources::spider::get_detail(&cfg(), &site(&base), "not-a-json-id")
        .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.result.parse"));
}

/// 真站：360 影视官方 API 在线（列表 / 详情 / 搜索）。
#[test]
#[ignore = "live network: api.web.360kan.com"]
fn live_360kan_list_and_search() {
    let mut s = Site::default();
    s.key = "360".to_string();
    s.name = "360".to_string();
    s.api = "csp_SP360".to_string();
    let c = TvConfig::default();
    let rows = vbox_core::sources::spider::list_videos(&c, &s, "1", 1).expect("live list");
    assert!(!rows.is_empty(), "live list empty");
    println!("live list first: {:?}", rows[0]);
    let d = vbox_core::sources::spider::get_detail(&c, &s, &rows[0].id).expect("live detail");
    println!("live detail: {} episodes={}", d.name, d.episodes.len());
    let hits = vbox_core::sources::spider::search(&c, &s, "海贼", false).expect("live search");
    assert!(!hits.is_empty(), "live search empty");
    println!("live search first: {:?}", hits[0]);
}

/// 真站：`csp_SP33` 走同一 360kan 官方 API，只是分类/列表/详情参数形态不同。
#[test]
#[ignore = "live network: api.web.360kan.com (SP33 variant)"]
fn live_sp33_variant_lists_and_details() {
    let mut s = Site::default();
    s.key = "sp33".to_string();
    s.name = "SP33".to_string();
    s.api = "csp_SP33".to_string();
    let c = TvConfig::default();

    let cats = vbox_core::sources::spider::list_categories(&c, &s).expect("live categories");
    assert!(!cats.is_empty(), "SP33 categories empty (fallback expected)");
    println!("sp33 categories: {:?}", cats.iter().take(4).collect::<Vec<_>>());

    let cat = cats[0].type_id.clone();
    let rows = vbox_core::sources::spider::list_videos(&c, &s, &cat, 1).expect("live list");
    assert!(!rows.is_empty(), "SP33 live list empty for cat {cat}");
    println!("sp33 list first: {:?}", rows[0]);

    let d = vbox_core::sources::spider::get_detail(&c, &s, &rows[0].id).expect("live detail");
    assert!(!d.name.is_empty(), "SP33 detail name empty");
    println!("sp33 detail: {} episodes={}", d.name, d.episodes.len());
}
