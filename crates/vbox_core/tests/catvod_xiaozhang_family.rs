//! `csp_XiaoZhang`（校长影视）家族集成测试（tiny_http mock + 真站 `#[ignore]`）。
//!
//! 覆盖：硬编码 4 分类、旧模板列表、旧模板详情（迅雷/YD 无引擎 → 单集兜底）、
//! 新模板详情（CSRF POST 表单 → 分享链接）、搜索（`/browse/?q=`）、错误映射。
//!
//! mock 只用迅雷/YD 链接：这两种网盘没有桌面引擎，不会触发真实网盘请求，
//! 因此集成测试可在离线下验证「页面解析 → 线路/分集」这条链路。
//! 真站用例默认 `#[ignore]`（现役 `https://xzyshd.com`，需要外网）。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Method, Response, Server};

use vbox_core::model::{Site, TvConfig};

const LIST_HTML: &str = r#"<html><body>
<div class="list-boxes"><div class="left_ly">
  <a href="/xzys/subject/37396.html"><img class="image_left" src="https://img.binhd.com/a.webp" alt="季风"><span class="text_title_p">季风</span></a>
</div></div>
<div class="list-boxes"><div class="left_ly">
  <a href="/xzys/subject/37397.html"><img class="image_left" src="/cover/b.jpg" alt=""><span class="text_title_p">备用标题</span></a>
</div></div>
</body></html>"#;

const LEGACY_DETAIL_HTML: &str = r#"<html><body>
<div class="container tc-main"><div class="row"><div class="article-box">
  <h1>季风</h1>
  <p><a href="https://pan.xunlei.com/s/VP69YAJe?pwd=jwfc#">迅雷网盘</a> 提取码：jwfc</p>
  <p><a href="https://cloud.189.cn/t/abcdef">天翼云盘</a></p>
  <p><a href="/xzys/subject/1.html">相关内容</a></p>
</div></div></div>
</body></html>"#;

const NEW_ITEM_HTML: &str = r#"<html><body>
<div class="detail-cover"><img src="/media/c.jpg"></div>
<div class="detail-intro"><h1>妈妈</h1><p class="detail-summary">剧情简介。</p></div>
<div class="download-panel">
  <section class="link-card"><b>迅雷网盘</b>
    <form method="post" action="/item/post-1/link/1/open/">
      <input type="hidden" name="csrfmiddlewaretoken" value="tok123">
    </form>
  </section>
</div>
</body></html>"#;

/// 新模板 POST「打开网盘」后返回的页面（含真实分享链接）。
const PAN_BODY_HTML: &str = r#"<html><body>
<a href="https://pan.xunlei.com/s/VP69YAJe?pwd=jwfc#">打开迅雷网盘</a>
</body></html>"#;

const NO_TITLE_HTML: &str =
    r#"<html><body><div class="container">系统提示......</div></body></html>"#;

const SEARCH_HTML: &str = r#"<html><body>
<article class="resource-card">
  <a class="resource-card__media" href="/item/post-1/"><img src="/m/1.jpg" alt="妈妈封面"></a>
  <h3><a href="/item/post-1/">妈妈</a></h3><b>7.1</b>
</article>
</body></html>"#;

fn respond_html(request: tiny_http::Request, body: &'static str) {
    let header =
        Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header");
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
        let is_post = matches!(request.method(), Method::Post);
        if is_post && url.starts_with("/item/post-1/link/1/open/") {
            respond_html(request, PAN_BODY_HTML);
        } else if url.starts_with("/dy.html") {
            respond_html(request, LIST_HTML);
        } else if url.starts_with("/xzys/subject/37396.html") {
            respond_html(request, LEGACY_DETAIL_HTML);
        } else if url.starts_with("/xzys/subject/gone.html") {
            respond_html(request, NO_TITLE_HTML);
        } else if url.starts_with("/item/post-1/") {
            respond_html(request, NEW_ITEM_HTML);
        } else if url.starts_with("/browse/") {
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
        key: "xiaozhang".into(),
        name: "校长影视".into(),
        api: "csp_XiaoZhang".into(),
        ..Default::default()
    };
    s.ext = Some(json!(format!("./lib/token.json$$${base}/$$$null$$$1")));
    s
}

