//! `csp_Kugou` 家族集成测试（tiny_http mock + 1 项真站）。
//!
//! 覆盖：固定 3 分类、榜单列表（`.pc_rank_sidebar` 按 `cat` 的下标取块）、
//! 榜单页详情（`.pc_temp_title h3` / `.pc_temp_songlist`）、单曲页详情
//! （`.songContent` 三选择器）、搜索返回 WebView 依赖错误、播放直出、空页面错误路径。
//! 真站（`#[ignore]`）：`www.kugou.com` 榜单页与 mixsong 页当前均可解析。

use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const CHART_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="pc_rank_sidebar">
  <h3 data-index="0"><a href="javascript:;" title="热门榜单">热门榜单</a></h3>
  <ul>
    <li><a title="酷狗飙升榜" href="/yy/rank/home/1-6666.html?from=rank">酷狗飙升榜</a></li>
    <li><a title="酷狗TOP500" href="/yy/rank/home/1-8888.html?from=rank">酷狗TOP500</a></li>
  </ul>
</div>
<div class="pc_rank_sidebar">
  <h3 data-index="1"><a href="javascript:;" title="特色音乐榜">特色音乐榜</a></h3>
  <ul><li><a title="内地榜" href="/yy/rank/home/1-31308.html?from=rank">内地榜</a></li></ul>
</div>
</body></html>"#;

const RANK_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="pc_temp_title"><h3>酷狗飙升榜</h3></div>
<div class="rank_update">更新于 2026-11-11</div>
<div class="pc_temp_songlist"><ul>
  <li title="张韶涵、虚环 - 回旋" data-eid="g9b2v34b">
    <a href="https://www.kugou.com/mixsong/g9b2v34b.html" class="pc_temp_songname" title="张韶涵、虚环 - 回旋">回旋</a>
  </li>
  <li>
    <a href="/mixsong/g9lm2vbd.html" class="pc_temp_songname" title="王源 - 管他的公路尽头">管他的公路尽头</a>
  </li>
</ul></div>
</body></html>"#;

const MIX_HTML: &str = r#"<!DOCTYPE html><html><body>
<div class="songContent fl">
  <div class="songName clearfix" title="">
    <span class="audioName"> 张韶涵、虚环 - 回旋 </span>
  </div>
  <div class="songDetail clearfix">
    <p class="albumName fl"><a title="回旋">回旋</a></p>
    <p class="singerName fl"><a title="张韶涵">张韶涵</a></p>
  </div>
</div>
</body></html>"#;

fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).expect("header")
}

fn respond(request: tiny_http::Request, body: &str) {
    let _ = request.respond(Response::from_string(body).with_header(html_header()));
}

fn start_server() -> Arc<Server> {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let handle = Arc::clone(&server);
    thread::spawn(move || loop {
        let Ok(request) = handle.recv() else { return };
        let url = request.url().to_string();
        if url.contains("/yy/rank/home/1-8888.html") {
            respond(request, RANK_HTML);
        } else if url.contains("/yy/rank/home/") {
            respond(request, CHART_HTML);
        } else if url.contains("/mixsong/") {
            respond(request, MIX_HTML);
        } else if url.contains("/empty.html") {
            respond(request, "<html><body></body></html>");
        } else {
            respond(request, "<html><body>not found</body></html>");
        }
    });
    server
}

fn base_of(server: &Arc<Server>) -> String {
    format!("http://{}", server.server_addr())
}

