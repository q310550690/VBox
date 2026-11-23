//! `csp_Douban` 家族集成测试（tiny_http mock + 1 个真站用例）。
//!
//! 覆盖：7 个固定分类、hot_gaia 列表（`items` 数组、`msearch:` 前缀、封面 `@Referer=` 后缀、
//! 评分备注、无封面条目过滤）、tv_hot 列表（`subject_collection_items` 数组）、
//! 详情/播放返回 `catvod.host.unsupported`、搜索为空。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const HOT_GAIA_JSON: &str = r#"{
  "items": [
    {
      "id": "36013664",
      "title": "流浪地球3",
      "pic": { "normal": "https://example.com/p1.jpg" },
      "rating": { "value": 8.2 }
    },
    {
      "id": "36013665",
      "title": "无名之辈2",
      "pic": { "normal": "https://example.com/p2.jpg" },
      "rating": { "value": 7.1 }
    },
    {
      "id": "36013666",
      "title": "没有封面的条目",
      "pic": { "normal": "" }
    }
  ]
}"#;

const TV_HOT_JSON: &str = r#"{
  "subject_collection_items": [
    {
      "id": "36210001",
      "title": "庆余年3",
      "pic": { "normal": "https://example.com/tv1.jpg" },
      "rating": { "value": 8.8 }
    }
  ]
}"#;

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).expect("header")
}

fn respond(request: tiny_http::Request, body: &str) {
    let _ = request.respond(Response::from_string(body).with_header(json_header()));
}

fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_handle = Arc::clone(&seen);
    let handle = Arc::clone(&server);
    thread::spawn(move || loop {
        let Ok(request) = handle.recv() else { return };
        let url = request.url().to_string();
        seen_handle.lock().unwrap().push(url.clone());
        if url.contains("/movie/hot_gaia") {
            respond(request, HOT_GAIA_JSON);
        } else if url.contains("/subject_collection/tv_hot/items") {
            respond(request, TV_HOT_JSON);
        } else {
            respond(request, "{}");
        }
    });
    (base, seen)
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "douban".into(),
        name: "豆瓣".into(),
        api: "csp_Douban".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

/// 豆瓣克隆 `csp_DouDou` 的站点（协议与 Douban 相同，仅条目 id 形态不同）。
fn doudou_site(base: &str) -> Site {
    let mut s = Site {
        key: "doudou".into(),
        name: "豆瓣克隆".into(),
        api: "csp_DouDou".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_are_fixed_seven() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    assert_eq!(cats.len(), 7);
    assert_eq!(cats[0].type_id, "hot_gaia");
    assert_eq!(cats[0].type_name, "热门电影");
    assert!(cats.iter().any(|c| c.type_id == "rank_list_tv" && c.type_name == "电视剧榜单"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn hot_gaia_lists_items_with_msearch_and_rating() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "hot_gaia", 1).expect("videos");
    assert_eq!(items.len(), 2, "无封面条目应被过滤：{items:?}");
    assert_eq!(items[0].id, "msearch:36013664");
    assert_eq!(items[0].name, "流浪地球3");
    let cover = items[0].cover.as_deref().expect("cover");
    assert!(cover.starts_with("https://example.com/p1.jpg@Referer="), "cover={cover}");
    assert_eq!(items[0].remark.as_deref(), Some("评分：8.2"));
    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().any(|u| u.contains("/movie/hot_gaia") && u.contains("start=0") && u.contains("count=20")), "{seen:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn tv_hot_uses_subject_collection_items() {
    let cfg = cfg();
    let (base, _) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "tv_hot", 1).expect("videos");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "msearch:36210001");
    assert_eq!(items[0].remark.as_deref(), Some("评分：8.8"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn second_page_uses_start_offset() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let _ = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "hot_gaia", 2).expect("videos");
    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().any(|u| u.contains("start=20")), "{seen:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_reports_unsupported_contract() {
    let cfg = cfg();
    let (base, _) = start_server();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "msearch:36013664")
        .expect_err("detail should fail");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.host.unsupported"), "msg={msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_is_empty() {
    let cfg = cfg();
    let (base, _) = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "海贼", false).expect("search");
    assert!(items.is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn doudou_is_native_and_uses_triplet_msearch_id() {
    let site = doudou_site("http://127.0.0.1:9");
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&site.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&site.api).is_none());
    let cfg = cfg();
    let (base, _) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &doudou_site(&base), "hot_gaia", 1).expect("doudou videos");
    assert_eq!(items.len(), 2, "无封面条目应被过滤：{items:?}");
    assert_eq!(items[0].id, "msearch:###流浪地球3###https://example.com/p1.jpg");
    assert_eq!(items[0].name, "流浪地球3");
    let cover = items[0].cover.as_deref().expect("cover");
    assert!(cover.starts_with("https://example.com/p1.jpg@Referer="), "cover={cover}");
    assert_eq!(items[0].remark.as_deref(), Some("评分：8.2"));
}

/// 真站用例：豆瓣小程序 API 公开可达，验证分类列表能取到条目。
#[test]
#[ignore]
fn live_douban_hot_gaia() {
    let cfg = cfg();
    let site = Site {
        key: "douban".into(),
        name: "豆瓣".into(),
        api: "csp_Douban".into(),
        ..Default::default()
    };
    let items = vbox_core::sources::spider::list_videos(&cfg, &site, "hot_gaia", 1).expect("live hot_gaia");
    assert!(!items.is_empty(), "豆瓣 hot_gaia 不应为空");
    println!("live douban first: {:?}", items.first());
}