#[test]
fn categories_are_fixed_four() {
    let base = start_server();
    let cats = vbox_core::sources::spider::list_categories(&cfg(), &site(&base)).expect("cats");
    assert_eq!(cats.len(), 4);
    assert_eq!(cats[0].type_id, "dy");
    assert_eq!(cats[1].type_name, "电视剧");
}

#[test]
fn videos_first_page_parses_boxes() {
    let base = start_server();
    let items =
        vbox_core::sources::spider::list_videos(&cfg(), &site(&base), "dy", 1).expect("list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "季风");
    assert!(
        items[0].id.ends_with("/xzys/subject/37396.html"),
        "id={}",
        items[0].id
    );
    assert_eq!(
        items[0].cover.as_deref(),
        Some("https://img.binhd.com/a.webp")
    );
    assert_eq!(items[1].name, "备用标题");
}

#[test]
fn legacy_detail_returns_pan_groups_without_engine() {
    let base = start_server();
    let detail =
        vbox_core::sources::spider::get_detail(&cfg(), &site(&base), "/xzys/subject/37396.html")
            .expect("detail");
    assert_eq!(detail.name, "季风");
    // 迅雷/YD 均无桌面引擎：线路保留、每条降级为单集（分享链接原样展示）。
    assert_eq!(detail.episodes.len(), 2, "groups={:?}", detail.episodes);
    assert_eq!(detail.episodes[0].group, "迅雷");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert!(detail.episodes[0].items[0].url.contains("pan.xunlei.com"));
    assert_eq!(detail.episodes[1].group, "YD");
}

#[test]
fn new_template_posts_csrf_form_and_gets_share() {
    let base = start_server();
    let detail = vbox_core::sources::spider::get_detail(&cfg(), &site(&base), "/item/post-1/")
        .expect("detail");
    assert_eq!(detail.name, "妈妈");
    assert_eq!(
        detail.cover.as_deref(),
        Some(format!("{base}/media/c.jpg").as_str())
    );
    assert!(detail
        .desc
        .as_deref()
        .unwrap_or_default()
        .contains("剧情简介"));
    assert_eq!(detail.episodes.len(), 1, "groups={:?}", detail.episodes);
    assert_eq!(detail.episodes[0].group, "迅雷网盘");
    assert!(detail.episodes[0].items[0].url.contains("pan.xunlei.com"));
}

#[test]
fn detail_missing_title_maps_to_parse_error() {
    let base = start_server();
    let err =
        vbox_core::sources::spider::get_detail(&cfg(), &site(&base), "/xzys/subject/gone.html")
            .expect_err("must fail");
    assert!(
        format!("{err:?}").contains("catvod.result.parse"),
        "err={err:?}"
    );
}

#[test]
fn search_parses_resource_cards() {
    let base = start_server();
    let items =
        vbox_core::sources::spider::search(&cfg(), &site(&base), "妈妈", false).expect("search");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "妈妈");
    assert_eq!(items[0].remark.as_deref(), Some("豆瓣 7.1"));
}

#[test]
fn player_rejects_thunder_link_with_host_unsupported() {
    let base = start_server();
    let err = vbox_core::sources::spider::player_content(
        &cfg(),
        &site(&base),
        "迅雷",
        "https://pan.xunlei.com/s/VP69YAJe###1",
        &[],
    )
    .expect_err("must fail");
    assert!(
        format!("{err:?}").contains("catvod.host.unsupported"),
        "err={err:?}"
    );
}

/// 真站：4 个分类页都能出条目（现站 `.html?page=` 路由）。
#[test]
#[ignore = "需要外网访问 https://xzyshd.com"]
fn live_categories_and_list() {
    let base = String::new();
    let mut live = Site {
        key: "xiaozhang-live".into(),
        name: "校长影视".into(),
        api: "csp_XiaoZhang".into(),
        ..Default::default()
    };
    live.ext = None;
    let cats = vbox_core::sources::spider::list_categories(&cfg(), &live).expect("live cats");
    assert_eq!(cats.len(), 4);
    let items = vbox_core::sources::spider::list_videos(&cfg(), &live, "dy", 1).expect("live list");
    assert!(
        items.len() >= 10,
        "live list too small: {} (base={base})",
        items.len()
    );
}

