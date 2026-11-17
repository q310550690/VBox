//! `csp_FourKHDR`（4KHDR 论坛）集成验收。
//!
//! 站点 `https://www.4khdr.cn` 2026-11 探测 ENOTFOUND/超时，因此用 tiny_http mock
//! 走完整分派链路：首页/分类（Discuz 版块）、详情（迅雷/YD 分享 + 磁力，避免网盘引擎触网）、
//! 搜索（GET 取 formhash → POST 搜索）、播放（磁力直出 / 引擎缺口 / 参数错误）。
//! 基址经 ext 段 3 覆盖（测试钩子偏差，见家族模块头）。

use std::io::Read;
use std::sync::Arc;
use std::thread;

use serde_json::json;
use tiny_http::{Header, Method, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider::{get_detail, list_categories, list_videos, player_content, search};

const HOME_BODY: &str = r##"<!doctype html><html><body>
<ul id="waterfall">
  <li><a href="/thread-1-1-1.html" title="首页4K片"><img src="/img/h1.jpg"></a>
      <div class="auth cl">2026-11-11 / 4K</div></li>
</ul>
</body></html>"##;

const LIST_BODY: &str = r##"<!doctype html><html><body>
<ul id="waterfall">
  <li><a href="/thread-2-1-1.html" title="分类片"><img src="https://img.test/c1.jpg"></a>
      <div class="auth cl">2026-11-10 / 1080P</div></li>
</ul>
</body></html>"##;

const DETAIL_BODY: &str = r##"<!doctype html><html><body>
<div id="thread_subject">测试片名 / 别名</div>
<img class="zoom" src="/img/z.jpg">
<table><tr><td id="postmessage_1"><font>这是剧情简介。</font></td></tr></table>
<div class="pcb"><table class="t_table">
  <tr><td><a href="https://pan.xunlei.com/s/x1">迅雷</a></td></tr>
  <tr><td><a href="https://cloud.189.cn/t/y1">YD</a></td></tr>
  <tr><td><a href="magnet:?xt=urn:btih:aa">磁力1</a></td></tr>
  <tr><td><a href="magnet:?xt=urn:btih:bb">磁力2</a></td></tr>
</table></div>
</body></html>"##;

const SEARCH_FORM_BODY: &str = r##"<!doctype html><html><body>
<form><input type="hidden" name="formhash" value="fh-1"></form>
</body></html>"##;

const SEARCH_RESULT_BODY: &str = r##"<!doctype html><html><body>
<div id="threadlist"><ul>
  <li><h3>搜索命中片</h3><p>0</p><p>1</p><p>作者</p><p>2026-11-09</p>
      <a href="/thread-9-1-1.html">查看</a></li>
</ul></div>
</body></html>"##;

fn cfg() -> TvConfig {
    TvConfig::default()
}

fn site(base: &str) -> Site {
    Site {
        key: "fourkhdr-mock".into(),
        name: "4KHDR".into(),
        kind: 3,
        api: "csp_FourKHDR".into(),
        searchable: true,
        quick_search: true,
        ext: Some(json!(format!("./lib/token.json$$$null$$$0$$${base}"))),
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
            let is_post = matches!(request.method(), Method::Post);
            if url.starts_with("/search.php") {
                if is_post {
                    respond_text(request, SEARCH_RESULT_BODY);
                } else {
                    respond_text(request, SEARCH_FORM_BODY);
                }
            } else if url.contains("typeid=") {
                respond_text(request, LIST_BODY);
            } else if url.starts_with("/thread-") {
                respond_text(request, DETAIL_BODY);
            } else if url.contains("forumdisplay") {
                respond_text(request, HOME_BODY);
            } else {
                respond_text(request, HOME_BODY);
            }
        }
    });
    addr
}

#[test]
fn dispatch_routes_fourkhdr_to_native_family() {
    let site = site("https://mock.invalid");
    assert!(vbox_core::spider::catvod::registry::is_native_ready(&site.api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(&site.api).is_none());
    assert_eq!(
        vbox_core::spider::catvod::families::family_of(&site.api),
        Some("fourkhdr")
    );
}

#[test]
fn categories_are_eleven_discuz_blocks() {
    let base = start_server();
    let site = site(&base);
    let cats = list_categories(&cfg(), &site).expect("分类");
    assert_eq!(cats.len(), 11);
    assert_eq!(cats[0].type_id, "3");
    assert_eq!(cats[0].type_name, "4K电影");
    assert_eq!(cats[10].type_id, "34");
}

#[test]
fn home_videos_parse_waterfall_rows() {
    let base = start_server();
    let site = site(&base);
    let items = vbox_core::spider::catvod::families::fourkhdr::home_videos(&cfg(), &site)
        .expect("首页");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "首页4K片");
    assert_eq!(items[0].id, format!("{base}/thread-1-1-1.html"));
    assert_eq!(items[0].cover.as_deref(), Some(&format!("{base}/img/h1.jpg")[..]));
    assert_eq!(items[0].remark.as_deref(), Some("2026-11-11 / 4K"));
}

#[test]
fn category_list_through_dispatch() {
    let base = start_server();
    let site = site(&base);
    let items = list_videos(&cfg(), &site, "3", 1).expect("分类页");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "分类片");
    assert_eq!(items[0].cover.as_deref(), Some("https://img.test/c1.jpg"));
}

#[test]
fn detail_builds_thunder_yd_and_magnet_lines() {
    let base = start_server();
    let site = site(&base);
    let d = get_detail(&cfg(), &site, &format!("{base}/thread-1-1-1.html")).expect("详情");
    assert_eq!(d.name, "测试片名");
    assert_eq!(d.cover.as_deref(), Some(&format!("{base}/img/z.jpg")[..]));
    assert_eq!(d.desc.as_deref(), Some("这是剧情简介。"));
    let groups: Vec<&str> = d.episodes.iter().map(|g| g.group.as_str()).collect();
    assert_eq!(groups, vec!["迅雷", "YD", "磁力1", "磁力2"]);
    assert_eq!(d.episodes[0].items[0].url, "https://pan.xunlei.com/s/x1");
    assert_eq!(d.episodes[2].items[0].url, "magnet:?xt=urn:btih:aa");
}

#[test]
fn search_takes_formhash_then_posts() {
    let base = start_server();
    let site = site(&base);
    let items = search(&cfg(), &site, "命中", false).expect("搜索");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "搜索命中片");
    assert_eq!(items[0].id, format!("{base}/thread-9-1-1.html"));
    assert_eq!(items[0].remark.as_deref(), Some("2026-11-09"));
}

#[test]
fn player_magnet_passes_through() {
    let base = start_server();
    let site = site(&base);
    let v = player_content(&cfg(), &site, "磁力1", "magnet:?xt=urn:btih:aa", &[]).expect("磁力");
    assert_eq!(v["parse"], 0);
    assert_eq!(v["url"], "magnet:?xt=urn:btih:aa");
}

#[test]
fn player_rejects_unsupported_engines_and_bad_ids() {
    let base = start_server();
    let site = site(&base);
    let err = player_content(&cfg(), &site, "迅雷", "https://pan.xunlei.com/s/x1###fid", &[]).unwrap_err();
    assert!(err.to_string().contains("引擎未实现"), "{err}");
    let err = player_content(&cfg(), &site, "YD", "https://cloud.189.cn/t/y1###fid", &[]).unwrap_err();
    assert!(err.to_string().contains("引擎未实现"), "{err}");
    let err = player_content(&cfg(), &site, "夸克", "no-separator", &[]).unwrap_err();
    assert!(err.to_string().contains("分享###fid"), "{err}");
}
