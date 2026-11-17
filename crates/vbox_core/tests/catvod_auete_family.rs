//! `csp_Auete` 家族集成测试（tiny_http mock）。
//!
//! 覆盖：首页导航分类、首页列表、分类列表第 1/2 页 URL、详情（og meta +
//! `span.detail-label` + 两条播放线路，已知线路在前）、空详情错误路径、搜索、
//! 播放（`base64decode(now)` 直链）。
//!
//! 上游原基址 `auete.art` 已 ENOTFOUND、`auete.pro` 超时；现役站
//! `www.auete.com` 的搜索入口被 WAF 挡住，因此真站只做分类/列表/详情/播放的
//! `#[ignore]` 冒烟（`live_site_smoke`）。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const HOME_HTML: &str = r#"<!DOCTYPE html><html><body>
<ul class="navbar-nav mr-auto">
  <li class="nav-item home"><a class="nav-link" href="/">首页</a></li>
  <li class="nav-item"><a class="nav-link" href="/Movie/index.html">电影</a></li>
  <li class="nav-item"><a class="nav-link" href="/Tv/index.html">电视剧</a></li>
  <li class="nav-item"><a class="nav-link" href="/Zy/index.html">综艺</a></li>
  <li class="nav-item"><a class="nav-link" href="/Dm/index.html">动漫</a></li>
  <li class="nav-item"><a class="nav-link" href="/qita/index.html">其他</a></li>
  <li class="nav-item"><a class="nav-link" href="/go/auete.html">地址发布页</a></li>
</ul>
<ul class="threadlist">
  <li data-href="/Dm/donghua/baxian_/" data-tid="43357">
    <a href="/Dm/donghua/baxian_/" class="pic"><img src="https://img.test/home.jpg"
        alt="八仙！"><span class="hdtag">HD国语</span></a>
    <h2><a href="/Dm/donghua/baxian_/" title="八仙！">八仙！</a></h2>
  </li>
</ul>
</body></html>"#;

const LIST_HTML: &str = r#"<!DOCTYPE html><html><body>
<ul class="threadlist">
  <li data-href="/Dm/donghua/baxian_/" data-tid="43357">
    <a href="/Dm/donghua/baxian_/" class="pic"><img src="https://img.test/a.jpg"
        alt="八仙！"><span class="hdtag">HD国语</span></a>
    <h2><a href="/Dm/donghua/baxian_/" title="八仙！">八仙！</a></h2>
  </li>
  <li data-href="/Dm/donghua/another_/" data-tid="1">
    <a href="/Dm/donghua/another_/" class="pic"><img src="/upload/b.jpg"
        alt="另一部"><button class="btn">更新至10集</button></a>
    <h2><a href="/Dm/donghua/another_/" title="另一部 迅雷下载">另一部 迅雷下载</a></h2>
  </li>
</ul>
<nav class="mb-4"><ul class="pagination">
  <li class="page-item"><a href="/Dm/index.html" class="page-link">首页</a></li>
  <li class="page-item active"><a href="/Dm/index.html" class="page-link">1</a></li>
  <li class="page-item"><a href="/Dm/index2.html" class="page-link">2</a></li>
  <li class="page-item"><a href="/Dm/index5.html" class="page-link">尾页</a></li>
</ul></nav>
</body></html>"#;

const LIST_PAGE2_HTML: &str = r#"<!DOCTYPE html><html><body>
<ul class="threadlist">
  <li data-href="/Dm/donghua/third_/" data-tid="2">
    <a href="/Dm/donghua/third_/" class="pic"><img src="https://img.test/c.jpg"
        alt="第三部"><span class="hdtag">全12集</span></a>
    <h2><a href="/Dm/donghua/third_/" title="第三部">第三部</a></h2>
  </li>
</ul>
<nav class="mb-4"><ul class="pagination">
  <li class="page-item"><a href="/Dm/index.html" class="page-link">上一页</a></li>
  <li class="page-item active"><a href="/Dm/index2.html" class="page-link">2</a></li>
  <li class="page-item"><a href="/Dm/index5.html" class="page-link">尾页</a></li>
</ul></nav>
</body></html>"#;

