//! `csp_Mp4Mov` 家族集成测试（tiny_http mock + 1 项真站）。
//!
//! 覆盖：固定 9 分类、列表页解析（`div#list_all ul>li`、`data-original` 前导空格、
//! `《…》` 标题拆分）、详情（`div.article-header h1` / `div.pic img` /
//! `ul.down-list` 只保留 `magnet:` 与 `ed2k:` 条目）、搜索（AES 时间戳 `t` 参数）、
//! 播放直出、空详情错误路径。真站 `https://mp4.z6.net.cn` 在线，列表/详情可复验；
//! `/search/` 端点上游当前返回站点 SQL 错误页 → 搜索仅 mock 验收。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};

const LIST_HTML: &str = r#"<!DOCTYPE html><html><body>
<div id="list_all"><ul>
  <li>
    <a href="/detail/107699.html"><img class="lazy" data-original=" https://img.test/a.jpg "></a>
    <div class="text_info">
      <h2><a href="/detail/107699.html">2010年泰国经典动作片《拳霸3》蓝光泰语中字迅雷下载</a></h2>
      <p><span class="update_time">2026-09-14</span></p>
    </div>
  </li>
  <li>
    <a href="/detail/2.html"><img src="/static/b.jpg"></a>
    <div class="text_info"><h2>无书名号影片</h2><p><span class="time">2024-01-02</span></p></div>
  </li>
</ul></div>
</body></html>"#;

const DETAIL_HTML: &str = r#"<!DOCTYPE html><html><body>
<ul class="breadcrumb cclear">
  <li><a href="/index.php">首页</a></li>
  <li><a href="/list/1-1.html">动作</a></li>
  <li class="active">下载详情</li>
</ul>
<div class="article-header">
  <h1>2010年泰国经典动作片《拳霸3》蓝光泰语中字迅雷下载</h1>
  <div class="pic"><img src="https://img.test/poster.jpg" alt="海报"></div>
</div>
<ul class="panel down-list thunder1">
  <li><div class="url-left"><a href="jianpian://pathtype=url&path=ftp://x.mp4" title="忽略">忽略</a></div></li>
  <li><div class="url-left"><a href="magnet:?xt=urn:btih:aa&dn=x.mp4" title="蓝光中字">蓝光</a></div></li>
  <li><div class="url-left"><a href="ed2k://|file|x.mp4|1|/|">电驴</a></div></li>
</ul>
</body></html>"#;

const EMPTY_HTML: &str = r#"<!DOCTYPE html><html><body><p>系统提示</p></body></html>"#;

fn header(content_type: &str) -> Header {
    Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).expect("header")
}

fn respond(request: tiny_http::Request, body: &str, content_type: &str) {
    let _ = request.respond(Response::from_string(body).with_header(header(content_type)));
}

/// 记录搜索请求的 URL（供断言 `wd` / `t` 参数）。
type Seen = Arc<Mutex<Vec<String>>>;

fn start_server() -> (Arc<Server>, Seen) {
    let server = Arc::new(Server::http("127.0.0.1:0").expect("server"));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let handle = Arc::clone(&server);
    let log = Arc::clone(&seen);
    thread::spawn(move || loop {
        let Ok(request) = handle.recv() else { return };
        let url = request.url().to_string();
        if url.starts_with("/search/") {
            log.lock().expect("lock").push(url);
            respond(request, LIST_HTML, "text/html; charset=utf-8");
        } else if url.starts_with("/detail/999") {
            respond(request, EMPTY_HTML, "text/html; charset=utf-8");
        } else if url.starts_with("/detail/") {
            respond(request, DETAIL_HTML, "text/html; charset=utf-8");
        } else if url.starts_with("/list/") {
            respond(request, LIST_HTML, "text/html; charset=utf-8");
        } else {
            respond(request, EMPTY_HTML, "text/html; charset=utf-8");
        }
    });
    (server, seen)
}

fn base_of(server: &Arc<Server>) -> String {
    format!("http://{}", server.server_addr())
}

