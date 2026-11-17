//! `csp_AppYsV2` 集成测试：App / Vod / Generic 三种基址形态的 mock 全链。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

fn start_server() -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("mock server"));
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let base_for_thread = base.clone();
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            let _ = base_for_thread;
            let (payload, status) = route(&url, "");
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ = request.respond(
                Response::from_string(payload).with_status_code(status).with_header(header),
            );
        }
    });
    base
}

fn route(url: &str, _body: &str) -> (String, u16) {
    if url.contains("nav?token=") {
        return (
            json!({"list": [{"type_id": "1", "type_name": "电影"}, {"type_id": "2", "type_name": "剧集"}]})
                .to_string(),
            200,
        );
    }
    if url.contains("/types") {
        return (
            json!({"data": [{"type_id": "10", "type_name": "动作片"}]}).to_string(),
            200,
        );
    }
    if url.contains("video_detail") {
        return (
            json!({"data": {
                "vod_name": "App 剧", "vod_pic": "https://img/a.jpg", "vod_content": "简介",
                "vod_url_with_player": [
                    {"code": "qq", "url": "https://cdn.test/1.m3u8"},
                    {"code": "qq", "url": "https://cdn.test/2.m3u8"},
                    {"name": "youku", "url": "https://cdn.test/3.m3u8"}
                ]
            }})
            .to_string(),
            200,
        );
    }
    if url.contains("/detail?vod_id=") {
        return (
            json!({"data": {
                "vod_name": "Vod 剧", "vod_pic": "https://img/v.jpg",
                "vod_play_list": [
                    {"player_info": {"from": "line1"}, "url": "第1集$https://cdn.test/v1.m3u8#第2集$https://cdn.test/v2.m3u8"}
                ]
            }})
            .to_string(),
            200,
        );
    }
    if url.contains("search?text=") || url.contains("?wd=") || url.contains("&zm=") {
        return (
            json!({"list": [{"vod_id": "s1", "vod_name": "搜索项", "vod_pic": "https://img/s.jpg", "vod_remarks": "HD"}]})
                .to_string(),
            200,
        );
    }
    if url.contains("video?tid=") || url.contains("?type=") || url.contains("ac=list") {
        return (
            json!({"totalpage": 2, "list": [
                {"vod_id": "9", "vod_name": "列表项", "vod_pic": "https://img/l.jpg", "vod_remarks": "更新"}
            ]})
            .to_string(),
            200,
        );
    }
    (json!({"list": []}).to_string(), 200)
}

fn cfg() -> TvConfig {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 20_000;
    cfg
}

fn site(base: &str, ext: &str) -> Site {
    let mut site = Site::default();
    site.key = "appysv2".into();
    site.name = "AppYsV2".into();
    site.api = "csp_AppYsV2".into();
    site.ext = Some(json!(format!("{base}{ext}")));
    site
}

#[test]
fn app_shape_full_chain() {
    let base = start_server();
    let s = site(&base, "/api.php/app/");

    let cats = spider::list_categories(&cfg(), &s).unwrap();
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_name, "电影");

    let videos = spider::list_videos(&cfg(), &s, "1", 2).unwrap();
    assert_eq!(videos.len(), 1);
    assert_eq!(videos[0].id, "9");

    let detail = spider::get_detail(&cfg(), &s, "9").unwrap();
    assert_eq!(detail.name, "App 剧");
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "qq");
    assert_eq!(detail.episodes[0].items.len(), 2);

    let hits = spider::search(&cfg(), &s, "海贼", false).unwrap();
    assert_eq!(hits[0].name, "搜索项");

    let play = spider::player_content(&cfg(), &s, "", "https://cdn.test/x.m3u8", &[]).unwrap();
    assert_eq!(play["parse"], 0);
}

#[test]
fn vod_shape_full_chain() {
    let base = start_server();
    let s = site(&base, "/api.php/v1.vod");

    let cats = spider::list_categories(&cfg(), &s).unwrap();
    assert_eq!(cats[0].type_id, "10");

    let videos = spider::list_videos(&cfg(), &s, "10", 1).unwrap();
    assert_eq!(videos[0].remark.as_deref(), Some("更新"));

    let detail = spider::get_detail(&cfg(), &s, "42").unwrap();
    assert_eq!(detail.name, "Vod 剧");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].title, "第2集");

    let hits = spider::search(&cfg(), &s, "剧", false).unwrap();
    assert_eq!(hits.len(), 1);

    let play = spider::player_content(&cfg(), &s, "line1", "https://v.qq.com/x/cover/a.html", &[]).unwrap();
    assert_eq!(play["parse"], 1);
}

#[test]
fn generic_shape_uses_fixed_categories_and_rejects_detail() {
    let base = start_server();
    let s = site(&base, "/api.php/v1/vod");

    let cats = spider::list_categories(&cfg(), &s).unwrap();
    assert_eq!(cats.len(), 6);
    assert_eq!(cats[0].type_name, "电影");

    let videos = spider::list_videos(&cfg(), &s, "movie", 1).unwrap();
    assert_eq!(videos.len(), 1);

    let err = spider::get_detail(&cfg(), &s, "1").unwrap_err().to_string();
    assert!(err.contains("没有独立详情接口"), "err={err}");
}

#[test]
fn ext_is_required() {
    let base = start_server();
    let mut s = site(&base, "");
    s.ext = Some(json!(""));
    let err = spider::list_categories(&cfg(), &s).unwrap_err().to_string();
    assert!(err.contains("ext"), "err={err}");
}

#[test]
fn legacy_appys_plain_base_shares_app_protocol() {
    // 旧版 AppYs 的 ext 是裸基址（没有 ###flag），协议与 AppYsV2 的 App 形态一致。
    let base = start_server();
    let mut s = site(&base, "/api.php/app/");
    s.api = "csp_AppYs".into();
    s.name = "AppYs".into();

    let cats = spider::list_categories(&cfg(), &s).unwrap();
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_name, "电影");

    let videos = spider::list_videos(&cfg(), &s, "1", 1).unwrap();
    assert_eq!(videos[0].name, "列表项");

    let detail = spider::get_detail(&cfg(), &s, "42").unwrap();
    assert_eq!(detail.name, "App 剧");
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].items.len(), 2);

    let hits = spider::search(&cfg(), &s, "剧", false).unwrap();
    assert_eq!(hits.len(), 1);

    let play = spider::player_content(&cfg(), &s, "", "https://cdn.test/9.m3u8", &[]).unwrap();
    assert_eq!(play["parse"], 0, "直链应 parse=0");
}
