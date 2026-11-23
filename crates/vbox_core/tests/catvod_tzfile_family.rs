//! `csp_TZFile` 家族集成测试（tiny_http mock + 真站 `#[ignore]`）。
//!
//! 覆盖：写死 5 分类、列表第 1/2 页、详情元数据（无网盘链接时返回空剧集）、
//! B2 REST 下载组件链路（getDownloadData → getDownloadPageData → /redirect → 分享链接）、
//! 搜索页、缺标题错误映射、播放非法地址错误映射。
//!
//! 真站用例默认 `#[ignore]`（现役 `https://t-rex.tzfile.com`，需要外网）。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const LIST_HTML: &str = r#"<html><body><div class="site-content"><ul>
<li class="post-list-item col-6 col-md-4">
  <a class="thumb-link" href="https://trex.tzfile.com/119749.html"></a>
  <img data-src="https://pic1.imgdb.cn/item/1.jpg" src="/static/default-img.jpg">
  <h2><a href="https://trex.tzfile.com/119749.html">幻视任务 (2025)</a></h2>
  <div class="post-info"><div class="post-list-cat">电影</div></div>
</li>
<li class="post-list-item col-6 col-md-4">
  <a class="thumb-link" href="https://trex.tzfile.com/119750.html"></a>
  <img src="/static/default-img.jpg">
  <h2><a href="https://trex.tzfile.com/119750.html">没有封面的影片</a></h2>
  <div class="post-info"><div class="post-list-cat">电视剧</div></div>
</li>
</ul></div></body></html>"#;

const LIST2_HTML: &str = r#"<html><body><div class="site-content"><ul>
<li class="post-list-item">
  <a class="thumb-link" href="https://trex.tzfile.com/119900.html"></a>
  <img data-src="https://pic1.imgdb.cn/item/2.jpg">
  <h2><a href="https://trex.tzfile.com/119900.html">第二页影片</a></h2>
  <div class="post-info"><div class="post-list-cat">动漫</div></div>
</li>
</ul></div></body></html>"#;

const DETAIL_HTML: &str = r#"<html><body><div id="primary-home">
<h1 class="entry-title">幻视任务</h1>
<ul class="post-meta"><li class="single-date">2025-01-01</li>
<li><div class="post-list-cat"><a href="/x">西山将贵</a></div></li></ul>
<article><div class="entry-content">
  <img data-src="https://pic1.imgdb.cn/item/cover.jpg">
  <p>一桩超自然案件把三个人带到同一个夜晚。</p>
</div>
<div class="download"><button>网盘</button></div>
</article></div></body></html>"#;

const SEARCH_HTML: &str = r#"<html><body><div class="site-content"><ul>
<li class="post-list-item">
  <a class="thumb-link" href="https://trex.tzfile.com/120000.html"></a>
  <img data-src="https://pic1.imgdb.cn/item/3.jpg">
  <h2><a href="https://trex.tzfile.com/120000.html">幻视任务 第二季</a></h2>
  <div class="post-info"><div class="post-list-cat">电视剧</div></div>
</li>
</ul></div></body></html>"#;

const NO_TITLE_HTML: &str = r#"<html><body><div id="primary-home"><p>系统提示......</p></div></body></html>"#;

const REST_DATA: &str = r#"[{"name":"资源下载","button":[
{"name":"夸克云盘","link":"https://t-rex.tzfile.com/download?post_id=119749&index=0&i=0"},
{"name":"百度云盘","link":"https://t-rex.tzfile.com/download?post_id=119749&index=0&i=1"}]}]"#;

const REST_PAGE: &str = r#"{"button":{"name":"夸克云盘","url":"eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.token"}}"#;

const REDIRECT_HTML: &str = r#"<html><body><div class="qrcode">
<a href="https://pan.quark.cn/s/ffffffffffff" class="download-link">扫码下载</a>
<script>jQuery('.qrcode').qrcode({text:"https://pan.quark.cn/s/ffffffffffff"});</script>
</div></body></html>"#;

fn respond_html(request: tiny_http::Request, body: &'static str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("header");
    let _ = request.respond(Response::from_string(body).with_header(header));
}