fn site(base: &str) -> Site {
    let mut s = Site::default();
    s.key = "mp4mov".to_string();
    s.name = "MP4电影".to_string();
    s.api = "csp_Mp4Mov".to_string();
    s.ext = Some(json!(base));
    s
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_are_fixed_nine() {
    let cats = vbox_core::spider::catvod::families::mp4mov::categories();
    assert_eq!(cats.len(), 9);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "动作片");
    assert_eq!(cats[8].type_id, "9");
    assert_eq!(cats[8].type_name, "动画片");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn videos_parse_list_page() {
    let (server, _) = start_server();
    let base = base_of(&server);
    let rows = vbox_core::sources::spider::list_videos(&cfg(), &site(&base), "1", 1).expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "/detail/107699.html");
    assert_eq!(rows[0].name, "拳霸3");
    assert_eq!(rows[0].cover.as_deref(), Some("https://img.test/a.jpg"));
    assert_eq!(rows[0].remark.as_deref(), Some("2026-09-14"));
    assert_eq!(rows[1].name, "无书名号影片");
    assert_eq!(rows[1].cover.as_deref(), Some(format!("{base}/static/b.jpg").as_str()));
    assert_eq!(rows[1].remark.as_deref(), Some("2024-01-02"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_maps_title_cover_and_groups() {
    let (server, _) = start_server();
    let base = base_of(&server);
    let d = vbox_core::sources::spider::get_detail(&cfg(), &site(&base), "/detail/107699.html")
        .expect("detail");
    assert_eq!(d.name, "2010年泰国经典动作片《拳霸3》蓝光泰语中字迅雷下载");
    assert_eq!(d.cover.as_deref(), Some("https://img.test/poster.jpg"));
    assert_eq!(d.episodes.len(), 1);
    assert_eq!(d.episodes[0].group, "列表1");
    assert_eq!(d.episodes[0].items.len(), 2);
    assert_eq!(d.episodes[0].items[0].title, "蓝光中字");
    assert!(d.episodes[0].items[0].url.starts_with("magnet:"));
    assert_eq!(d.episodes[0].items[1].title, "电驴");
    assert!(d.episodes[0].items[1].url.starts_with("ed2k:"));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_without_content_reports_parse_error() {
    let (server, _) = start_server();
    let base = base_of(&server);
    let err = vbox_core::sources::spider::get_detail(&cfg(), &site(&base), "/detail/999.html")
        .expect_err("should fail");
    let text = format!("{err:?}");
    assert!(text.contains("catvod.result.parse"), "实际: {text}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_passes_signed_token() {
    let (server, seen) = start_server();
    let base = base_of(&server);
    let rows = vbox_core::sources::spider::search(&cfg(), &site(&base), "拳霸", false).expect("search");
    assert_eq!(rows.len(), 2);
    let urls = seen.lock().expect("lock").clone();
    assert_eq!(urls.len(), 1);
    let url = &urls[0];
    assert!(url.contains("wd=%E6%8B%B3%E9%9C%B8"), "实际: {url}");
    let token = url.split("t=").nth(1).expect("t 参数");
    // 10 位时间戳 → PKCS5 补到 16 字节 → base64 24 字符（末尾 `==`），
    // 再按 query 规则百分号编码（`=` → `%3D`）。
    assert!(token.contains("%3D%3D"), "实际: {token}");
    let raw_len = token.matches("%3D").count();
    assert_eq!(raw_len, 2, "应有两个 `=` 被编码，实际: {token}");
    assert!(token.len() >= 24, "实际: {token}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_passthrough_parse_zero() {
    let (server, _) = start_server();
    let base = base_of(&server);
    let raw = vbox_core::sources::spider::player_content(
        &cfg(),
        &site(&base),
        "列表1",
        "magnet:?xt=urn:btih:aa",
        &[],
    )
    .expect("play");
    assert_eq!(raw["parse"], 0);
    assert_eq!(raw["url"], "magnet:?xt=urn:btih:aa");
}

/// 真站复验：列表 + 详情（上游可用、搜索端点故障）。
#[test]
#[ignore]
fn live_list_and_detail() {
    let base = "https://mp4.z6.net.cn".to_string();
    let s = site(&base);
    let rows = vbox_core::sources::spider::list_videos(&cfg(), &s, "1", 1).expect("live list");
    println!("live list rows={}", rows.len());
    for row in rows.iter().take(3) {
        println!("{row:?}");
    }
    assert!(rows.len() >= 5, "真站列表应至少 5 行，实际 {}", rows.len());

    let id = rows[0].id.clone();
    let d = vbox_core::sources::spider::get_detail(&cfg(), &s, &id).expect("live detail");
    println!("live detail name={:?} groups={} eps={}", d.name, d.episodes.len(),
        d.episodes.iter().map(|g| g.items.len()).sum::<usize>());
    assert!(!d.name.is_empty());
    assert!(!d.episodes.is_empty(), "详情应有 magnet/ed2k 分组");
}
