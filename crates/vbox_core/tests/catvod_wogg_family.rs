//! `Wogg` 家族集成测试（4 api 共用：`csp_Wogg` / `csp_Wobg` / `csp_Wo4k` / `csp_WoGG`）。
//!
//! 用 tiny_http 起本地站点，验证「分类 → 列表 → 详情（网盘线路）→ 播放分派」链路；
//! mock 详情页只挂迅雷/YD 分享：这两种网盘没有桌面引擎，只做线路降级，不会触发真实网盘请求。
//! `csp_WoGG`/`csp_Wobg` 的 URL 模板与 `csp_Wogg` 不同，也各覆盖一例。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const HOME_HTML: &str = r##"<html><body>
<nav>
  <a class="nav-link" href="/vodtype/1.html">电影</a>
  <a class="nav-link" href="/vodtype/2.html">电视剧</a>
</nav>
<div class="module-item">
  <div class="module-item-pic"><img data-src="/upload/a.jpg" alt="a"></div>
  <div class="video-name"><a href="/detail/1.html">第一部</a></div>
  <div class="module-item-text">更新至 12 集</div>
</div>
<div class="module-item">
  <div class="module-item-pic"><img data-src="https://cdn.example/b.jpg"></div>
  <div class="video-name"><a href="/detail/2.html">第二部</a></div>
  <div class="module-item-text">全 24 集</div>
</div>
</body></html>"##;

const LIST_HTML: &str = r##"<html><body>
<div class="module-item">
  <div class="module-item-pic"><img data-src="/upload/a.jpg" alt="a"></div>
  <div class="video-name"><a href="/detail/1.html">第一部</a></div>
  <div class="module-item-text">更新至 12 集</div>
</div>
<div class="module-item">
  <div class="module-item-pic"><img data-src="/upload/b.jpg" alt="b"></div>
  <div class="video-name"><a href="/detail/2.html">第二部</a></div>
  <div class="module-item-text">全 24 集</div>
</div>
<script>$(".mac_total").text('145');</script>
</body></html>"##;

const DETAIL_HTML: &str = r##"<html><body>
<div class="video-info-header">
  <h1 class="page-title">测试影片</h1>
  <a class="tag-link" href="#">2024</a>
  <a class="tag-link" href="#">国产剧</a>
  <div class="tag-link"><a href="#">剧情</a><a href="#">都市</a></div>
</div>
<div class="module-item-pic"><img data-src="/p/cover.jpg"></div>
<div class="module-row-text" data-clipboard-text="https://pan.xunlei.com/s/bbb">迅雷</div>
<div class="module-row-text" data-clipboard-text="https://cloud.189.cn/t/ccc">YD</div>
</body></html>"##;

const SEARCH_HTML: &str = r##"<html><body>
<div class="module-search-item">
  <div class="module-item-pic"><img data-src="/s/a.jpg"></div>
  <div class="video-serial"><a href="/detail/9.html" title="搜索命中">第九部</a></div>
  <div class="video-tag-icon">HD</div>