const DETAIL_HTML: &str = r#"<!DOCTYPE html><html><head>
<meta property="og:title" content="八仙！">
<meta property="og:image" content="https://img.test/cover.jpg">
<meta property="og:description" content="八个仙人过海的故事。">
<meta property="og:video:area" content="大陆">
<meta property="og:video:director" content="某导演">
<meta property="og:video:actor" content="甲,乙">
</head><body>
<div class="message break-all">
  <p><span class="detail-label">◎影片片名：</span><b>八仙！</b></p>
  <p><span class="detail-label">◎上映年份：</span><b>2026-07-18</b></p>
</div>
<div class="card mb-3 play-card" id="player_list">
  <div class="card-header"><span><b>『八仙！』云播J线</b></span></div>
  <div class="card-body"><ul class="episode-list">
    <li id="00"><a title="正片国语" href="/Dm/donghua/baxian_/play-0-0.html">正片国语</a></li>
  </ul></div>
</div>
<div class="card mb-3 play-card" id="player_list">
  <div class="card-header"><span><b>『八仙！』云播F线</b></span></div>
  <div class="card-body"><ul class="episode-list">
    <li id="10"><a title="正片国语" href="/Dm/donghua/baxian_/play-1-0.html">正片国语</a></li>
    <li id="11"><a title="第2集" href="/Dm/donghua/baxian_/play-1-1.html">第2集</a></li>
  </ul></div>
</div>
</body></html>"#;

const DETAIL_EMPTY_HTML: &str = r#"<!DOCTYPE html><html><body><p>没有标题</p></body></html>"#;

const SEARCH_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="card-body"><ul>
  <li class="media"><img src="https://img.test/s1.jpg">
    <div class="media-body">
      <div class="subject"><a href="/Dm/donghua/baxian_/"><span>八仙！</span></a></div>
      <div class="d-flex"><div class="text-muted"><span>2026 · 动漫</span></div></div>
    </div>
  </li>
</ul></div>
</body></html>"#;

const PLAY_HTML: &str = r#"<!DOCTYPE html><html><body>
<script>var vid="43357";var vfrom="0";var vpart="0"; var now=base64decode("aHR0cHM6Ly9oZC5rdWt0eHUuY29tL3BsYXkvYjJrZ3laamQvaW5kZXgubTN1OA==");var pn="kvd";</script>
</body></html>"#;

fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header")
}

fn respond_html(request: tiny_http::Request, body: &str) {
    let _ = request.respond(Response::from_string(body).with_header(html_header()));
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
        if url.contains("/play-0-0.html") {
            respond_html(request, PLAY_HTML);
        } else if url.contains("/auete4so.php") {
            respond_html(request, SEARCH_HTML);
        } else if url.contains("/baxian_/") {
            respond_html(request, DETAIL_HTML);
        } else if url.contains("/index2.html") {
            respond_html(request, LIST_PAGE2_HTML);
        } else if url.contains("/index.html") || url.contains("/Dm/") {
            respond_html(request, LIST_HTML);
        } else if url.contains("/gone") {
            respond_html(request, DETAIL_EMPTY_HTML);
        } else if url == "/" {
            respond_html(request, HOME_HTML);
        } else {
            respond_html(request, "<html><body>not found</body></html>");
        }
    });
    (base, seen)
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "auete".into(),
        name: "Auete".into(),
        api: "csp_Auete".into(),
        ..Default::default()
    };
    s.ext = Some(json!(base));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
fn categories_parse_nav_and_skip_non_sections() {
    let cfg = cfg();
    let (base, _) = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("categories");
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(names, vec!["电影", "电视剧", "综艺", "动漫", "其他"]);
    assert_eq!(cats[3].type_id, "Dm");
}

#[test]
fn home_lists_recommendations() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "", 1).expect("home");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "Dm/donghua/baxian_");
    assert_eq!(items[0].name, "八仙！");
    assert_eq!(items[0].remark.as_deref(), Some("HD国语"));
    assert_eq!(items[0].cover.as_deref(), Some("https://img.test/home.jpg"));
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|u| u == "/"), "unexpected urls: {seen:?}");
}

#[test]
fn videos_page_one_hits_index_html() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "Dm", 1).expect("videos");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "八仙！");
    assert_eq!(items[1].id, "Dm/donghua/another_");
    assert_eq!(items[1].name, "另一部");
    assert_eq!(items[1].remark.as_deref(), Some("更新至10集"));
    let expected_cover = format!("{base}/upload/b.jpg");
    assert_eq!(items[1].cover.as_deref(), Some(expected_cover.as_str()));
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u == "/Dm/index.html"),
        "unexpected urls: {seen:?}"
    );
}

