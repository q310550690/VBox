//! `csp_Kkys` 家族集成测试（tiny_http mock，不访问真实网络）。
//!
//! 覆盖：首页分类（跳过「推荐」）、分类页参数与分页字段、详情多线路与 `jp` 前缀、
//! 搜索、平台 referer 播放、`jqq-/rr-` 代理播放、空 id 与缺 `data.video` 错误。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};
use tiny_http::{Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

#[derive(Clone, Default)]
struct Recorded {
    url: String,
}

fn start_server() -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let server = Server::http("127.0.0.1:0").expect("mock server 可启动");
    let base = format!("http://{}", server.server_addr());
    let hits = Arc::new(Mutex::new(Vec::new()));
    let sink = hits.clone();
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            sink.lock().unwrap().push(Recorded { url: url.clone() });
            let payload: Value = if url.starts_with("/api.php/Index/getTopVideoCategory") {
                json!({"data": [
                    {"nav_type_id": "1", "nav_name": "推荐"},
                    {"nav_type_id": "2", "nav_name": "电影"},
                    {"nav_type_id": "3", "nav_name": "剧集"}
                ]})
            } else if url.starts_with("/api.php/Video/getFilterVideoList") {
                json!({"data": {"pagecount": 5, "list": [
                    {"vod_id": "10", "vod_name": "影片A", "vod_pic": "http://img/10.jpg", "vod_remarks": "更新"},
                    {"vod_id": "11", "vod_name": "", "vod_pic": "", "vod_remarks": ""}
                ]}})
            } else if url.starts_with("/api.php/Video/getVideoInfo") {
                if url.contains("video_id=missing") {
                    json!({"data": {}})
                } else {
                json!({"data": {"video": {
                    "vod_id": "10", "vod_name": "影片A", "vod_pic": "http://img/10.jpg",
                    "vod_year": "2024", "vod_area": "CN", "vod_remarks": "更新",
                    "vod_director": "导演", "vod_actor": "演员", "vod_content": " 剧情  简介 ",
                    "vod_play": [
                        {"playerForm": "jp", "url": [{"title": "第1集", "play_url": "http://x/1"}]},
                        {"playerForm": "jx", "url": [{"title": "第1集", "play_url": "http://y/1"}]}
                    ]
                }}})
                }
            } else if url.starts_with("/api.php/Search/getSearch") {
                json!({"data": {"list": [
                    {"vod_id": "21", "vod_name": "搜索结果", "vod_pic": "http://img/21.jpg", "vod_remarks": "HD"}
                ]}})
            } else if url.starts_with("/8520.php") {
                json!({"data": {"url": "https://cdn.example/real.m3u8"}})
            } else {
                json!({"data": {}})
            };
            let body = payload.to_string();
            let _ = request.respond(Response::from_string(body).with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
            ));
        }
    });
    (base, hits)
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    serde_json::from_value(json!({
        "key": "kkys", "name": "Kkys", "type": 3, "api": "csp_Kkys",
        "ext": format!("{base}|proxy={base}/8520.php?url=")
    }))
    .expect("site")
}

#[test]
fn home_categories_skip_recommend() {
    let (base, _) = start_server();
    let cats = spider::list_categories(&cfg(), &site(&base)).expect("分类可解析");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "2");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[1].type_name, "剧集");
}

#[test]
fn category_page_passes_params_and_reads_pagecount() {
    let (base, hits) = start_server();
    let items = spider::list_videos(&cfg(), &site(&base), "电影", 2).expect("列表可解析");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "10");
    assert_eq!(items[0].cover.as_deref(), Some("http://img/10.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("更新"));
    let urls = hits.lock().unwrap();
    let url = &urls.iter().find(|h| h.url.contains("getFilterVideoList")).expect("命中分类").url;
    assert!(url.contains("p=2"), "{url}");
    assert!(url.contains("class=fll---"), "{url}");
    assert!(url.contains("type=%E7%94%B5%E5%BD%B1"), "{url}");
}

#[test]
fn detail_groups_and_jp_prefix() {
    let (base, _) = start_server();
    let d = spider::get_detail(&cfg(), &site(&base), "10").expect("详情可解析");
    assert_eq!(d.name, "影片A");
    assert_eq!(d.year.as_deref(), Some("2024"));
    assert_eq!(d.area.as_deref(), Some("CN"));
    assert_eq!(d.desc.as_deref(), Some("剧情 简介"));
    assert_eq!(d.episodes.len(), 2);
    assert_eq!(d.episodes[0].group, "jp");
    assert_eq!(d.episodes[0].items[0].url, "tvbox-xg:http://x/1");
    assert_eq!(d.episodes[1].group, "jx");
}

#[test]
fn search_returns_items() {
    let (base, hits) = start_server();
    let items = spider::search(&cfg(), &site(&base), "测试 词", false).expect("搜索可解析");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "搜索结果");
    let urls = hits.lock().unwrap();
    let url = &urls.iter().find(|h| h.url.contains("getSearch")).expect("命中搜索").url;
    assert!(url.contains("key=%E6%B5%8B%E8%AF%95%20%E8%AF%8D"), "{url}");
}

#[test]
fn player_platform_sets_referer_and_parse_zero() {
    let (base, _) = start_server();
    let v = spider::player_content(
        &cfg(),
        &site(&base),
        "",
        "https://www.bilibili.com/video/BV1xx",
        &[],
    )
    .expect("平台播放可解析");
    assert_eq!(v["parse"], 0);
    assert_eq!(v["url"], "https://www.bilibili.com/video/BV1xx");
    assert_eq!(v["header"]["referer"], "https://www.bilibili.com");
    assert!(v["header"]["User-Agent"].as_str().unwrap().contains("Chrome/80"));
}

#[test]
fn player_jqq_uses_proxy_and_returns_real_url() {
    let (base, _) = start_server();
    let v = spider::player_content(&cfg(), &site(&base), "", "jqq-abc", &[]).expect("代理播放可解析");
    assert_eq!(v["parse"], 0);
    assert_eq!(v["url"], "https://cdn.example/real.m3u8");
    assert!(v["header"]["User-Agent"].as_str().unwrap().starts_with("Dalvik"));
}

#[test]
fn player_empty_id_is_error() {
    let (base, _) = start_server();
    let err = spider::player_content(&cfg(), &site(&base), "", "  ", &[]).expect_err("空 id 应报错");
    assert!(format!("{err}").contains("catvod.result.parse"));
}

#[test]
fn detail_without_video_is_parse_error() {
    let (base, _) = start_server();
    let err = spider::get_detail(&cfg(), &site(&base), "missing").expect_err("缺 data.video 应报错");
    assert!(format!("{err}").contains("catvod.result.parse"), "{err}");
}
