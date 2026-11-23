//! `csp_YunPan`（弹幕 | 云盘）集成验收。
//!
//! 上游 `https://res.yunpan.win/` 2026-11 探测返回 200 但 0 字节（空响应），
//! 因此用 tiny_http mock 走完整分派链路：首页卡片、分类/搜索（同形 URL）、
//! 详情（仅 YD/迅雷分享 → 不触网、按原类语义报错）、播放（引擎缺口/未展开/未知分享）。
//! 家族侧通过 `ext` 覆盖基址（与 Bttwoo 一致的显式偏差，见文档 §130.4）。

use std::io::Read;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};

const HOME_BODY: &str = r#"<!doctype html><html><body>
<div class="row">
  <div class="col">
    <a onclick="window.open('https://pan.quark.cn/s/abc123')"><img alt="首页夸克片" src="/p/1.jpg"></a>
    <div class="card-footer"><small>1080P</small></div>
    <div class="card-body"><p>一句话</p><p>网盘</p><p>第 3 段备注</p></div>
  </div>
</div>
</body></html>"#;

const LIST_BODY: &str = r#"<!doctype html><html><body>
<div class="row">
  <div class="col">
    <a onclick="window.open('https://pan.quark.cn/s/q1')"><img alt="夸克资源" src="https://img.test/q1.jpg"></a>
    <div class="card-footer"><small>4K</small></div>
    <div class="card-body"><p>a</p><p>b</p><p>第三段</p></div>
  </div>
  <div class="col">
    <a onclick="window.open('https://pan.xunlei.com/s/x2')"><img alt="迅雷资源" src="/p/x2.jpg"></a>
    <div class="card-footer"><small>HD</small></div>
    <div class="card-body"><p>a</p><p>b</p></div>
  </div>
</div>
</body></html>"#;

const EMPTY_BODY: &str = r#"<!doctype html><html><body><div class="row"></div></body></html>"#;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "yunpan-mock".into(),
        name: "云盘".into(),
        kind: 3,
        api: "csp_YunPan".into(),
        searchable: true,
        quick_search: true,
        ext: Some(json!(format!("null$$${base}"))),
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
            let mut _body = String::new();
            let _ = request.as_reader().take(64 * 1024).read_to_string(&mut _body);
            if url.contains("Keyword=") && !url.contains("Keyword&") {
                respond_text(request, LIST_BODY);
            } else if url.contains("PageIndex=9") {
                respond_text(request, EMPTY_BODY);
            } else if url.contains("PageIndex=") {
                respond_text(request, LIST_BODY);
            } else {
                respond_text(request, HOME_BODY);
            }
        }
    });
    addr
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_routes_yunpan_to_native_family() {
    let site = site("https://mock.invalid");
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&site.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&site.api).is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_is_single_home() {
    let base = start_server();
    let site = site(&base);
    let cats = list_categories(&cfg(), &site).expect("分类");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "/");
    assert_eq!(cats[0].type_name, "首页");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn home_videos_parse_cards() {
    let base = start_server();
    let site = site(&base);
    let items = vbox_core::spider::catvod::families::yunpan::home_videos(&cfg(), &site)
        .expect("首页");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "首页夸克片");
    assert_eq!(items[0].id, "https://pan.quark.cn/s/abc123###首页夸克片");
    assert_eq!(items[0].cover.as_deref(), Some(&format!("{base}/p/1.jpg")[..]));
    // 备注 = 网盘 / footer / 第 3 个 p
    assert_eq!(items[0].remark.as_deref(), Some("夸克/1080P/第 3 段备注"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn list_videos_parse_two_cards_and_cover_absolutization() {
    let base = start_server();
    let site = site(&base);
    let items = list_videos(&cfg(), &site, "/", 2).expect("分类列表");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "夸克资源");
    assert_eq!(items[0].cover.as_deref(), Some("https://img.test/q1.jpg"));
    assert_eq!(items[1].name, "迅雷资源");
    assert_eq!(items[1].cover.as_deref(), Some(&format!("{base}/p/x2.jpg")[..]));
    assert_eq!(items[1].remark.as_deref(), Some("迅雷/HD"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_error_when_page_has_no_cards() {
    let base = start_server();
    let site = site(&base);
    let err = list_videos(&cfg(), &site, "/", 9).expect_err("空列表应报错");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_parses_cards() {
    let base = start_server();
    let site = site(&base);
    let items = search(&cfg(), &site, "云盘", false).expect("搜索");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "夸克资源");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_rejects_ids_without_separator() {
    let base = start_server();
    let site = site(&base);
    let err = get_detail(&cfg(), &site, "https://pan.quark.cn/s/x").expect_err("缺 ### 应报错");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "{text}");
    assert!(text.contains("###"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_with_only_unsupported_providers_reports_parse_error() {
    let base = start_server();
    let site = site(&base);
    // YD（天翼云盘）在原类里不进入详情；此处只给 YD → 线路为空 → parse 错误，且不触网。
    let err = get_detail(&cfg(), &site, "https://cloud.189.cn/s/abc###云盘片")
        .expect_err("仅 YD 应报错");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "{text}");
    assert!(text.contains("不受支持"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_reports_engine_gap_for_thunder() {
    let base = start_server();
    let site = site(&base);
    let err = player_content(&cfg(), &site, "迅雷线路", "https://pan.xunlei.com/s/x###fid1", &[])
        .expect_err("迅雷无引擎");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.host.unsupported"), "{text}");
    assert!(text.contains("迅雷"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_reports_engine_gap_for_yd() {
    let base = start_server();
    let site = site(&base);
    let err = player_content(&cfg(), &site, "YD线路", "https://cloud.189.cn/s/x###fid1", &[])
        .expect_err("YD 无引擎");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.host.unsupported"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_rejects_unexpanded_id() {
    let base = start_server();
    let site = site(&base);
    let err = player_content(&cfg(), &site, "夸克线路", "https://pan.quark.cn/s/x", &[])
        .expect_err("缺 ###fid");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.host.unsupported"), "{text}");
    assert!(text.contains("fid"), "{text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_rejects_unknown_share_link() {
    let base = start_server();
    let site = site(&base);
    let err = player_content(&cfg(), &site, "夸克线路", "not-a-share###fid1", &[])
        .expect_err("无法识别分享");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "{text}");
    assert!(text.contains("无法识别"), "{text}");
}