fn site(base: &str) -> Site {
    Site {
        key: "kugou".to_string(),
        name: "酷狗音乐".to_string(),
        api: "csp_Kugou".to_string(),
        ext: Some(json!(base)),
        ..Default::default()
    }
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn categories_via_dispatch() {
    let cfg = TvConfig::default();
    let categories = vbox_core::sources::spider::list_categories(&cfg, &site("http://127.0.0.1:1"))
        .expect("categories");
    assert_eq!(categories.len(), 3);
    assert_eq!(categories[0].type_id, "6666|0");
    assert_eq!(categories[2].type_name, "全球榜");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn list_charts_follows_category_index() {
    let server = start_server();
    let base = base_of(&server);
    let cfg = TvConfig::default();
    let items = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "6666|0", 1).expect("list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "酷狗飙升榜");
    assert!(items[0].id.ends_with("/yy/rank/home/1-6666.html?from=rank"));

    let second = vbox_core::sources::spider::list_videos(&cfg, &site(&base), "33162|1", 1).expect("list2");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].name, "内地榜");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn rank_page_detail_collects_songs() {
    let server = start_server();
    let base = base_of(&server);
    let cfg = TvConfig::default();
    let id = format!("{base}/yy/rank/home/1-8888.html?from=rank");
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), &id).expect("detail");
    assert_eq!(detail.name, "酷狗飙升榜");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].group, "Qile");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[0].title, "张韶涵、虚环 - 回旋");
    assert!(detail.episodes[0].items[1].url.contains("/mixsong/g9lm2vbd.html"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn mixsong_detail_is_single_episode() {
    let server = start_server();
    let base = base_of(&server);
    let cfg = TvConfig::default();
    let id = format!("{base}/mixsong/g9b2v34b.html");
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site(&base), &id).expect("detail");
    assert_eq!(detail.name, "张韶涵、虚环 - 回旋");
    assert_eq!(detail.episodes[0].items.len(), 1);
    assert_eq!(detail.episodes[0].items[0].title, "张韶涵");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn empty_page_reports_parse_error() {
    let server = start_server();
    let base = base_of(&server);
    let cfg = TvConfig::default();
    let id = format!("{base}/empty.html");
    let err = vbox_core::sources::spider::get_detail(&cfg, &site(&base), &id).unwrap_err();
    assert!(format!("{err:?}").contains("catvod.result.parse"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn search_reports_webview_dependency() {
    let cfg = TvConfig::default();
    let err = vbox_core::sources::spider::search(&cfg, &site("http://127.0.0.1:1"), "周杰伦", false)
        .unwrap_err();
    assert!(format!("{err:?}").contains("catvod.host.unsupported"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：判据/家族/审计矩阵测试，不再维护"]
fn player_returns_direct_url_with_ua() {
    let cfg = TvConfig::default();
    let value = vbox_core::sources::spider::player_content(
        &cfg,
        &site("http://127.0.0.1:1"),
        "Qile",
        "https://www.kugou.com/mixsong/abc.html",
        &[],
    )
    .expect("player");
    assert_eq!(value["parse"], 0);
    assert!(value["url"].as_str().unwrap().ends_with("abc.html"));
    assert!(value["header"]["User-Agent"].as_str().unwrap().contains("Chrome"));
}

/// 真站：榜单列表 + 榜单页详情（`www.kugou.com` 服务端 HTML 含目标选择器）。
#[test]
#[ignore = "需要访问 www.kugou.com"]
fn live_rank_and_detail() {
    let cfg = TvConfig::default();
    let site = Site {
        key: "kugou-live".to_string(),
        name: "酷狗音乐".to_string(),
        api: "csp_Kugou".to_string(),
        ..Default::default()
    };
    let charts = vbox_core::sources::spider::list_videos(&cfg, &site, "6666|0", 1).expect("live list");
    assert!(!charts.is_empty(), "live charts empty");
    println!("live charts: {} first={:?}", charts.len(), charts.first());

    let first = charts[0].id.clone();
    let detail = vbox_core::sources::spider::get_detail(&cfg, &site, &first).expect("live detail");
    assert!(!detail.name.is_empty(), "live detail name empty");
    assert!(
        detail.episodes.iter().any(|g| !g.items.is_empty()),
        "live detail has no songs"
    );
    println!(
        "live detail: name={:?} groups={} songs={}",
        detail.name,
        detail.episodes.len(),
        detail.episodes[0].items.len()
    );
}
