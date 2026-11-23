//! `csp_LiteApple`（小苹果影视）集成验收。
//!
//! `item.xpgtv.com` 2026-11 仍在线（真站用例在文件末尾，默认 `#[ignore]`）；
//! 这里用 tiny_http mock 走完整分派链路：分类、列表、详情（无播放地址）、
//! 搜索（名称包含过滤）、播放（CDN 拼接 / 直链透传）。

use std::io::Read;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};

const TYPES_BODY: &str = r#"{"code":200,"msg":"success","data":[
  {"type_id":1,"type_name":"电影","classes":["动作片"],"areas":["大陆"],"years":["2026","2025"]},
  {"type_id":2,"type_name":"电视剧","classes":["国产剧"],"areas":["大陆"],"years":["2026"]}
]}"#;

const LIST_BODY: &str = r#"{"code":200,"msg":"success","data":[
  {"id":132221,"name":"另一个妈妈","pic":"http://jpg.example.com/1.jpg","year":"2026","updateInfo":"更新至1集","className":"恐怖"},
  {"id":132222,"name":"第二部","pic":"http://jpg.example.com/2.jpg","year":"2025","updateInfo":"全12集","className":"剧情"}
]}"#;

const LIST_PAGE2_BODY: &str = r#"{"code":200,"msg":"success","data":[
  {"id":132300,"name":"第三页首条","pic":"http://jpg.example.com/3.jpg","updateInfo":""}
]}"#;

const DETAIL_BODY: &str = r#"{"code":200,"msg":"success","data":{
  "uploader":"Rivulet","id":132221,"name":"另一个妈妈","pic":"http://jpg.example.com/1.jpg",
  "year":"2026","content":"  故事  讲述一个 女孩。 ","attention":0,
  "className":"恐怖","classType1":10,"classType2":1,"updateInfo":"更新至1集",
  "actor":"演员B","area":"大陆","director":"导演A","score":"6.5","type_id_1":1,"state":1,
  "urls":[{"type":1,"key":"蓝光","url":"V1RjSllm"},{"type":1,"key":"超清","url":"V2FiY2Q="}]
}}"#;

const DETAIL_NO_URLS: &str = r#"{"code":200,"msg":"success","data":{
  "id":404,"name":"已下架","pic":"","content":"x","urls":[]
}}"#;