#[test]
fn videos_page_two_hits_index_n_html() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "Dm", 2).expect("page2");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "第三部");
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u == "/Dm/index2.html"),
        "unexpected urls: {seen:?}"
    );
}

#[test]
fn detail_merges_lines_with_known_line_first() {
    let cfg = cfg();
    let (base, _) = start_server();
    let detail =
        vbox_core::sources::spider::get_detail(&cfg, &site(&base), "Dm/donghua/baxian_")
            .expect("detail");
    assert_eq!(detail.id, "Dm/donghua/baxian_");
    assert_eq!(detail.name, "八仙！");
    assert_eq!(detail.cover.as_deref(), Some("https://img.test/cover.jpg"));
    assert_eq!(detail.desc.as_deref(), Some("八个仙人过海的故事。"));
    assert_eq!(detail.year.as_deref(), Some("2026-07-18"));
    assert_eq!(detail.area.as_deref(), Some("大陆"));
    assert_eq!(detail.director.as_deref(), Some("某导演"));
    assert_eq!(detail.actor.as_deref(), Some("甲,乙"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "云播F线");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[1].group, "云播J线");
    assert_eq!(
        detail.episodes[1].items[0].url,
        "/Dm/donghua/baxian_/play-0-0.html"
    );
}

#[test]
fn detail_without_title_errors() {
    let cfg = cfg();
    let (base, _) = start_server();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "gone")
        .expect_err("should fail");
    let text = format!("{err:?}");
    assert!(text.contains("详情页缺少标题"), "unexpected error: {text}");
}

#[test]
fn search_parses_media_rows() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "八仙", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "Dm/donghua/baxian_");
    assert_eq!(items[0].name, "八仙！");
    assert_eq!(items[0].remark.as_deref(), Some("2026 · 动漫"));
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u.contains("/auete4so.php")),
        "unexpected urls: {seen:?}"
    );
}

#[test]
fn player_decodes_base64_now() {
    let cfg = cfg();
    let (base, seen) = start_server();
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "",
        "/Dm/donghua/baxian_/play-0-0.html",
        &[],
    )
    .expect("player");
    assert_eq!(
        value.get("url").and_then(|v| v.as_str()),
        Some("https://hd.kuktxu.com/play/b2kgyZjd/index.m3u8")
    );
    assert_eq!(value.get("parse").and_then(|v| v.as_i64()), Some(0));
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|u| u == "/Dm/donghua/baxian_/play-0-0.html"),
        "unexpected urls: {seen:?}"
    );
}

/// 真站冒烟：现役 `www.auete.com` 的分类 / 列表 / 详情 / 播放。
/// 需要外网；`cargo test -- --ignored` 才会跑。
#[test]
#[ignore]
fn live_site_smoke() {
    let cfg = cfg();
    let live = Site {
        key: "auete".into(),
        name: "Auete".into(),
        api: "csp_Auete".into(),
        ..Default::default()
    };
    let cats = vbox_core::sources::spider::list_categories(&cfg, &live).expect("categories");
    let names: Vec<&str> = cats.iter().map(|c| c.type_name.as_str()).collect();
    assert!(names.contains(&"动漫"), "unexpected categories: {names:?}");

    let items = vbox_core::sources::spider::list_videos(&cfg, &live, "Dm", 1).expect("videos");
    assert!(!items.is_empty(), "分类列表为空");
    assert!(items.iter().any(|i| i.id.contains('/')), "列表 id 异常");

    let first = items
        .iter()
        // 站点子分类会调整（2026-11-14 实测 /Dm/ 首页已是 riman/guoman，donghua 下架），
        // 因此不绑定具体子分类：取任意「三级相对路径」条目即可覆盖列表→详情链路。
        .find(|i| i.id.starts_with("Dm/") && i.id.matches('/').count() >= 2)
        .expect("没有详情 id");
    let detail = vbox_core::sources::spider::get_detail(&cfg, &live, &first.id).expect("detail");
    assert!(!detail.name.is_empty());
    assert!(!detail.episodes.is_empty(), "详情没有播放线路");

    let play = &detail.episodes[0].items[0];
    let value = vbox_core::sources::spider::player_content(&cfg, &live, "", &play.url, &[])
        .expect("player");
    let url = value.get("url").and_then(|v| v.as_str()).unwrap_or_default();
    assert!(url.starts_with("http"), "unexpected player url: {url}");
}
