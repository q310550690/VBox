//! `csp_Anime1` 原生家族集成测试（本地 mock；上游索引域名已 DNS 失效，暂无真站用例）。
//!
//! 覆盖：固定分类、索引 JSON 解析与分类过滤、年份归档页详情（`data-apireq` → 集名；
//! 搜索、播放接口（POST 表单 + `Set-Cookie` → 播放头），以及 `sources::spider` 分派接线。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Method, Response, Server};
use vbox_core::model::{Site, TvConfig};

const INDEX_JSON: &str = r#"[
  ["https://anime1.me/2024/01/a", "甲番", "12", "2024", "10", "组A"],
  ["https://anime1.me/2023/02/b", "乙番", "3", "2023", "1", "组B"],
  ["https://anime1.me/1990/03/c", "老番", "1", "1990", "4", "组C"]
]"#;

const CAT_HTML: &str = r#"
<article>
  <h2><a href="https://anime1.me/2024/01/a">[12] 甲番</a></h2>
  <div class="vjscontainer"><video data-apireq="req-12"></video></div>
</article>
<article>
  <h2><a href="https://anime1.me/2024/01/sp">特别篇</a></h2>
  <div class="vjscontainer">
    <video data-apireq="req-sp1"></video>
    <video data-apireq="req-sp2"></video>
  </div>
</article>"#;

const PLAY_JSON: &str = r#"{"s":[{"src":"//cdn.example.test/hls/12.m3u8"}]}"#;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let is_post = req.method() == &Method::Post;
            let (body, ctype) = if url.starts_with("/index") {
                (INDEX_JSON.to_string(), "application/json")
            } else if url.starts_with("/api") && is_post {
                (PLAY_JSON.to_string(), "application/json")
            } else if url.contains("cat=") {
                (CAT_HTML.to_string(), "text/html")
            } else {
                (String::from("<html></html>"), "text/html")
            };
            let mut resp = Response::from_string(body).with_header(
                Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap(),
            );
            if url.starts_with("/api") {
                for cookie in ["sid=a; Path=/", "cid=b; Path=/", "did=c; Path=/"] {
                    resp = resp.with_header(
                        Header::from_bytes(&b"Set-Cookie"[..], cookie.as_bytes()).unwrap(),
                    );
                }
            }
            let _ = req.respond(resp);
        }
    });
    base
}

fn site(base: &str) -> Site {
    Site {
        key: "anime1".to_string(),
        name: "Anime1".to_string(),
        api: "csp_Anime1".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_are_fixed_eight() {
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&TvConfig::default(), &site(&base))
        .expect("分类");
    assert_eq!(cats.len(), 8);
    assert_eq!(cats[0].type_id, "0");
    assert_eq!(cats[0].type_name, "最近更新");
    assert_eq!(cats[7].type_id, "7");
    assert_eq!(cats[7].type_name, "更早");
    assert_eq!(cats[1].type_id, "1");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn latest_category_lists_index_rows() {
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(
        &TvConfig::default(),
        &site(&base),
        "0",
        1,
    )
    .expect("列表");
    assert_eq!(items.len(), 3, "实际: {items:?}");
    assert_eq!(items[0].id, "https://anime1.me/2024/01/a");
    assert_eq!(items[0].name, "甲番");
    assert_eq!(items[0].remark.as_deref(), Some("12"));
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://sta.anicdn.com/playerImg/8.jpg")
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn older_category_filters_by_year_threshold() {
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(
        &TvConfig::default(),
        &site(&base),
        "7",
        1,
    )
    .expect("更早");
    assert_eq!(items.len(), 1, "实际: {items:?}");
    assert_eq!(items[0].name, "老番");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_builds_single_episode_group() {
    let base = start_server();
    let d = vbox_core::sources::spider::get_detail(
        &TvConfig::default(),
        &site(&base),
        "https://anime1.me/2024/01/a",
    )
    .expect("详情");
    assert_eq!(d.name, "甲番");
    assert_eq!(d.year.as_deref(), Some("2024"));
    assert_eq!(d.area.as_deref(), Some("日本"));
    assert_eq!(d.actor.as_deref(), Some("组A"));
    assert_eq!(d.desc.as_deref(), Some("10"));
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "Anime1");
    let titles: Vec<&str> = d.episodes[0].items.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, vec!["012", "特别篇01", "特别篇02"]);
    assert_eq!(d.episodes[0].items[1].url, "req-sp1");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_matches_name_locally() {
    let base = start_server();
    let items = vbox_core::sources::spider::search(
        &TvConfig::default(),
        &site(&base),
        "甲",
        false,
    )
    .expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "https://anime1.me/2024/01/a");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_posts_form_and_returns_cdn_url_with_cookies() {
    let base = start_server();
    let v = vbox_core::sources::spider::player_content(
        &TvConfig::default(),
        &site(&base),
        "Anime1",
        "req-12",
        &[],
    )
    .expect("播放");
    assert_eq!(v["parse"], json!(0));
    assert_eq!(v["url"], json!("https://cdn.example.test/hls/12.m3u8"));
    assert_eq!(v["header"]["cookie"], json!("sid=a; cid=b; did=c"));
    assert_eq!(v["header"]["Authority"], json!("cdn.example.test"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn garbage_upstream_returns_stable_catvod_error() {
    let server = Server::http("127.0.0.1:0").expect("mock");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let _ = req.respond(
                Response::from_string("<html><body>garbage, not anime1</body></html>")
                    .with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap()),
            );
        }
    });
    let cfg = TvConfig::default();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "bogus")
        .expect_err("垃圾响应应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod."), "错误缺少稳定码: {msg}");
}
