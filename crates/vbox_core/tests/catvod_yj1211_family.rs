//! `csp_Yj1211`（📺 YJ 直播聚合）集成验收。
//!
//! 上游 `live.yj1211.work` / `yj1211.work` / `www.yj1211.work` 全部 NXDOMAIN，
//! 无头环境不可达；因此用 tiny_http mock 走完整分派链路：平台分类、
//! 推荐列表（分页）、平台列表、房间详情（清晰度）、搜索、播放计划。

use std::io::Read;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};

const RECOMMEND_BODY: &str = r#"{"code":0,"data":[
  {"platForm":"douyu","roomId":"1","ownerName":"主播A","ownerHeadPic":"http://img.example.com/a.jpg","categoryName":"英雄联盟"},
  {"platForm":"bilibili","roomId":"2","ownerName":"主播B","ownerHeadPic":"http://img.example.com/b.jpg","categoryName":"守望先锋"}
]}"#;

const RECOMMEND_PAGE2_BODY: &str = r#"{"code":0,"data":[
  {"platForm":"huya","roomId":"3","ownerName":"主播C","ownerHeadPic":"http://img.example.com/c.jpg","categoryName":"DOTA2"}
]}"#;

const PLATFORM_BODY: &str = r#"{"code":0,"data":[
  {"platForm":"douyu","roomId":"9","ownerName":"斗鱼主播","categoryName":"绝地求生"}
]}"#;

const INFO_BODY: &str = r#"{"code":0,"data":{
  "roomName":"主播A直播间","roomPic":"http://img.example.com/room.jpg",
  "ownerName":"主播A","roomId":"1","platForm":"douyu",
  "isLive":"1","online":"1234","categoryName":"英雄联盟"
}}"#;

const INFO_EMPTY_STREAMS: &str = r#"{"code":0,"data":{
  "roomName":"空直播间","roomPic":"","ownerName":"主播X","roomId":"404",
  "platForm":"huya","isLive":"","online":"0","categoryName":"其他"
}}"#;

const URLS_BODY: &str = r#"{"data":{
  "OD":"http://cdn.example.com/original.m3u8","HD":"",
  "SD":"http://cdn.example.com/hd.m3u8","LD":"","FD":""
}}"#;

const URLS_EMPTY_BODY: &str = r#"{"data":{}}"#;

const SEARCH_BODY: &str = r#"{"code":0,"data":[
  {"nickName":"主播A","headPic":"http://img.example.com/h.jpg","cateName":"英雄联盟","platform":"douyu","roomId":"1"}
]}"#;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "yj1211-mock".into(),
        name: "YJ1211".into(),
        kind: 3,
        api: "csp_Yj1211".into(),
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
            if url.contains("/api/live/getRecommendByPlatform") {
                respond_text(request, PLATFORM_BODY);
            } else if url.contains("/api/live/getRecommend?") {
                if url.contains("page=2") {
                    respond_text(request, RECOMMEND_PAGE2_BODY);
                } else {
                    respond_text(request, RECOMMEND_BODY);
                }
            } else if url.contains("/api/live/getRoomInfo") {
                if url.contains("roomId=404") {
                    respond_text(request, INFO_EMPTY_STREAMS);
                } else {
                    respond_text(request, INFO_BODY);
                }
            } else if url.contains("/api/live/getRealUrl") {
                if url.contains("roomId=404") {
                    respond_text(request, URLS_EMPTY_BODY);
                } else {
                    respond_text(request, URLS_BODY);
                }
            } else if url.contains("/api/live/search") {
                respond_text(request, SEARCH_BODY);
            } else {
                respond_text(request, RECOMMEND_BODY);
            }
        }
    });
    addr
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_routes_yj1211_to_native_family() {
    let site = site("https://mock.invalid");
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&site.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&site.api).is_none());
    assert_eq!(
        vbox_core::spider::catvod::families::family_of(&site.api),
        Some("yj1211")
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_are_hardcoded_five() {
    let cats = list_categories(&cfg(), &site("https://mock.invalid")).expect("分类");
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(names, vec!["推荐", "斗鱼", "哔哩哔哩", "虎牙", "网易CC"]);
    let ids: Vec<&str> = cats.iter().map(|c| c.type_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "?",
            "ByPlatform?platform=douyu&",
            "ByPlatform?platform=bilibili&",
            "ByPlatform?platform=huya&",
            "ByPlatform?platform=cc&"
        ]
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn recommend_list_through_dispatch() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "?", 1).expect("推荐列表");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "douyu&1");
    assert_eq!(items[0].name, "主播A");
    assert_eq!(
        items[0].cover.as_deref(),
        Some("http://img.example.com/a.jpg")
    );
    assert_eq!(items[0].remark.as_deref(), Some("英雄联盟"));
    assert_eq!(items[1].id, "bilibili&2");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn recommend_page_two_uses_page_query() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "?", 2).expect("第 2 页");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "huya&3");
    assert_eq!(items[0].name, "主播C");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn platform_category_uses_platform_endpoint() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "ByPlatform?platform=douyu&", 1)
        .expect("平台列表");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "douyu&9");
    assert_eq!(items[0].name, "斗鱼主播");
    assert_eq!(items[0].remark.as_deref(), Some("绝地求生"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_joins_room_info_and_streams() {
    let base = start_server();
    let detail = get_detail(&cfg(), &site(&base), "douyu&1").expect("详情");
    assert_eq!(detail.name, "主播A直播间");
    assert_eq!(detail.area.as_deref(), Some("douyu"));
    assert_eq!(detail.episodes.len(), 1);
    let group = &detail.episodes[0];
    assert_eq!(group.group, "YJ1211");
    assert_eq!(group.items.len(), 2);
    assert_eq!(group.items[0].title, "原画");
    assert_eq!(group.items[0].url, "http://cdn.example.com/original.m3u8");
    assert_eq!(group.items[1].title, "高清");
    assert_eq!(group.items[1].url, "http://cdn.example.com/hd.m3u8");
    let desc = detail.desc.as_deref().unwrap_or("");
    assert!(desc.contains("正在直播中"), "desc={desc}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_without_streams_falls_back_to_nostream() {
    let base = start_server();
    let detail = get_detail(&cfg(), &site(&base), "huya&404").expect("详情");
    assert_eq!(detail.episodes[0].group, "YJ1211");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(detail.episodes[0].items[0].title, "NoStream");
    assert_eq!(detail.episodes[0].items[0].url, "nolink");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_rejects_malformed_id() {
    let base = start_server();
    let err = get_detail(&cfg(), &site(&base), "douyu").expect_err("非法 id");
    let text = format!("{err}");
    assert!(text.contains("planform") || text.contains("platform") || text.contains("id"), "err={text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_through_dispatch() {
    let base = start_server();
    let items = search(&cfg(), &site(&base), "主播", false).expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "douyu&1");
    assert_eq!(items[0].name, "主播A");
    assert_eq!(items[0].remark.as_deref(), Some("英雄联盟"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_returns_parse_one_with_header() {
    let base = start_server();
    let value = player_content(&cfg(), &site(&base), "YJ1211", "http://live.example.com/x.m3u8", &[])
        .expect("播放");
    assert_eq!(value["parse"], json!(1));
    assert_eq!(value["playUrl"], json!(""));
    assert_eq!(value["url"], json!("http://live.example.com/x.m3u8"));
    assert!(value["header"]["User-Agent"].as_str().unwrap().contains("YaBrowser"));
}
