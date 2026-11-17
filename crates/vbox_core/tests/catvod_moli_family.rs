//! `csp_Moli` 家族集成测试（tiny_http mock + 真站 `#[ignore]`）。
//!
//! 覆盖：首页导航分类、列表第 1/2 页、详情（标题/封面/简介，网盘链接过滤后的兜底）、
//! 搜索页、播放错误映射、详情缺标题错误映射。
//!
//! 真站用例默认 `#[ignore]`（现役 `https://www.hdmoli.com`，需要外网）。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const HOME_HTML: &str = r#"<html><body>
<ul class="myui-header__menu">
  <li><a href="/mlist/index1.html">电影</a></li>
  <li><a href="/mlist/index2.html">剧集</a></li>
  <li><a href="/mlist/index41.html">动画</a></li>
</ul></body></html>"#;

const LIST_HTML: &str = r#"<html><body><ul class="myui-vodlist clearfix">
<li class="col-lg-5"><div class="myui-vodlist__box">
  <a class="myui-vodlist__thumb" href="/movie/index3638.html" title="幻视任务">
    <img class="vod-poster" src="https://img.hdmoli.com/1.jpg">
    <span class="pic-text text-right">8集全</span>
  </a>
  <div class="myui-vodlist__detail"><h4 class="title"><a href="/movie/index3638.html" title="幻视任务">幻视任务</a></h4></div>
</div></li>
<li class="col-lg-5"><div class="myui-vodlist__box">
  <a class="myui-vodlist__thumb" href="/movie/index3639.html" title="奥德赛">
    <img src="https://img.hdmoli.com/2.jpg"><span class="pic-text">HD</span>
  </a>
</div></li>
</ul></body></html>"#;

const LIST2_HTML: &str = r#"<html><body><ul class="myui-vodlist clearfix">
<li class="col-lg-5"><div class="myui-vodlist__box">
  <a class="myui-vodlist__thumb" href="/movie/index3700.html" title="第二页影片">
    <img src="https://img.hdmoli.com/3.jpg"><span class="pic-text">更新至10集</span>
  </a>
</div></li>
</ul></body></html>"#;

const DETAIL_HTML: &str = r#"<html><body>
<div class="myui-content__thumb"><img class="vod-poster" src="https://img.hdmoli.com/cover.jpg"></div>
<div class="myui-content__detail">
  <h1 class="title text-fff">幻视任务</h1>
  <div class="myui-content__desc">简介文本：一桩超自然案件。</div>
</div>
<ul class="myui-panel"><li><a href="https://pan.quark.cn/s/1d4747fa10ca">夸克网盘</a></li></ul>
</body></html>"#;

const SEARCH_HTML: &str = r#"<html><body><ul class="myui-vodlist clearfix">
<li class="col-lg-5"><div class="myui-vodlist__box">
  <a class="myui-vodlist__thumb" href="/movie/index4000.html" title="幻视任务 第二季">
    <img src="https://img.hdmoli.com/4.jpg"><span class="pic-text">更新至3集</span>
  </a>
</div></li>
</ul></body></html>"#;

const NO_TITLE_HTML: &str = r#"<html><body><div class="myui-panel">系统提示......</div></body></html>"#;

fn respond_html(request: tiny_http::Request, body: &'static str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("header");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

fn start_server() -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let base = format!("http://{}", server.server_addr());
    thread::spawn(move || loop {
        let Ok(request) = server.recv() else {
            return;
        };
        let url = request.url().to_string();
        if url == "/" {
            respond_html(request, HOME_HTML);
        } else if url.starts_with("/mlist/index1-2.html") {
            respond_html(request, LIST2_HTML);
        } else if url.starts_with("/mlist/index1.html") {
            respond_html(request, LIST_HTML);
        } else if url.starts_with("/movie/index3638.html") {
            respond_html(request, DETAIL_HTML);
        } else if url.starts_with("/movie/gone.html") {
            respond_html(request, NO_TITLE_HTML);
        } else if url.starts_with("/search.php") {
            respond_html(request, SEARCH_HTML);
        } else {
            respond_html(request, "<html><body>404</body></html>");
        }
    });
    base
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    let mut s = Site {
        key: "moli".into(),
        name: "HDmoli".into(),
        api: "csp_Moli".into(),
        ..Default::default()
    };
    s.ext = Some(json!(format!("./lib/token.json$$${base}/$$$null$$$1")));
    s
}

#[test]
fn categories_come_from_nav() {
    let cfg = cfg();
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("cats");
    assert_eq!(cats.len(), 3);
    assert_eq!(cats[0].type_id, "/mlist/index1.html");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[2].type_name, "动画");
}