fn respond_json(request: tiny_http::Request, body: &'static str) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
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
            respond_html(request, LIST_HTML);
        } else if url.starts_with("/movies/page/2") {
            respond_html(request, LIST2_HTML);
        } else if url.starts_with("/movies/page/1") {
            respond_html(request, LIST_HTML);
        } else if url.starts_with("/119749.html") {
            respond_html(request, DETAIL_HTML);
        } else if url.starts_with("/gone.html") {
            respond_html(request, NO_TITLE_HTML);
        } else if url.starts_with("/?type=post") {
            respond_html(request, SEARCH_HTML);
        } else if url.starts_with("/wp-json/b2/v1/getDownloadData") {
            respond_json(request, REST_DATA);
        } else if url.starts_with("/wp-json/b2/v1/getDownloadPageData") {
            respond_json(request, REST_PAGE);
        } else if url.starts_with("/redirect?token=") {
            respond_html(request, REDIRECT_HTML);
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
        key: "tzfile".into(),
        name: "TZFile".into(),
        api: "csp_TZFile".into(),
        ..Default::default()
    };
    s.ext = Some(json!(format!("{base}/")));
    s
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_are_five_fixed() {
    let cfg = cfg();
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg, &site(&base)).expect("cats");
    assert_eq!(cats.len(), 5);
    assert_eq!(cats[0].type_id, "movies");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[4].type_name, "综艺");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_first_page_parses_rows() {
    let cfg = cfg();
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "movies", 1)
        .expect("list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "幻视任务 (2025)");
    assert!(items[0].id.ends_with("/119749.html"), "id={}", items[0].id);
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://pic1.imgdb.cn/item/1.jpg")
    );
    assert_eq!(items[0].remark.as_deref(), Some("电影"));
    // 第二行只有默认占位图（被过滤）→ 无封面。
    assert_eq!(items[1].name, "没有封面的影片");
    assert!(items[1].cover.is_none(), "cover={:?}", items[1].cover);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_second_page_uses_page_route() {
    let cfg = cfg();
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "movies", 2)
        .expect("list");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "第二页影片");
    assert_eq!(items[0].remark.as_deref(), Some("动漫"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_parses_metadata_without_pan_links() {
    let cfg = cfg();
    let base = start_server();
    let detail =
        vbox_core::sources::spider::get_detail(&cfg, &site(&base), "119749.html").expect("detail");
    assert_eq!(detail.name, "幻视任务");
    assert_eq!(
        detail.cover.as_deref(),
        Some("https://pic1.imgdb.cn/item/cover.jpg")
    );
    assert_eq!(detail.year.as_deref(), Some("2025"));
    assert_eq!(detail.actor.as_deref(), Some("西山将贵"));
    assert!(
        detail.desc.as_deref().unwrap_or_default().contains("超自然"),
        "desc={:?}",
        detail.desc
    );
    // 页面无内联链接，B2 REST 链取到夸克分享；mock 环境浏览失败 → 降级为「分享链接单集」。
    assert_eq!(detail.episodes.len(), 1, "episodes={:?}", detail.episodes);
    assert_eq!(detail.episodes[0].group, "夸克网盘");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(
        detail.episodes[0].items[0].url,
        "https://pan.quark.cn/s/ffffffffffff"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_missing_title_maps_to_parse_error() {
    let cfg = cfg();
    let base = start_server();
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), "gone.html")
        .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.result.parse"), "err={err:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_parses_rows() {
    let cfg = cfg();
    let base = start_server();
    let items = vbox_core::sources::spider::search(&cfg, &site(&base), "幻视", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "幻视任务 第二季");
    assert_eq!(items[0].remark.as_deref(), Some("电视剧"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn rest_chain_extracts_pan_share_link() {
    let cfg = cfg();
    let base = start_server();
    let links = vbox_core::spider::catvod::families::tzfile::fetch_rest_links(&cfg, &base, "119749");
    assert_eq!(
        links,
        vec!["https://pan.quark.cn/s/ffffffffffff".to_string()]
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_rejects_unknown_share_link() {
    let cfg = cfg();
    let base = start_server();
    let err = vbox_core::sources::spider::player_content(
        &cfg,
        &site(&base),
        "夸克网盘",
        "https://example.com/not-a-pan-link",
        &[],
    )
    .expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.host.unsupported"), "err={err:?}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_rejects_empty_id() {
    let cfg = cfg();
    let base = start_server();
    let err =
        vbox_core::sources::spider::player_content(&cfg, &site(&base), "夸克", "  ", &[]).expect_err("must fail");
    assert!(format!("{err:?}").contains("catvod.result.parse"), "err={err:?}");
}

/// 真站冒烟：现役 `https://t-rex.tzfile.com` 的分类列表 + 详情元数据。
#[test]
#[ignore = "需要外网访问 https://t-rex.tzfile.com"]
fn live_site_list_and_detail() {
    let cfg = cfg();
    let mut site = Site {
        key: "tzfile-live".into(),
        name: "TZFile".into(),
        api: "csp_TZFile".into(),
        ..Default::default()
    };
    site.ext = None;
    let items = vbox_core::sources::spider::list_videos(&cfg, &site, "movies", 1).expect("live list");
    assert!(!items.is_empty(), "live list empty");
    let id = items[0].id.clone();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site, &id).expect("live detail");
    assert!(!detail.name.is_empty(), "live detail name empty");
}
