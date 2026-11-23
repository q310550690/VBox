//! `csp_Zhaozy`（找资源）家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、登录（表单 + Set-Cookie 聚合 + 复用）、搜索解析与关键词过滤、
//! 搜索 URL 编码、详情页提取阿里分享（`push` 语义展开）、直连阿里分享 id、
//! 播放（阿里剧集直链）与空分类/空关键词行为。
//!
//! 上游现状：`zhaoziyuan.pw` 的 http/https/搜索均不可达，因此只有 mock 验收，
//! 无真站 `#[ignore]` 用例（与附录 EY 偏差 3 一致）。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

const SEARCH_HTML: &str = r##"<html><body>
  <div class="li_con"><div class="news_text">
    <a href="https://www.alipan.com/s/AAA111"><h3>三体 全集</h3><p>2024-01-02</p></a>
  </div></div>
  <div class="li_con"><div class="news_text">
    <a href="/res/2.html"><h3>三体 花絮</h3><p>备注乙</p></a>
  </div></div>
  <div class="li_con"><div class="news_text">
    <a href="/res/3.html"><h3>别的资源</h3><p>备注丙</p></a>
  </div></div>
</body></html>"##;

const DETAIL_HTML: &str = r##"<html><body>
  <div class="content">资源说明
    <a href="https://www.aliyundrive.com/s/SHARE9">阿里云盘下载</a>
  </div>
</body></html>"##;

const ANON_JSON: &str = r#"{"share_name":"找资源分享","share_id":"AAA111","file_infos":[]}"#;
const TOKEN_JSON: &str = r#"{"share_token":"tok-z","expire_time":4102416000}"#;
const LIST_JSON: &str = r#"{"items":[
  {"file_id":"f1","file_name":"S01E01.mp4","type":"file","size":1024},
  {"file_id":"f2","file_name":"S01E02.mp4","type":"file","size":1024}
],"next_marker":""}"#;
const DOWNLOAD_JSON: &str = r#"{"url":"https://cdn.test/zhao/ep1.mp4"}"#;

fn respond(request: tiny_http::Request, body: &str, ctype: &str) {
    let header = Header::from_bytes("Content-Type", ctype).expect("ctype");
    request
        .respond(Response::from_string(body.to_string()).with_header(header))
        .expect("respond");
}

fn respond_login(request: tiny_http::Request) {
    let cookie = Header::from_bytes("Set-Cookie", "sid=abc; Path=/; HttpOnly").expect("cookie");
    let header = Header::from_bytes("Content-Type", "text/html; charset=utf-8").expect("ctype");
    request
        .respond(
            Response::from_string("ok".to_string())
                .with_header(header)
                .with_header(cookie),
        )
        .expect("respond login");
}

/// 返回 (root, 请求日志)。日志元素形如 `POST /logiu.html`、`GET /so?filename=… cookie`。
fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Server::http("127.0.0.1:0").expect("server");
    let root = format!("http://{}", server.server_addr());
    let log = Arc::new(Mutex::new(Vec::new()));
    let ret = root.clone();
    let log2 = Arc::clone(&log);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let method = request.method().to_string();
            let url = request.url().to_string();
            let has_cookie = request.headers().iter().any(|h| {
                h.field.equiv("Cookie") && h.value.as_str().contains("sid=abc")
            });
            log2.lock().expect("log").push(format!(
                "{method} {url}{}",
                if has_cookie { " cookie" } else { " no-cookie" }
            ));
            if url.contains("/logiu.html") {
                respond_login(request);
            } else if url.contains("get_share_by_anonymous") {
                respond(request, ANON_JSON, "application/json");
            } else if url.contains("get_share_token") {
                respond(request, TOKEN_JSON, "application/json");
            } else if url.contains("adrive/v3/file/list") {
                respond(request, LIST_JSON, "application/json");
            } else if url.contains("get_share_link_download_url") {
                respond(request, DOWNLOAD_JSON, "application/json");
            } else if url.contains("/so?filename=") {
                respond(request, SEARCH_HTML, "text/html; charset=utf-8");
            } else if url.contains("/res/1.html") {
                respond(request, DETAIL_HTML, "text/html; charset=utf-8");
            } else {
                respond(request, "<html><body>empty</body></html>", "text/html; charset=utf-8");
            }
        }
    });
    (ret, log)
}

fn site(root: &str) -> Site {
    let ext = json!({
        "username": "u1",
        "password": "p1",
        "searchurl": format!("{root}/so?filename={{wd}}"),
        "ali_base": format!("{root}/ali/"),
    })
    .to_string();
    Site {
        key: "zhaozy".into(),
        name: "找资源".into(),
        api: "csp_Zhaozy".into(),
        kind: 3,
        searchable: true,
        ext: Some(json!(ext)),
        ..Site::default()
    }
}

