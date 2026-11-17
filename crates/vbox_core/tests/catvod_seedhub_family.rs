//! `csp_SeedHub`（🧲 SeedHub 磁力站）集成验收。
//!
//! 上游 `https://www.seedhub.cc` 已挂 Cloudflare「Just a moment…」（403），
//! `https://www.seedhub.info` 返回 JS 挑战页（200/约 1.6KB），无头环境不可达；
//! 因此用 tiny_http mock 走完整分派链路：分类文件夹、详情（磁力/外链）、播放计划。
//! 搜索依赖外部 Bing（`cn.bing.com`），集成测试不覆盖，纯解析函数由单测覆盖。

use std::io::Read;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content};

const FOLDER_BODY: &str = r##"<!doctype html><html><body>
<div class="content">
  <div class="cover">
    <a href="/movies/777/" title="沙丘2"><img src="/img/dune.jpg"></a>
    <li>评分: 8.7<a href="/movies/777/">详情</a></li>
  </div>
  <div class="cover">
    <a href="/movies/888/" title="奥本海默"><img src="/img/opp.jpg"></a>
    <li>评分: 8.9<a href="/movies/888/">详情</a></li>
  </div>
</div>
</body></html>"##;

const FOLDER_PAGE2_BODY: &str = r##"<!doctype html><html><body>
<div class="content">
  <div class="cover">
    <a href="/movies/999/" title="第二页条目"><img src="/img/p2.jpg"></a>
    <li>评分: 7.0<a href="/movies/999/">详情</a></li>
  </div>
</div>
</body></html>"##;

/// 「详情当分类」：`div.content>div.cover>img` + `ul.seeds > li`。
const MOVIE_BODY: &str = r##"<!doctype html><html><body>
<div class="content">
  <div class="cover"><img src="/img/dune.jpg" alt="沙丘2"></div>
  <ul class="seeds">
    <li><a href="magnet:?xt=urn:btih:aaa" title="4K">A</a></li>
    <li><a href="magnet:?xt=urn:btih:bbb" title="1080P">B</a></li>
  </ul>
</div>
</body></html>"##;

const MAGNET_BODY: &str = r##"<!doctype html><html><body>
<div class="content">
  <p class="link-start"><a href="magnet:?xt=urn:btih:ccc">磁力下载</a></p>
</div>
</body></html>"##;

const LINK_BODY: &str = r##"<!doctype html><html><body>
<div class="content">
  <p class="link-start"><a href="https://pan.example.com/s/abc">网盘</a></p>
</div>
</body></html>"##;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "seedhub-mock".into(),
        name: "SeedHub".into(),
        kind: 3,
        api: "csp_SeedHub".into(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

fn respond_text(request: tiny_http::Request, body: &str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("header");
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
            if url.contains("/categories/") {
                if url.contains("page=2") {
                    respond_text(request, FOLDER_PAGE2_BODY);
                } else {
                    respond_text(request, FOLDER_BODY);
                }
            } else if url.contains("/movies/magnet") {
                respond_text(request, MAGNET_BODY);
            } else if url.contains("/movies/link") {
                respond_text(request, LINK_BODY);
            } else if url.contains("/movies/") {
                respond_text(request, MOVIE_BODY);
            } else {
                respond_text(request, FOLDER_BODY);
            }
        }
    });
    addr
}

#[test]
fn dispatch_routes_seedhub_to_native_family() {
    let site = site("https://mock.invalid");
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&site.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&site.api).is_none());
    assert_eq!(
        vbox_core::spider::catvod::families::family_of(&site.api),
        Some("seedhub")
    );
}

#[test]
fn categories_are_hardcoded_three() {
    let cats = list_categories(&cfg(), &site("https://mock.invalid")).expect("分类");
    let ids: Vec<&str> = cats.iter().map(|c| c.type_id.as_str()).collect();
    assert_eq!(ids, vec!["1", "3", "2"]);
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(names, vec!["电影", "电视剧", "动漫"]);
}

#[test]
fn folder_category_list_through_dispatch() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "1", 1).expect("分类页");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "/movies/777/");
    assert_eq!(items[0].name, "沙丘2");
    let cover = items[0].cover.as_deref().unwrap_or("");
    assert!(cover.starts_with(&format!("{base}/img/dune.jpg")), "cover={cover}");
    assert!(cover.contains("@Headers="), "cover={cover}");
    assert_eq!(items[0].remark.as_deref(), Some("详情"));
    assert_eq!(items[1].id, "/movies/888/");
}

#[test]
fn folder_category_page_two_uses_page_query() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "1", 2).expect("第 2 页");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "/movies/999/");
    assert_eq!(items[0].name, "第二页条目");
}

#[test]
fn movies_category_returns_reversed_seed_rows() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "movies/777", 1).expect("详情当分类");
    assert_eq!(items.len(), 2);
    // 原类先把整组 li 倒序，故 1080P 在前。
    assert_eq!(items[0].name, "1080P");
    assert_eq!(items[0].id, format!("沙丘2$$${base}/img/dune.jpg$$$magnet:?xt=urn:btih:bbb"));
    assert_eq!(items[1].id, format!("沙丘2$$${base}/img/dune.jpg$$$magnet:?xt=urn:btih:aaa"));
}

#[test]
fn detail_magnet_becomes_magnet_episode() {
    let base = start_server();
    let id = format!("沙丘2$$$https://img.example.com/c.jpg$$$/movies/magnet");
    let detail = get_detail(&cfg(), &site(&base), &id).expect("详情");
    assert_eq!(detail.name, "沙丘2");
    assert_eq!(
        detail.cover.as_deref(),
        Some("https://img.example.com/c.jpg@Headers={\"User-Agent\":\"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/94.0.4606.54 Safari/537.36\"}")
    );
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "magnet");
    assert_eq!(detail.episodes[0].items[0].url, "magnet:?xt=urn:btih:ccc");
}

#[test]
fn detail_non_magnet_falls_back_to_page_url() {
    let base = start_server();
    let id = format!("某片$$$https://img.example.com/x.jpg$$$/movies/link");
    let detail = get_detail(&cfg(), &site(&base), &id).expect("详情");
    assert_eq!(detail.episodes[0].group, "SeedHub");
    assert_eq!(detail.episodes[0].items[0].url, format!("{base}/movies/link"));
}

#[test]
fn player_plan_matches_original_prefix_rules() {
    let base = start_server();
    let ed2k = player_content(&cfg(), &site(&base), "magnet", "ed2k://|file|a.mkv|1|H|/", &[])
        .expect("ed2k");
    assert_eq!(ed2k["parse"], json!(0));
    assert_eq!(ed2k["playUrl"], json!(""));
    let http = player_content(&cfg(), &site(&base), "magnet", "https://cdn.example.com/x.m3u8", &[])
        .expect("http");
    assert_eq!(http["parse"], json!(1));
    assert_eq!(http["url"], json!("https://cdn.example.com/x.m3u8"));
    let header = http["header"].as_str().unwrap_or("");
    assert!(header.contains("Android 13"), "header={header}");
}
