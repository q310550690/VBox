//! `csp_FirstAid` 原生家族集成测试（本地 mock + 可选真站）。
//!
//! 覆盖：写死 8 分类、按区块下标的列表、内嵌 `window.__INITIAL_DATA__` 详情、
//! 旧模板 DOM 兜底、搜索结果、播放直链，以及 `sources::spider` 分派接线。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};

fn block_html(idx: usize) -> String {
    format!(
        r#"
        <img class="block100" src="https://img.test/cat{idx}.png">
        <div class="jj-title-li">
          <ul class="list-flex-br">
            <li class="list-br3"><a href="/jijiu/article/block{idx}-a.html"><div class="line-clamp1">第{idx}块条目A</div></a></li>
            <li class="list-br3"><a href="/jijiu/article/block{idx}-b.html"><div class="line-clamp1">第{idx}块条目B</div></a></li>
          </ul>
        </div>"#
    )
}

fn home_html() -> String {
    (0..8).map(block_html).collect::<String>()
}

const DETAIL_HTML: &str = r#"<html><head><meta charset="utf-8"></head><body>
<script>window.__INITIAL_DATA__={"videoDetailData":{"id":"399920","title":"中暑如何处理","lit_pic":"https://file.test/a.jpeg","content":"<p>中暑根据程度不同。</p><br/>2、重症中暑","media_url":"https://vod.test/a-sd.mp4","format_add_time":"2023-03-21 00:00:05","doctor":{"name":"吕有凯"}},"seo":{}};window.__USE_SSR__=true;</script>
</body></html>"#;

const LEGACY_DETAIL_HTML: &str = r#"<html><body>
<h1 class="video-title h1-title">心肺复苏</h1>
<div class="video-cover list-flex-in"><img src="//file.test/b.jpeg"></div>
<span class="doc-name">张三</span>
<div class="img-text-con">按压深度 5 厘米</div>
<video id="video"><source src="https://vod.test/b.mp4"></video>
</body></html>"#;

const SEARCH_HTML: &str = r#"
<div class="search-video-li list-br2">
  <a href="/video/article/399920.html">
    <dl>
      <dt class="logo-bg"><img src="//file.test/c.jpeg"></dt>
      <dd><h5 class="line-clamp1">中暑如何处理</h5><p>吕有凯 副主任医师</p></dd>
    </dl>
  </a>
</div>"#;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let body = if url == "/jijiu" {
                home_html()
            } else if url.contains("legacy") {
                LEGACY_DETAIL_HTML.to_string()
            } else if url.starts_with("/video/") {
                DETAIL_HTML.to_string()
            } else if url.starts_with("/cse/search") {
                SEARCH_HTML.to_string()
            } else {
                String::from("<html></html>")
            };
            let _ = req.respond(
                Response::from_string(body).with_header(
                    Header::from_bytes(&b"Content-Type"[..], b"text/html; charset=utf-8").unwrap(),
                ),
            );
        }
    });
    base
}

fn site(base: &str) -> Site {
    Site {
        key: "firstaid".to_string(),
        name: "有来急救".to_string(),
        api: "csp_FirstAid".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_are_eight_fixed() {
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&TvConfig::default(), &site(&base))
        .expect("分类");
    assert_eq!(cats.len(), 8);
    assert_eq!(cats[0].type_id, "jijiu|0");
    assert_eq!(cats[2].type_name, "急危重症");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn list_dispatches_by_block_index() {
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(
        &TvConfig::default(),
        &site(&base),
        "jijiu|3",
        1,
    )
    .expect("列表");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].name, "第3块条目A");
    assert_eq!(items[0].id, format!("{base}/jijiu/article/block3-a.html"));
    assert_eq!(items[0].cover.as_deref(), Some("https://img.test/cat3.png"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_prefers_initial_data_json() {
    let base = start_server();
    let d = vbox_core::sources::spider::get_detail(
        &TvConfig::default(),
        &site(&base),
        &format!("{base}/video/article/399920.html"),
    )
    .expect("详情");
    assert_eq!(d.name, "中暑如何处理");
    assert_eq!(d.actor.as_deref(), Some("吕有凯"));
    assert_eq!(d.year.as_deref(), Some("2023"));
    assert_eq!(d.cover.as_deref(), Some("https://file.test/a.jpeg"));
    assert_eq!(d.episodes[0].items[0].url, "https://vod.test/a-sd.mp4");
    assert!(d.desc.as_deref().unwrap().contains("中暑根据程度不同"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_falls_back_to_legacy_dom() {
    let base = start_server();
    let d = vbox_core::sources::spider::get_detail(
        &TvConfig::default(),
        &site(&base),
        &format!("{base}/legacy/1.html"),
    )
    .expect("旧模板详情");
    assert_eq!(d.name, "心肺复苏");
    assert_eq!(d.actor.as_deref(), Some("张三"));
    assert_eq!(d.cover.as_deref(), Some("https://file.test/b.jpeg"));
    assert_eq!(d.episodes[0].items[0].url, "https://vod.test/b.mp4");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_returns_rows() {
    let base = start_server();
    let items =
        vbox_core::sources::spider::search(&TvConfig::default(), &site(&base), "中暑", false)
            .expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "中暑如何处理");
    assert_eq!(items[0].cover.as_deref(), Some("https://file.test/c.jpeg"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_returns_direct_url() {
    let base = start_server();
    let v = vbox_core::sources::spider::player_content(
        &TvConfig::default(),
        &site(&base),
        "Qile",
        "https://vod.test/a-sd.mp4",
        &[],
    )
    .expect("播放");
    assert_eq!(v["url"], json!("https://vod.test/a-sd.mp4"));
    assert_eq!(v["parse"], json!(0));
}

/// 真站回归（默认忽略；`cargo test -p vbox_core --test catvod_firstaid_family -- --ignored`）。
#[test]
#[ignore]
fn live_firstaid_smoke() {
    let cfg = TvConfig::default();
    let site = Site {
        key: "firstaid".to_string(),
        name: "有来急救".to_string(),
        api: "csp_FirstAid".to_string(),
        ..Default::default()
    };

    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("真站分类");
    assert_eq!(cats.len(), 8);

    let items = vbox_core::sources::spider::list_videos(&cfg, &site, &cats[0].type_id, 1)
        .expect("真站列表");
    assert!(items.len() >= 5, "列表过少: {}", items.len());
    println!("live list first = {:?}", items[0]);

    let d = vbox_core::sources::spider::get_detail(&cfg, &site, &items[0].id).expect("真站详情");
    assert!(!d.name.is_empty(), "详情缺标题");
    assert!(!d.episodes.is_empty(), "详情缺播放地址");
    println!(
        "live detail = name={:?} actor={:?} year={:?} play={:?}",
        d.name,
        d.actor,
        d.year,
        d.episodes[0].items[0].url
    );

    let found = vbox_core::sources::spider::search(&cfg, &site, "中暑", false).expect("真站搜索");
    assert!(!found.is_empty(), "搜索无结果");
    println!("live search first = {:?}", found[0]);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn garbage_upstream_returns_stable_catvod_error() {
    let server = Server::http("127.0.0.1:0").expect("mock");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let _ = req.respond(
                Response::from_string("<html><body>garbage, not youlai</body></html>")
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
