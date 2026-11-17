//! `csp_AppMao`（萌米 App，fantaiying「appto」框架）集成验收。
//!
//! 上游 5 个 ext 的 domain 全部不可达（lmapp.org / mervod / COS 桶 404），
//! 因此这里用 tiny_http mock 走完整分派链路，并把「配置加密信封」纳入验收：
//! mock 返回 `{"ENCRYPTION":1,"data":{...}}` 明文配置，家族侧应直接解出。
//! 详情/播放链路：`vod/getVod` → 剧集 `url@vod_id` → `parsing/proxy`。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};

const CONFIG_BODY: &str = r#"{"ENCRYPTION":1,"data":{
 "get_home_cate":[
  {"mold":"1","cate":"1","title":"电影"},
  {"mold":"1","cate":"2","title":"电视剧"}
 ],
 "get_parsing":{"lists":[{"label":"解析一"},{"label":"解析二"}]}
}}"#;

const LIST_BODY: &str = r#"{"code":0,"data":{"list":[
 {"vod_id":11,"vod_name":"列一","vod_pic":"/img/a.jpg","vod_remarks":"更新至 1 集"},
 {"id":12,"name":"列二","vod_pic_thumb":"https://cdn/b.jpg"}
]}}"#;

const DETAIL_BODY: &str = r#"{"code":0,"data":{
 "vod_id":11,"vod_name":"列一","vod_pic":"/img/a.jpg","vod_year":"2024",
 "vod_area":"大陆","vod_remarks":"全 1 集","vod_content":"<p>剧情&amp;简介</p>",
 "vod_play_list":[{"title":"线路一","player_info":{"show":"解析二"},
   "urls":[{"name":"第1集","url":"https://v/1.m3u8"}]}]
}}"#;

const PARSING_BODY: &str = r#"{"url":"https://cdn/resolved.m3u8"}"#;

fn respond(request: tiny_http::Request, body: &str, status: u16) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header");
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header);
    let _ = request.respond(response);
}

fn start_server() -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || loop {
        let Ok(request) = server.recv() else {
            return;
        };
        let url = request.url().to_string();
        if url.contains("/config/get") {
            respond(request, CONFIG_BODY, 200);
        } else if url.contains("/vod/getLists") || url.contains("/vod/getVodSearch") {
            respond(request, LIST_BODY, 200);
        } else if url.contains("/parsing/proxy") {
            respond(request, PARSING_BODY, 200);
        } else if url.contains("/vod/getVod") {
            respond(request, DETAIL_BODY, 200);
        } else {
            respond(request, "{\"error\":\"not found\"}", 404);
        }
    });
    base
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

/// ext 用明文 JSON（`decode_ext` 失败后回退裸 JSON），host 指向 mock。
fn site(base: &str) -> Site {
    let ext = json!({
        "host": base,
        "key": "0123456789abcdef0123456789abcdef",
        "iv": "1238389483762837",
        "token": "tk",
        "__ic": "ic",
        "configV2": false,
        "isV4": false
    });
    Site {
        key: "appmao-mock".to_string(),
        name: "萌米App".to_string(),
        kind: 3,
        api: "csp_AppMao".to_string(),
        ext: Some(json!(ext.to_string())),
        ..Default::default()
    }
}

#[test]
fn dispatch_routes_appmao_to_native_family() {
    assert!(vbox_core::spider::catvod::is_native_ready("csp_AppMao"));
    assert!(vbox_core::spider::catvod::blocked_reason("csp_AppMao").is_none());
}

#[test]
fn categories_come_from_config() {
    let base = start_server();
    let cats = list_categories(&cfg(), &site(&base)).expect("categories");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[1].type_id, "2");
}

#[test]
fn list_videos_absolutises_relative_covers() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "1", 1).expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "11");
    assert_eq!(items[0].name, "列一");
    assert_eq!(items[0].cover.as_deref(), Some(format!("{base}/img/a.jpg").as_str()));
    assert_eq!(items[0].remark.as_deref(), Some("更新至 1 集"));
    assert_eq!(items[1].cover.as_deref(), Some("https://cdn/b.jpg"));
}

#[test]
fn detail_maps_play_lines_and_strips_html() {
    let base = start_server();
    let detail = get_detail(&cfg(), &site(&base), "11").expect("detail");
    assert_eq!(detail.name, "列一");
    assert!(detail.desc.as_deref().unwrap_or_default().contains("剧情&简介"));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "线路一");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(detail.episodes[0].items[0].url, "https://v/1.m3u8@11");
}

#[test]
fn search_maps_rows() {
    let base = start_server();
    let items = search(&cfg(), &site(&base), "测试", false).expect("search");
    assert_eq!(items.len(), 2);
    assert_eq!(items[1].id, "12");
}

#[test]
fn player_resolves_through_parsing_proxy() {
    let base = start_server();
    let play = player_content(&cfg(), &site(&base), "线路一", "https://v/1.m3u8@11", &[])
        .expect("play");
    assert_eq!(play.get("url").and_then(|v| v.as_str()), Some("https://cdn/resolved.m3u8"));
    assert_eq!(play.get("parse").and_then(|v| v.as_i64()), Some(0));
}

#[test]
fn player_requires_at_key_separator() {
    let base = start_server();
    let err = player_content(&cfg(), &site(&base), "线路一", "no-separator", &[]).unwrap_err();
    assert!(err.to_string().contains("@key"), "{err}");
}