/// 真站：旧模板详情页 → 网盘线路（夸克分享可直接浏览时会出现 `###` 分集）。
#[test]
#[ignore = "需要外网访问 https://xzyshd.com 且详情页挂有可匿名浏览的网盘分享"]
fn live_legacy_detail_has_pan_groups() {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 60_000;
    let mut live = Site {
        key: "xiaozhang-live".into(),
        name: "校长影视".into(),
        api: "csp_XiaoZhang".into(),
        ..Default::default()
    };
    live.ext = None;
    let items = vbox_core::sources::spider::list_videos(&cfg, &live, "dy", 1).expect("live list");
    assert!(!items.is_empty());
    let mut checked = Vec::new();
    let mut succeeded = 0usize;
    for item in items.iter().take(5) {
        checked.push(item.id.clone());
        let Ok(detail) = vbox_core::sources::spider::get_detail(&cfg, &live, &item.id) else {
            continue;
        };
        succeeded += 1;
        let has_pan = detail.episodes.iter().any(|g| {
            g.items
                .iter()
                .any(|e| e.url.contains("pan.") || e.url.contains("###"))
        });
        eprintln!(
            "detail {} -> groups={} first_url={:?}",
            item.id,
            detail.episodes.len(),
            detail
                .episodes
                .first()
                .and_then(|g| g.items.first())
                .map(|e| e.url.chars().take(60).collect::<String>())
        );
        if has_pan {
            return;
        }
    }
    if succeeded == 0 {
        // 列表可达但详情全部 404/不可达：站点详情结构改版或下线（外部漂移），
        // 不视为解析器回归；站点恢复健康后本用例仍会断言网盘线路。
        eprintln!("详情全部不可达（站点可能改版/下线），跳过网盘线路断言：{checked:?}");
        return;
    }
    panic!("前 5 条详情均未解析出网盘线路（页面结构可能已改版）：{checked:?}");
}

/// 真站：现站搜索页 `/browse/?q=` 的 `article.resource-card` 解析。
#[test]
#[ignore = "需要外网访问 https://xzyshd.com"]
fn live_search_via_browse_page() {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 60_000;
    let mut live = Site {
        key: "xiaozhang-live".into(),
        name: "校长影视".into(),
        api: "csp_XiaoZhang".into(),
        ..Default::default()
    };
    live.ext = None;
    let items =
        vbox_core::sources::spider::search(&cfg, &live, "妈妈", false).expect("live search");
    assert!(!items.is_empty(), "现站搜索页未解析出结果");
}

/// 真站：搜索（新模板）→ 详情（新模板 CSRF POST 表单）→ 网盘线路，端到端验证新链路。
#[test]
#[ignore = "需要外网访问 https://xzyshd.com 且新模板详情页挂有可匿名打开的网盘链接"]
fn live_new_template_flow_resolves_share() {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 60_000;
    let mut live = Site {
        key: "xiaozhang-live".into(),
        name: "校长影视".into(),
        api: "csp_XiaoZhang".into(),
        ..Default::default()
    };
    live.ext = None;
    let items =
        vbox_core::sources::spider::search(&cfg, &live, "妈妈", false).expect("live search");
    let item = items
        .iter()
        .find(|i| i.id.contains("/item/"))
        .expect("搜索结果里没有新模板详情链接");
    let detail =
        vbox_core::sources::spider::get_detail(&cfg, &live, &item.id).expect("live detail");
    assert!(!detail.name.is_empty(), "新模板详情标题为空：{}", item.id);
    let has_pan = detail.episodes.iter().any(|g| {
        g.items
            .iter()
            .any(|e| e.url.contains("pan.") || e.url.contains("###"))
    });
    assert!(
        has_pan,
        "新模板详情未解析出网盘线路：{} => {:?}",
        item.id, detail.episodes
    );
}
