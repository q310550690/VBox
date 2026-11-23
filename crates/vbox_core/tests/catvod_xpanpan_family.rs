//! `csp_Xpanpan`（小盘盘）集成验收：WordPress + 网盘分享清单站。
//!
//! 站点 `https://xpanpan.site` 已不可达（2026-11 探测 fetch failed），因此这里
//! 用 tiny_http mock 走完整分派链路：首页分类 → 分类列表 → 详情（网盘线路）→
//! 搜索 → 播放拒绝。mock 详情只挂迅雷/天翼链接：这两种网盘没有桌面引擎，
//! 不会触发真实网盘请求，可离线验证「页面解析 → 线路/分集」全链路。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};

const HOME_HTML: &str = r##"<html><body>
<nav class="site-nav"><ul>
<li><a href="/category/dianying/">电影</a></li>
<li><a href="/category/dianshiju/">电视剧</a></li>
</ul></nav>
<main class="site-main">
<article class="post">
 <header class="entry-header"><h4 class="entry-title"><a href="/archives/101" title="星际穿越 (2014)">星际穿越</a></h4></header>
 <ul class="cat-links"><li><a href="/tag/sf/">科幻</a></li><li><a href="/tag/movie/">电影</a></li></ul>
</article>
<article class="post">
 <header><h4><a href="/archives/102">流浪地球</a></h4></header>
 <div class="cat-links"><a href="/tag/sf/">科幻</a></div>
</article>
</main></body></html>"##;

const LIST_HTML: &str = r##"<html><body><main>
<article class="post">
 <header class="entry-header"><h4 class="entry-title"><a href="/archives/201" title="沙丘2">沙丘2</a></h4></header>
 <div class="cat-links"><a href="/tag/sf/">科幻</a></div>
</article>
</main></body></html>"##;

const DETAIL_HTML: &str = r##"<html><body><main>
<article class="post">
 <header class="entry-header"><h1 class="entry-title">星际穿越</h1></header>
 <div class="entry-meta"><span class="posted-on">2026-01-02</span></div>
 <div class="entry-content">
 <p>简介：一支探险队穿越虫洞寻找新家园。</p>
 <p><a href="https://pan.xunlei.com/s/VP69YAJe?pwd=jwfc#">迅雷网盘</a></p>
 <p><a href="https://cloud.189.cn/t/yd123">天翼云盘</a></p>
 <p><a href="https://example.org/not-pan">无关链接</a></p>
 </div>
</article>
</main></body></html>"##;

const SEARCH_HTML: &str = r##"<html><body><main>
<article class="post">
 <header class="entry-header"><h4 class="entry-title"><a href="/archives/301" title="搜索命中">搜索命中</a></h4></header>
 <div class="cat-links"><a href="/tag/sf/">科幻</a></div>
</article>
</main></body></html>"##;

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
        if url.starts_with("/category/dianying/page/2") {
            respond_html(request, LIST_HTML);
        } else if url.starts_with("/archives/101") {
            respond_html(request, DETAIL_HTML);
        } else if url.starts_with("/page/1/") {
            respond_html(request, SEARCH_HTML);
        } else {
            respond_html(request, HOME_HTML);
        }
    });
    base
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "xpanpan-live".to_string(),
        name: "小盘盘".to_string(),
        kind: 3,
        api: "csp_Xpanpan".to_string(),
        ext: Some(json!(format!("./lib/token.json$$${base}$$$null$$$1"))),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn dispatch_routes_xpanpan_to_native_family() {
    assert!(vbox_core::spider::catvod::is_native_ready("csp_Xpanpan"));
    assert!(vbox_core::spider::catvod::blocked_reason("csp_Xpanpan").is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_come_from_nav() {
    let base = start_server();
    let cats = list_categories(&cfg(), &site(&base)).expect("categories");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "/category/dianying/");
    assert_eq!(cats[0].type_name, "电影");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn category_page_lists_articles() {
    let base = start_server();
    let items = list_videos(&cfg(), &site(&base), "/category/dianying/", 2).expect("videos");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "/archives/201");
    assert_eq!(items[0].name, "沙丘2");
    assert_eq!(items[0].remark.as_deref(), Some("科幻"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn detail_exposes_pan_lines_with_degraded_episode() {
    let base = start_server();
    let detail = get_detail(&cfg(), &site(&base), "/archives/101").expect("detail");
    assert_eq!(detail.name, "星际穿越");
    let groups = &detail.episodes;
    assert_eq!(groups.len(), 2, "groups={groups:?}");
    let names: Vec<&str> = groups.iter().map(|g| g.group.as_str()).collect();
    assert!(names.contains(&"迅雷"), "names={names:?}");
    assert!(names.contains(&"YD"), "names={names:?}");
    // 迅雷/天翼都没有桌面引擎 → 降级为「原分享链接」单集。
    let thunder = groups.iter().find(|g| g.group == "迅雷").expect("thunder group");
    assert_eq!(thunder.items.len(), 1);
    assert!(thunder.items[0].url.starts_with("https://pan.xunlei.com/s/"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_hits_browse_page() {
    let base = start_server();
    let items = search(&cfg(), &site(&base), "沙丘", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "/archives/301");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_rejects_thunder_with_host_error() {
    let base = start_server();
    let err = player_content(
        &cfg(),
        &site(&base),
        "迅雷",
        "https://pan.xunlei.com/s/VP69YAJe",
        &[],
    )
    .expect_err("thunder has no engine");
    let text = err.to_string();
    assert!(text.contains("迅雷"), "err={text}");
    assert!(text.contains("引擎"), "err={text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_rejects_unexpanded_episode() {
    let base = start_server();
    let err = player_content(
        &cfg(),
        &site(&base),
        "夸克",
        "https://pan.quark.cn/s/abc123",
        &[],
    )
    .expect_err("missing ###fid");
    assert!(err.to_string().contains("###fid"), "err={err}");
}