#[test]
fn videos_first_page_parses_rows() {
    let cfg = cfg();
    let base = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "/mlist/index1.html", 1)
            .expect("list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "幻视任务");
    assert!(items[0].id.ends_with("/movie/index3638.html"), "id={}", items[0].id);
    assert_eq!(items[0].cover.as_deref(), Some("https://img.hdmoli.com/1.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("8集全"));
    assert_eq!(items[1].name, "奥德赛");
    assert_eq!(items[1].remark.as_deref(), Some("HD"));
}

#[test]
fn videos_second_page_uses_dash_route() {
    let cfg = cfg();
    let base = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg, &site(&base), "/mlist/index1.html", 2)
            .expect("list");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "第二页影片");
}

#[test]
fn detail_parses_meta_and_falls_back_when_pan_unexpandable() {
    let cfg = cfg();
    let base = start_server();
    let detail =
        vbox_core::sources::spider::get_detail(&cfg, &site(&base), "/movie/index3638.html")
            .expect("detail");
    assert_eq!(detail.name, "幻视任务");
    assert_eq!(detail.cover.as_deref(), Some("https://img.hdmoli.com/cover.jpg"));
    assert!(detail
        .desc
        .as_deref()
        .unwrap_or_default()
        .contains("超自然"));
    // mock 环境无法访问真实夸克接口：应降级为「每条资源一集」或空集，但不报错。
    assert!(detail.episodes.len() <= 1, "episodes={:?}", detail.episodes);
}

#[test]
fn detail_missing_title_maps_to_parse_error() {
    let cfg = cfg();
    let base = start_server();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "/movie/gone.html")
        .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.result.parse"), "err={err:?}");
}

#[test]
fn search_parses_rows() {
    let cfg = cfg();
    let base = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "幻视", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "幻视任务 第二季");
    assert_eq!(items[0].remark.as_deref(), Some("更新至3集"));
}

#[test]
fn player_rejects_invalid_share_link() {
    let cfg = cfg();
    let base = start_server();
    let err = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "flag",
        "https://example.com/not-a-pan-link",
        &[],
    )
    .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.host.unsupported"), "err={err:?}");
}

/// 真站冒烟：现役 `https://www.hdmoli.com` 的分类 + 列表。
#[test]
#[ignore = "需要外网访问 https://www.hdmoli.com"]
fn live_site_categories_and_list() {
    let cfg = cfg();
    let mut site = Site {
        key: "moli-live".into(),
        name: "HDmoli".into(),
        api: "csp_Moli".into(),
        ..Default::default()
    };
    site.ext = None;
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("live cats");
    assert!(cats.iter().any(|c| c.type_name == "电影"), "cats={cats:?}");
    let first = &cats[0];
    let items = vbox_core::sources::spider::list_videos(&cfg, &site, &first.type_id, 1)
        .expect("live list");
    assert!(!items.is_empty(), "live list empty for {first:?}");
}

/// 真站全链路：列表 → 详情 → 网盘分享 → W9 引擎展开分集。
///
/// 这是「HTML 站点 + 网盘引擎」这条**组合链路**唯一的真站证据：
/// mock 只能分别证明「详情页解析」和「网盘 API 调用」，而这里验证两者拼起来确实产出分集。
/// 站点前几条可能挂的是其它网盘（无引擎/不可匿名），因此最多尝试前 5 条，命中一条即可。
#[test]
#[ignore = "需要外网访问 https://www.hdmoli.com 且其详情页挂有可匿名浏览的夸克分享"]
fn live_site_detail_expands_pan_episodes() {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 60_000;
    let mut site = Site {
        key: "moli-live".into(),
        name: "HDmoli".into(),
        api: "csp_Moli".into(),
        ..Default::default()
    };
    site.ext = None;
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site).expect("live cats");
    let cat = cats.iter().find(|c| c.type_name.contains("电影")).unwrap_or(&cats[0]);
    let items = vbox_core::sources::spider::list_videos(&cfg, &site, &cat.type_id, 1)
        .expect("live list");
    assert!(!items.is_empty(), "live list empty for {cat:?}");

    let mut checked = Vec::new();
    for item in items.iter().take(5) {
        checked.push(item.id.clone());
        let Ok(detail) = vbox_core::sources::spider::get_detail(&cfg, &site, &item.id) else {
            continue;
        };
        let expanded: Vec<_> = detail
            .episodes
            .iter()
            .filter(|g| g.items.iter().any(|e| e.url.contains("###")))
            .collect();
        if !expanded.is_empty() {
            let episodes = expanded.iter().map(|g| g.items.len()).sum::<usize>();
            assert!(episodes > 0, "引擎展开后分集为空：{checked:?}");
            return;
        }
    }
    panic!("前 5 条详情均未产生「分享 + 引擎展开」分集（站点结构或网盘挂载可能已变化）：{checked:?}");
}
