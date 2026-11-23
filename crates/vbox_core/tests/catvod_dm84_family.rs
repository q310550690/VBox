//! `csp_Dm84` 原生家族集成测试（本地 mock + 可选真站）。
//!
//! 覆盖：首页分类、分类列表（真实路由 `/show-{id}------{page}.html`）、详情（`og:*` 元数据 +
//! 线路/分集配对）、搜索、播放页 `iframe[src]`，以及 `sources::spider` 分派接线。

use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};
use vbox_core::model::{Site, TvConfig};

const HOME_HTML: &str = r#"
<ul class="nav_row">
  <li class="current"><a href="/" title="首页">首页</a></li>
  <li><a href="/list-1.html" title="国产">国漫</a></li>
  <li><a href="/list-2.html" title="日本">日漫</a></li>
  <li><a href="/list-3.html" title="欧美">欧美动漫</a></li>
  <li><a href="/list-4.html" title="电影">电影</a></li>
</ul>
<div class="item">
  <a href="/v/494.html" class="cover lazy" data-bg="http://img.test/a.jpg" title="逆天至尊">逆天至尊</a>
  <a class="title" href="/v/494.html" title="逆天至尊">逆天至尊</a>
  <span class="desc">第565话</span>
</div>"#;

const LIST_HTML: &str = r#"
<div class="item">
  <a href="/v/494.html" class="cover lazy" data-bg="http://img.test/a.jpg" title="逆天至尊">逆天至尊</a>
  <a class="title" href="/v/494.html" title="逆天至尊">逆天至尊</a>
  <span class="desc">第565话</span>
</div>
<div class="item">
  <a href="/v/28.html" class="cover lazy" data-bg="/img/b.jpg" title="无上神帝">无上神帝</a>
  <a class="title" href="/v/28.html" title="无上神帝">无上神帝</a>
  <span class="desc">第656话</span>
</div>"#;

const DETAIL_HTML: &str = r#"
<h1 class="v_title"><a href="/v/494.html" title="逆天至尊在线播放">逆天至尊</a></h1>
<p class="v_desc"><span class="desc">第565话</span><em class="hr">|</em>2021<em class="hr">|</em>大陆</p>
<meta property="og:image" content="/upload/494.jpg"/>
<meta property="og:description" content="整个宇宙分为域内宇宙和域外宇宙"/>
<meta name="og:video:area" content="大陆"/>
<meta name="og:video:actor" content="阿旦,糖醋里脊"/>
<meta name="og:video:director" content="高峰"/>
<meta name="og:video:release_date" content="2021"/>
<ul class="tab_control play_from"><li class="current">线路1</li></ul>
<div class="tab_content" id="play_list">
  <ul class="play_list current">
    <li><a href="/p/494-1-565.html">565</a></li>
    <li><a href="/p/494-1-564.html">564</a></li>
  </ul>
</div>"#;

const SEARCH_HTML: &str = r#"
<div class="item">
  <a href="/v/6.html" class="cover lazy" data-bg="http://img.test/c.jpg" title="灵剑尊">灵剑尊</a>
  <a class="title" href="/v/6.html" title="灵剑尊">灵剑尊</a>
  <span class="desc">第648话</span>
</div>"#;

const PLAYER_HTML: &str = r#"<html><body>
<iframe src="https://hhjx.hhplayer.com/?url=ABC123" width="100%" height="100%"></iframe>
</body></html>"#;

