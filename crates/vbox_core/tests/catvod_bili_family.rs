//! `csp_Bili` 家族集成测试：tiny_http mock 三接口（view / playurl / search），
//! 经 `sources::spider` 公共分派验证端到端；另含 `#[ignore]` 真站回归。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

const VIEW_JSON: &str = r#"{"code":0,"data":{"title":"测试视频","pic":"//i0.hdslb.com/p.jpg","tname":"动画","desc":"<p>简介</p>","duration":125,"pages":[{"cid":11,"part":"P1 标题"},{"cid":12,"part":"P2 标题"}]}}"#;
const PLAY_JSON: &str = r#"{"code":0,"data":{"durl":[{"url":"https://upos.example.test/a/b.m4s","size":123,"length":456}]}}"#;
const SEARCH_JSON: &str = r#"{"code":0,"data":{"result":[{"aid":77,"title":"<em>窗</em>白噪音","pic":"//i0.hdslb.com/s.jpg","duration":"12:34"},{"aid":78,"title":"雨声","pic":"http://i0.hdslb.com/r.jpg","duration":"05:00"}]}}"#;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let body = if url.starts_with("/x/web-interface/view") {
                VIEW_JSON
            } else if url.starts_with("/x/player/playurl") {
                PLAY_JSON
            } else if url.starts_with("/x/web-interface/search/type") {
                SEARCH_JSON
            } else {
                "{}"
            };
            let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ = req.respond(Response::from_string(body).with_header(header));
        }
    });
    addr
}

fn site(base: &str) -> Site {
    let mut site = Site {
        key: "bili".to_string(),
        name: "哔哩哔哩".to_string(),
        api: "csp_Bili".to_string(),
        ..Default::default()
    };
    site.ext = Some(json!(format!(
        r#"{{"class":[{{"type_id":"1","type_name":"动画"}}],"cookie":"sid=abc","apiBase":"{base}"}}"#
    )));
    site
}

#[test]
fn categories_come_from_ext() {
    let cfg = TvConfig::default();
    let cats = spider::list_categories(&cfg, &site(&start_server())).unwrap();
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "动画");
}

#[test]
fn videos_go_through_search_api() {
    let cfg = TvConfig::default();
    let items = spider::list_videos(&cfg, &site(&start_server()), "1", 1).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "77");
    assert_eq!(items[0].name, "窗白噪音");
    assert_eq!(items[0].cover.as_deref(), Some("https://i0.hdslb.com/s.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("12分钟"));
    assert_eq!(items[1].id, "78");
}

#[test]
fn search_maps_results() {
    let cfg = TvConfig::default();
    let items = spider::search(&cfg, &site(&start_server()), "白噪音", false).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1].name, "雨声");
}

#[test]
fn detail_maps_view_and_pages() {
    let cfg = TvConfig::default();
    let detail = spider::get_detail(&cfg, &site(&start_server()), "42").unwrap();
    assert_eq!(detail.name, "测试视频");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "B站");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[0].url, "42+ 11");
    assert_eq!(detail.episodes[0].items[1].title, "P2 标题");
    assert_eq!(detail.cover.as_deref(), Some("https://i0.hdslb.com/p.jpg"));
}

#[test]
fn player_returns_durl_with_referer_header() {
    let cfg = TvConfig::default();
    let value = spider::player_content(&cfg, &site(&start_server()), "B站", "42+ 12", &[]).unwrap();
    assert_eq!(value["url"], "https://upos.example.test/a/b.m4s");
    assert_eq!(value["parse"], json!(0));
    assert_eq!(value["header"]["Referer"], "https://www.bilibili.com");
}

#[test]
fn player_accepts_bare_aid_and_resolves_cid() {
    let cfg = TvConfig::default();
    let value = spider::player_content(&cfg, &site(&start_server()), "B站", "42", &[]).unwrap();
    assert_eq!(value["url"], "https://upos.example.test/a/b.m4s");
}

#[test]
fn unknown_video_reports_parse_error() {
    // 用一个不存在的 api 名验证不会误入 bili 家族。
    let cfg = TvConfig::default();
    let mut s = site(&start_server());
    s.api = "csp_BiliGuard".to_string();
    let err = spider::search(&cfg, &s, "x", false).unwrap_err();
    let text = format!("{err:?}");
    assert!(text.contains("catvod.jar.protected") || text.contains("catvod."), "{text}");
}

#[test]
#[ignore = "需要外网访问 api.bilibili.com"]
fn live_view_and_playurl() {
    let cfg = TvConfig::default();
    let mut s = Site {
        key: "bili".to_string(),
        name: "哔哩哔哩".to_string(),
        api: "csp_Bili".to_string(),
        ..Default::default()
    };
    s.ext = Some(json!("{}"));
    let detail = spider::get_detail(&cfg, &s, "170001").unwrap();
    assert!(!detail.name.is_empty(), "{detail:?}");
    assert!(!detail.episodes[0].items.is_empty(), "{detail:?}");
    let url = &detail.episodes[0].items[0].url;
    let play = spider::player_content(&cfg, &s, "B站", url, &[]).unwrap();
    let p = play["url"].as_str().unwrap_or_default();
    assert!(p.starts_with("http"), "{play}");
}
