//! `csp_Meijumi` 家族集成测试（tiny_http mock + 真站回归）。
//!
//! 覆盖：首页 `ul.nav-menu` 导航 → 分类（过滤更新/推荐/首页/韩剧/即将）、
//! 列表页 `div#post_list_box article`（标题去「迅雷下载」、封面 `@Headers=` 标记）、
//! 分页 `{type_id}/page/{n}/`、详情（`.entry-title` + 首图 + magnet/ed2k 名称解码）、
//! 搜索 `/?s=`、播放分派（ed2k → parse 0，magnet → parse 1 + UA 头），
//! 以及空标题错误路径。上游 `www.meijumi.net` 在线，另有 1 个 `#[ignore]` 真站用例。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/94.0.4606.54 Safari/537.36";

fn home_html(base: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html><body>
<ul class="nav-menu">
  <li><a href="/">首页</a></li>
  <li><a href="{base}/usa/">美剧</a></li>
  <li><a href="{base}/usa/xuanyi/">灵异/惊悚</a></li>
  <li><a href="{base}/hanju/">韩剧</a></li>
  <li><a href="{base}/gengxin/">美剧更新时间表</a></li>
  <li><a href="{base}/zixun/haoju/">好剧推荐</a></li>
  <li><a href="javascript:void(0);">收藏本站</a></li>
</ul>
</body></html>"#
    )
}

fn list_html(base: &str, page2: bool) -> String {
    if page2 {
        return format!(
            r#"<!DOCTYPE html><html><body><div id="post_list_box">
  <article>
    <a href="{base}/50001.html" title="《第二页剧集》网盘/迅雷下载">
      <img src="/wp/page2.jpg" /><span class="gxts">更新至：第3集</span>
    </a>
  </article>
</div></body></html>"#
        );
    }
    format!(
        r#"<!DOCTYPE html><html><body><div id="post_list_box">
  <article>
    <a href="{base}/45785.html" title="《幻视任务第一季》VisionQuest 网盘/迅雷下载">
      <img src="https://tututu.wujinyuzhou.top/2026/10/huanshizhengtu_compressed.jpg" />
      <span class="gxts">更新至：全8集</span>
    </a>
  </article>
  <article>
    <a href="/12345.html"><img src="/wp/1.jpg" /><span class="gxts">完结</span></a>
  </article>
</div></body></html>"#
    )
}

const DETAIL_HTML: &str = r#"<!DOCTYPE html><html><body>
<h1 class="entry-title">《幻视任务第一季》VisionQuest 网盘/迅雷下载</h1>
<div class="single-content">
  <p><img src="https://tututu.wujinyuzhou.top/2026/10/huanshizhengtu_compressed.jpg" /></p>
  <p><a href="magnet:?xt=urn:btih:d7292586b0fef8a3d514c478fac2694c58eae90d&amp;dn=VisionQuest%20S01E01%2EPilot">E01</a></p>
  <p><a href="ed2k://|file|VisionQuest.S01E02.1080p.mkv|1234|HASH|/">E02</a></p>
  <p><a href="https://pan.xunlei.com/s/xyz">迅雷云盘</a></p>
</div>
</body></html>"#;

const DETAIL_BAD_HTML: &str = r#"<!DOCTYPE html><html><body><p>空页面</p></body></html>"#;

const SEARCH_HTML: &str = r#"<!DOCTYPE html><html><body><ul class="search-page">
  <article><a href="https://www.meijumi.net/45785.html" title="《幻视任务第一季》迅雷下载">
    <img src="/wp/a.jpg" /><span class="gxts">全8集</span></a></article>
</ul></body></html>"#;

fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header")
}

fn respond(request: tiny_http::Request, body: &str) {
    let _ = request.respond(Response::from_string(body).with_header(html_header()));
}

fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    let home = home_html(&base);
    let page1 = list_html(&base, false);
    let page2 = list_html(&base, true);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_handle = Arc::clone(&seen);
    let handle = Arc::clone(&server);
    thread::spawn(move || loop {
        let Ok(request) = handle.recv() else { return };
        let url = request.url().to_string();
        seen_handle.lock().unwrap().push(url.clone());
        if url == "/" {
            respond(request, &home);
        } else if url.contains("?s=") {
            respond(request, SEARCH_HTML);
        } else if url.contains("/bad.html") {
            respond(request, DETAIL_BAD_HTML);
        } else if url.contains("/45785.html") {
            respond(request, DETAIL_HTML);
        } else if url.contains("/usa/page/2/") {
            respond(request, &page2);
        } else if url.contains("/usa/") {
            respond(request, &page1);
        } else {
            respond(request, "<html><body>not found</body></html>");
        }
    });
    (base, seen)
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "meijumi".into(),
        name: "美剧迷".into(),
        api: "csp_Meijumi".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

fn live_site() -> Site {
    Site {
        key: "meijumi".into(),
        name: "美剧迷".into(),
        api: "csp_Meijumi".into(),
        ..Default::default()
    }
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
fn categories_come_from_nav_and_skip_non_categories() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(names, vec!["美剧", "灵异/惊悚"]);
    assert_eq!(cats[0].type_id, format!("{base}/usa/"));
    assert_eq!(cats[1].type_id, format!("{base}/usa/xuanyi/"));
}