fn cfg() -> TvConfig {
    TvConfig::default()
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn dispatch_and_registry_recognise_zhaozy() {
    let api = "csp_Zhaozy";
    assert_eq!(
        vbox_core::spider::catvod::families::family_of(api),
        Some("zhaozy")
    );
    assert!(vbox_core::spider::catvod::registry::is_native_ready(api));
    assert!(vbox_core::spider::catvod::registry::blocked_reason(api).is_none());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn login_posts_form_and_cookie_is_reused() {
    let (root, log) = start_server();
    let site = site(&root);
    let cfg = cfg();
    let rows = spider::search(&cfg, &site, "三体", false).expect("search 1");
    assert_eq!(rows.len(), 2);
    let _ = spider::search(&cfg, &site, "三体", false).expect("search 2");
    let logs = log.lock().expect("log").clone();
    let logins = logs.iter().filter(|l| l.contains("POST /logiu.html")).count();
    assert_eq!(logins, 1, "登录只应发生一次，实际日志: {logs:?}");
    let searches: Vec<&String> = logs.iter().filter(|l| l.contains("/so?filename=")).collect();
    assert!(searches.len() >= 2, "搜索应发生两次，实际日志: {logs:?}");
    assert!(
        searches.iter().all(|l| l.ends_with("cookie")),
        "搜索请求应带登录 Cookie，实际日志: {logs:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_parses_rows_and_filters_keyword() {
    let (root, _log) = start_server();
    let site = site(&root);
    let rows = spider::search(&cfg(), &site, "三体", false).expect("search");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "https://www.alipan.com/s/AAA111");
    assert_eq!(rows[0].name, "三体 全集");
    assert_eq!(rows[0].remark.as_deref(), Some("2024-01-02"));
    assert_eq!(rows[1].id, "/res/2.html");
    assert_eq!(rows[1].name, "三体 花絮");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn search_url_encodes_keyword() {
    let (root, log) = start_server();
    let site = site(&root);
    let _ = spider::search(&cfg(), &site, "三体", false).expect("search");
    let logs = log.lock().expect("log").clone();
    assert!(
        logs.iter()
            .any(|l| l.contains("filename=%E4%B8%89%E4%BD%93")),
        "关键词应被 URL 编码，实际日志: {logs:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn empty_keyword_does_not_touch_network() {
    let (root, log) = start_server();
    let site = site(&root);
    let rows = spider::search(&cfg(), &site, "  ", false).expect("search");
    assert!(rows.is_empty());
    assert!(log.lock().expect("log").is_empty());
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_from_page_extracts_ali_share() {
    let (root, log) = start_server();
    let site = site(&root);
    let detail = spider::get_detail(&cfg(), &site, "res/1.html").expect("detail");
    assert_eq!(detail.episodes.len(), 1);
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert!(detail.episodes[0].items[0].url.contains("###f1"));
    assert!(detail.name.contains("找资源"), "name={}", detail.name);
    let logs = log.lock().expect("log").clone();
    assert!(logs.iter().any(|l| l.contains("GET /res/1.html")));
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn detail_accepts_direct_ali_share_id() {
    let (root, log) = start_server();
    let site = site(&root);
    let detail = spider::get_detail(&cfg(), &site, "https://www.alipan.com/s/AAA111").expect("detail");
    assert_eq!(detail.episodes[0].items.len(), 2);
    let logs = log.lock().expect("log").clone();
    assert!(
        !logs.iter().any(|l| l.contains("GET /res/1.html")),
        "直连分享不应抓详情页，实际日志: {logs:?}"
    );
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn player_resolves_ali_episode() {
    let (root, _log) = start_server();
    let site = site(&root);
    let value = spider::player_content(
        &cfg(),
        &site,
        "",
        "https://www.alipan.com/s/AAA111###f1",
        &[],
    )
    .expect("player");
    assert_eq!(value.get("parse").and_then(|v| v.as_i64()), Some(0));
    let url = value
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(url.contains("cdn.test"), "url={url}");
}

#[test]
#[ignore = "REA 已退役（2026-11-23 冻结）：依赖 _rea 判据数据/原生家族，不再维护"]
fn categories_and_videos_are_empty() {
    let (root, log) = start_server();
    let site = site(&root);
    let cfg = cfg();
    assert!(spider::list_categories(&cfg, &site).expect("cats").is_empty());
    assert!(spider::list_videos(&cfg, &site, "1", 1).expect("videos").is_empty());
    assert!(log.lock().expect("log").is_empty());
}