</div>
</body></html>"##;

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
        if url.starts_with("/index.php/vodshow/") {
            respond_html(request, LIST_HTML);
        } else if url.starts_with("/index.php/vod/type/id/") {
            respond_html(request, LIST_HTML);
        } else if url.starts_with("/pianku/") {
            respond_html(request, LIST_HTML);
        } else if url.starts_with("/detail/1.html") || url.starts_with("/detail/2.html") {
            respond_html(request, DETAIL_HTML);
        } else if url.starts_with("/index.php/vodsearch/") || url.starts_with("/index.php/vod/search") {
            respond_html(request, SEARCH_HTML);
        } else if url.starts_with("/wosearch/") {
            respond_html(request, SEARCH_HTML);
        } else if url == "/" {
            respond_html(request, HOME_HTML);
        } else {
            respond_html(request, "<html><body>404</body></html>");
        }
    });
    base
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(api: &str, base: &str) -> Site {
    let mut s = Site {
        key: "wogg-test".into(),
        name: "Wogg 测试".into(),
        api: api.into(),
        ..Default::default()
    };
    s.ext = Some(json!(format!(
        "./lib/token.json$$${base}/$$$null$$$1"
    )));
    s
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_come_from_nav_links() {
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg(), &site("csp_Wogg", &base))
        .expect("cats");
    assert_eq!(cats.len(), 2, "{cats:?}");
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[1].type_id, "2");
    assert_eq!(cats[1].type_name, "电视剧");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn list_videos_parses_module_items() {
    let base = start_server();
    let items = vbox_core::sources::spider::list_videos(&cfg(), &site("csp_Wogg", &base), "1", 1)
        .expect("list");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].name, "第一部");
    assert!(items[0].id.ends_with("/detail/1.html"), "id={}", items[0].id);
    assert_eq!(
        items[0].cover.as_deref(),
        Some(format!("{base}/upload/a.jpg").as_str())
    );
    assert_eq!(items[0].remark.as_deref(), Some("更新至 12 集"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn home_videos_parses_home_page() {
    let base = start_server();
    let site = site("csp_Wogg", &base);
    let items = vbox_core::spider::catvod::families::wogg::home_videos(&cfg(), &site)
        .expect("home");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[1].name, "第二部");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_parses_search_items() {
    let base = start_server();
    let items = vbox_core::sources::spider::search(&cfg(), &site("csp_Wogg", &base), "测试", false)
        .expect("search");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].name, "第九部");
    assert_eq!(items[0].remark.as_deref(), Some("HD"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn wobg_template_uses_vod_type_path() {
    let base = start_server();
    // Wobg 的列表模板是 /index.php/vod/type/id/{id}/page/{page}.html。
    let items = vbox_core::sources::spider::list_videos(&cfg(), &site("csp_Wobg", &base), "22", 1)
        .expect("wobg list");
    assert_eq!(items.len(), 2);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn wo4k_template_uses_pianku_path() {
    let base = start_server();
    // Wo4k 的列表模板是 /pianku/{id}.html（分页才加 query）。
    let items = vbox_core::sources::spider::list_videos(&cfg(), &site("csp_Wo4k", &base), "5", 1)
        .expect("wo4k list");
    assert_eq!(items.len(), 2);
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_keeps_unsupported_pan_lines_without_network() {
    let base = start_server();
    let detail = vbox_core::sources::spider::get_detail(
        &cfg(),
        &site("csp_Wogg", &base),
        "/detail/1.html",
    )
    .expect("detail");
    assert_eq!(detail.name, "测试影片");
    assert_eq!(
        detail.cover.as_deref(),
        Some(format!("{base}/p/cover.jpg").as_str())
    );
    let desc = detail.desc.as_deref().unwrap_or_default();
    assert!(desc.contains("2024") && desc.contains("剧情"), "desc={desc}");
    assert_eq!(detail.episodes.len(), 2, "groups={:?}", detail.episodes);
    assert_eq!(detail.episodes[0].group, "迅雷");
    assert_eq!(detail.episodes[1].group, "YD");
    // 无引擎网盘：保留原始分享链接，不生成 ###fid（播放时会明确报错）。
    assert!(detail.episodes[0].items[0].url.contains("pan.xunlei.com"));
    assert!(!detail.episodes[0].items[0].url.contains("###"));
    assert!(detail.episodes[1].items[0].url.contains("cloud.189.cn"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_rejects_thunder_line_with_host_unsupported() {
    let base = start_server();
    let s = site("csp_Wogg", &base);
    let err = vbox_core::sources::spider::player_content(
        &cfg(),
        &s,
        "迅雷",
        "https://pan.xunlei.com/s/bbb###fid1",
        &[],
    )
    .expect_err("thunder has no engine");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.host.unsupported"), "err={msg}");
    assert!(msg.contains("迅雷"), "err={msg}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_rejects_unexpanded_line() {
    let base = start_server();
    let s = site("csp_Wogg", &base);
    let err = vbox_core::sources::spider::player_content(
        &cfg(),
        &s,
        "夸克",
        "https://pan.quark.cn/s/aaa",
        &[],
    )
    .expect_err("missing ###fid");
    let msg = format!("{err:?}");
    assert!(msg.contains("catvod.host.unsupported"), "err={msg}");
    assert!(msg.contains("剧集标识"), "err={msg}");
}