const SEARCH_BODY: &str = r#"{"code":200,"msg":null,"data":[
  {"id":118651,"name":"三体","pic":"http://jpg.example.com/s1.jpg","updateInfo":"全30集"},
  {"id":118652,"name":"三体前传","pic":"http://jpg.example.com/s2.jpg","updateInfo":"全10集"},
  {"id":118653,"name":"狂飙","pic":"http://jpg.example.com/s3.jpg","updateInfo":"全39集"}
]}"#;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "liteapple-mock".into(),
        name: "小苹果".into(),
        kind: 3,
        api: "csp_LiteApple".into(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

fn respond_text(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header");
    let response = Response::from_string(body.to_string()).with_header(header);
    let _ = request.respond(response);
}

fn start_server() -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("mock server"));
    let addr = format!("http://{}", server.server_addr());
    let server_for_thread = Arc::clone(&server);
    thread::spawn(move || {
        for mut request in server_for_thread.incoming_requests() {
            let url = request.url().to_string();
            let mut body = String::new();
            let _ = request.as_reader().take(64 * 1024).read_to_string(&mut body);
            if url.contains("/api.php/v2.vod/androidtypes") {
                respond_text(request, TYPES_BODY);
            } else if url.contains("/api.php/v2.vod/androidfilter") {
                if url.contains("page=2") {
                    respond_text(request, LIST_PAGE2_BODY);
                } else {
                    respond_text(request, LIST_BODY);
                }
            } else if url.contains("/api.php/v3.vod/androiddetail2") {
                if url.contains("vod_id=404") {
                    respond_text(request, DETAIL_NO_URLS);
                } else {
                    respond_text(request, DETAIL_BODY);
                }
            } else if url.contains("/api.php/v2.vod/androidsearch10086") {
                respond_text(request, SEARCH_BODY);
            } else {
                respond_text(request, r#"{"code":200,"data":[]}"#);
            }
        }
    });
    addr
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_routes_liteapple_to_native_family() {
    let site = site("http://mock.invalid");
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&site.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&site.api).is_none());
    assert_eq!(
        vbox_core::spider::catvod::families::family_of(&site.api),
        Some("liteapple")
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_come_from_androidtypes() {
    let base = start_server();
    let cats = list_categories(&cfg(), &site(&base)).expect("分类");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[1].type_name, "电视剧");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn list_videos_maps_rows() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "1", 1).expect("列表");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "132221");
    assert_eq!(items[0].name, "另一个妈妈");
    assert_eq!(items[0].cover.as_deref(), Some("http://jpg.example.com/1.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("更新至1集"));
    assert_eq!(items[1].id, "132222");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn list_videos_page_two_keeps_working() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "1", 2).expect("第 2 页");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "132300");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_collects_urls_into_liteapple_group() {
    let base = start_server();
    let detail = get_detail(&cfg(), &site(&base), "132221").expect("详情");
    assert_eq!(detail.name, "另一个妈妈");
    assert_eq!(detail.year.as_deref(), Some("2026"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.director.as_deref(), Some("导演A"));
    assert_eq!(detail.actor.as_deref(), Some("演员B"));
    let desc = detail.desc.as_deref().unwrap_or("");
    assert!(desc.contains("故事 讲述一个 女孩。"), "desc={desc}");
    assert!(desc.contains("分类：恐怖"), "desc={desc}");
    assert_eq!(detail.episodes.len(), 1);
    let group = &detail.episodes[0];
    assert_eq!(group.group, "LiteApple");
    assert_eq!(group.items.len(), 2);
    assert_eq!(group.items[0].title, "蓝光");
    assert_eq!(group.items[0].url, "V1RjSllm");
    assert_eq!(group.items[1].title, "超清");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_without_urls_reports_parse_error() {
    let base = start_server();
    let err = get_detail(&cfg(), &site(&base), "404").expect_err("应当报错");
    let text = format!("{err}");
    assert!(text.contains("无播放地址"), "err={text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_filters_by_name_contains() {
    let base = start_server();
    let items = search(&cfg(), &site(&base), "三体", false).expect("搜索");
    assert_eq!(items.len(), 2, "狂飙 应被过滤掉");
    assert_eq!(items[0].id, "118651");
    assert_eq!(items[1].name, "三体前传");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_builds_cdn_url_and_header() {
    let base = start_server();
    let value = player_content(&cfg(), &site(&base), "LiteApple", "V1RjSllm", &[]).expect("播放");
    assert_eq!(value["parse"], 0);
    assert_eq!(value["url"], "http://c.xpgtv.net/m3u8/V1RjSllm.m3u8");
    let header = value["header"].as_str().unwrap_or("");
    assert!(header.contains("token2"), "header={header}");
    assert!(header.contains("XPGBOX com.phoenix.tv1.5.5"), "header={header}");

    let direct = player_content(
        &cfg(),
        &site(&base),
        "LiteApple",
        "http://cdn.example.com/a.m3u8",
        &[],
    )
    .expect("直链播放");
    assert_eq!(direct["url"], "http://cdn.example.com/a.m3u8");
}

/// 真站验收：分类 / 搜索 / 详情 / 播放（2026-11 上游在线，默认忽略）。
#[test]
#[ignore = "真站回归：item.xpgtv.com（2026-11 在线）"]
fn live_site_categories_search_detail_and_play() {
    let site = Site {
        key: "liteapple".into(),
        name: "小苹果".into(),
        kind: 3,
        api: "csp_LiteApple".into(),
        ext: None,
        ..Default::default()
    };
    let mut config = cfg();
    config.http.timeout = 60_000;

    let cats = list_categories(&config, &site).expect("真站分类");
    assert!(cats.len() >= 5, "分类偏少：{cats:?}");

    let items = search(&config, &site, "三体", false).expect("真站搜索");
    assert!(!items.is_empty(), "搜索 三体 无结果");
    assert!(items.iter().any(|i| i.name.contains("三体")));

    let first = items.first().expect("首条");
    let detail = get_detail(&config, &site, &first.id).expect("真站详情");
    assert!(!detail.name.is_empty());
    assert!(!detail.episodes.is_empty(), "详情无播放地址：{detail:?}");
    let token = &detail.episodes[0].items[0].url;

    let play = player_content(&config, &site, "LiteApple", token, &[]).expect("真站播放");
    let url = play["url"].as_str().unwrap_or("");
    assert!(url.contains(".m3u8"), "play.url={url}");
}