#[test]
fn videos_parse_rows_and_mark_cover_headers() {
    let cfg = cfg();
    let (base, _) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), &format!("{base}/usa/"), 1)
        .expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, format!("{base}/45785.html"));
    assert_eq!(items[0].name, "《幻视任务第一季》VisionQuest 网盘/");
    assert_eq!(items[0].remark.as_deref(), Some("更新至：全8集"));
    let cover = items[0].cover.as_deref().unwrap_or("");
    assert!(
        cover.starts_with("https://tututu.wujinyuzhou.top/2026/10/huanshizhengtu_compressed.jpg@Headers={\"User-Agent\":\""),
        "cover={cover}"
    );
    assert!(cover.contains("Chrome/94.0.4606.54"), "cover={cover}");
    // 相对 href 被绝对化；相对封面走 join_url 后同样带 UA 标记。
    assert_eq!(items[1].id, format!("{base}/12345.html"));
    assert_eq!(
        items[1].cover.as_deref(),
        Some(format!("{base}/wp/1.jpg@Headers={{\"User-Agent\":\"{UA}\"}}").as_str())
    );
}

#[test]
fn videos_page_two_uses_page_segment() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), &format!("{base}/usa/"), 2)
        .expect("videos2");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "《第二页剧集》网盘/");
    assert_eq!(items[0].remark.as_deref(), Some("更新至：第3集"));
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u.contains("/usa/page/2/")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
fn detail_extracts_magnet_and_ed2k_names() {
    let cfg = cfg();
    let (base, _) = start_server();
    let id = format!("{base}/45785.html");
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), &id).expect("detail");
    assert_eq!(detail.name, "《幻视任务第一季》VisionQuest 网盘/");
    assert!(detail.cover.as_deref().unwrap_or("").contains("@Headers="));
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "美剧迷");
    let items = &detail.episodes[0].items;
    assert_eq!(items.len(), 2, "items={items:?}");
    assert_eq!(items[0].title, "VisionQuest S01E01.Pilot");
    assert!(items[0].url.starts_with("magnet:?xt=urn:btih:d7292586"));
    assert_eq!(items[1].title, "VisionQuest.S01E02.1080p.mkv");
    assert!(items[1].url.starts_with("ed2k://|file|"));
}

#[test]
fn detail_without_entry_title_reports_parse_error() {
    let cfg = cfg();
    let (base, _) = start_server();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), &format!("{base}/bad.html"))
        .expect_err("err");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.result.parse"), "unexpected: {msg}");
}

#[test]
fn search_uses_query_and_parses_rows() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "幻视", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "《幻视任务第一季》");
    assert_eq!(items[0].remark.as_deref(), Some("全8集"));
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|u| u.contains("?s=")), "unexpected urls: {seen:?}");
}

#[test]
fn player_splits_ed2k_direct_from_magnet_parsed() {
    let cfg = cfg();
    let (base, _) = start_server();
    let ed2k = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "",
        "ed2k://|file|VisionQuest.S01E02.1080p.mkv|1234|HASH|/",
        &[],
    )
    .expect("ed2k");
    assert_eq!(ed2k["parse"], 0);
    assert!(ed2k["url"].as_str().unwrap_or("").starts_with("ed2k://|file|"));

    let magnet = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "",
        "magnet:?xt=urn:btih:abc&dn=x",
        &[],
    )
    .expect("magnet");
    assert_eq!(magnet["parse"], 1);
    assert_eq!(magnet["header"]["User-Agent"], UA);
}

/// 真站回归：分类含「美剧」，`https://www.meijumi.net/usa/` 列表 ≥10 条，
/// 首条详情有标题与磁力/ed2k 线路，播放按 id 前缀分流。
#[test]
#[ignore]
fn live_meijumi_lists_and_details() {
    // 站点响应较慢：给真站用例单独放宽超时（默认 6s 易超时）
    let mut cfg = cfg();
    cfg.http.timeout = 60_000;
    let site = live_site();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("live categories");
    assert!(cats.iter().any(|c| c.type_name == "美剧"), "cats={cats:?}");

    let usa = cats
        .iter()
        .find(|c| c.type_name == "美剧")
        .expect("美剧分类")
        .type_id
        .clone();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site, &usa, 1).expect("live videos");
    assert!(items.len() >= 10, "len={}", items.len());
    assert!(items[0].cover.as_deref().unwrap_or("").contains("@Headers="));

    // 站点部分文章没有下载链接（如置顶的宣传文）：取前若干条直到找到带磁力的详情
    let mut checked = 0usize;
    let mut found = None;
    for item in items.iter().take(6) {
        let detail =
            vbox_core::sources::spider::get_detail(&cfg, &site, &item.id).expect("live detail");
        checked += 1;
        if !detail.episodes.is_empty() {
            found = Some(detail);
            break;
        }
    }
    let detail = found.unwrap_or_else(|| panic!("no detail with episodes in {checked} items"));
    assert!(!detail.name.is_empty(), "detail name empty");
    let ep = detail.episodes[0].items.first().cloned().expect("episode");
    assert!(
        ep.url.starts_with("magnet:") || ep.url.starts_with("ed2k:"),
        "url={}",
        ep.url
    );

    let value = vbox_core::sources::spider::player_content(&cfg, &site, "", &ep.url, &[]).expect("live player");
    let expected = if ep.url.starts_with("ed2k") { 0 } else { 1 };
    assert_eq!(value["parse"], expected);
    assert_eq!(value["url"].as_str().unwrap_or(""), ep.url);
}