fn start_server() -> String {
    let server = Server::http("127.0.0.1:0").expect("mock server");
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let body = if url == "/" {
                HOME_HTML.to_string()
            } else if url.starts_with("/show-") {
                LIST_HTML.to_string()
            } else if url.starts_with("/v/") {
                DETAIL_HTML.to_string()
            } else if url.starts_with("/s-") {
                SEARCH_HTML.to_string()
            } else if url.starts_with("/p/") {
                PLAYER_HTML.to_string()
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
        key: "dm84".to_string(),
        name: "巴士动漫".to_string(),
        api: "csp_Dm84".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_come_from_nav_row() {
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&TvConfig::default(), &site(&base))
        .expect("分类");
    assert_eq!(cats.len(), 4, "实际: {cats:?}");
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "国漫");
    assert_eq!(cats[3].type_name, "电影");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn list_uses_live_show_route() {
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(
        &TvConfig::default(),
        &site(&base),
        "1",
        2,
    )
    .expect("列表");
    assert_eq!(items.len(), 2, "实际: {items:?}");
    assert_eq!(items[0].id, "494.html");
    assert_eq!(items[0].name, "逆天至尊");
    assert_eq!(items[0].remark.as_deref(), Some("第565话"));
    assert_eq!(items[0].cover.as_deref(), Some("http://img.test/a.jpg"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_parses_og_meta_and_playlists() {
    let base = start_server();
    let d = vbox_core::sources::spider::get_detail(
        &TvConfig::default(),
        &site(&base),
        "494.html",
    )
    .expect("详情");
    assert_eq!(d.name, "逆天至尊");
    assert_eq!(d.year.as_deref(), Some("2021"));
    assert_eq!(d.area.as_deref(), Some("大陆"));
    assert_eq!(d.actor.as_deref(), Some("阿旦,糖醋里脊"));
    assert_eq!(d.director.as_deref(), Some("高峰"));
    assert_eq!(d.desc.as_deref(), Some("整个宇宙分为域内宇宙和域外宇宙"));
    assert!(d.cover.as_deref().unwrap().ends_with("/upload/494.jpg"));
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "线路1");
    assert_eq!(d.episodes[0].items.len(), 2);
    assert_eq!(d.episodes[0].items[0].title, "565");
    assert_eq!(d.episodes[0].items[0].url, "/p/494-1-565.html");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_returns_rows() {
    let base = start_server();
    let items = vbox_core::sources::spider::search(
        &TvConfig::default(),
        &site(&base),
        "灵剑",
        false,
    )
    .expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "6.html");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_extracts_iframe_src() {
    let base = start_server();
    let v = vbox_core::sources::spider::player_content(
        &TvConfig::default(),
        &site(&base),
        "线路1",
        "/p/494-1-565.html",
        &[],
    )
    .expect("播放");
    assert_eq!(v["url"], json!("https://hhjx.hhplayer.com/?url=ABC123"));
    assert_eq!(v["parse"], json!(0));
    assert_eq!(v["header"]["Referer"], json!(format!("{base}/")));
}

/// 真站回归（默认忽略；`cargo test -p vbox_core --test catvod_dm84_family -- --ignored`）。
#[test]
#[ignore]
fn live_dm84_smoke() {
    let cfg = TvConfig::default();
    let site = Site {
        key: "dm84".to_string(),
        name: "巴士动漫".to_string(),
        api: "csp_Dm84".to_string(),
        ..Default::default()
    };

    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("真站分类");
    assert!(cats.len() >= 3, "分类过少: {cats:?}");
    println!("live categories = {cats:?}");

    let items = vbox_core::sources::spider::list_videos(&cfg, &site, &cats[0].type_id, 1)
        .expect("真站列表");
    assert!(items.len() >= 10, "列表过少: {}", items.len());
    println!("live list first = {:?}", items[0]);

    let d = vbox_core::sources::spider::get_detail(&cfg, &site, &items[0].id).expect("真站详情");
    assert!(!d.name.is_empty(), "详情缺标题");
    assert!(!d.episodes.is_empty(), "详情缺线路");
    println!(
        "live detail = name={:?} year={:?} area={:?} groups={} eps={}",
        d.name,
        d.year,
        d.area,
        d.episodes.len(),
        d.episodes[0].items.len()
    );

    let found = vbox_core::sources::spider::search(&cfg, &site, "海贼", false).expect("真站搜索");
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
                Response::from_string("<html><body>garbage, not dm84</body></html>")
                    .with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap()),
            );
        }
    });
    let cfg = TvConfig::default();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "bogus.html")
        .expect_err("垃圾响应应报错");
    let msg = err.to_string();
    assert!(msg.contains("catvod."), "错误缺少稳定码: {msg}");
}
