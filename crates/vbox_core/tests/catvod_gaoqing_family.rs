//! `csp_Gaoqing`（高清网 / 音范丝）家族集成测试（tiny_http mock，全离线）。
//!
//! 覆盖：分派/registry 识别、JS 倒计时挑战的 HTTP 等价模拟（首跳挑战 → `/jump-node`
//! 拿 Set-Cookie → 带 Cookie 重试）、首页分类与列表、分类分页 URL、详情
//! （标题/封面/内容磁力 + 表格磁力 + 阿里分享展开、迅雷线路跳过）、搜索、
//! 播放（磁力直出 / 阿里剧集直链）与错误路径。
//!
//! 上游现状：`www.yinfans.me` 只剩倒计时挑战并跳到无关站点、`gaoqing.la` 不可达，
//! 因此本家族只有 mock 验收，无真站 `#[ignore]` 用例（与 blog 偏差 5 一致）。

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use tiny_http::{Header, Response, Server};

use vbox_core::model::{Site, TvConfig};
use vbox_core::sources::spider;

const CHALLENGE_HTML: &str = r#"<html><body><div id="countdown">2</div>
<script>let timeLeft = 2; window.location.replace('/jump-node');</script></body></html>"#;

const HOME_HTML: &str = r#"<html><body>
  <ul class="mainmenu">
    <li class="menu-item"><a href="/">首页</a></li>
    <li class="menu-item"><a href="/movie">电影</a></li>
    <li class="menu-item"><a href="/tv">电视剧</a></li>
  </ul>
  <div id="post_container"><ul>
    <li class="post"><div class="post-box"><img src="/i/1.jpg"/><a href="/p/1.html"></a>
      <h2>电影甲</h2><p class="info">HD 1080P</p></div></li>
    <li class="post"><div class="post-box"><a href="/p/2.html"></a><h2>电影乙</h2></div></li>
  </ul></div>
</body></html>"#;

const LIST_HTML: &str = r#"<html><body>
  <div id="post_container"><ul>
    <li class="post"><div class="post-box"><img src="/i/3.jpg"/><a href="/p/3.html"></a>
      <h2>第三部</h2><p class="info">更新至 12 集</p></div></li>
  </ul></div>
</body></html>"#;

const DETAIL_HTML: &str = r#"<html><body>
  <div class="article_container"><h1>测试影片</h1></div>
  <div id="post_content">
    <img src="/c.jpg"/>
    <a href="magnet:?xt=urn:btih:111&dn=a">磁力一</a>
    <a href="https://www.alipan.com/s/abc123">阿里云盘</a>
    <a href="https://pan.xunlei.com/s/xunlei1">迅雷云盘</a>
  </div>
  <table id="cili"><tr><td>
    <a href="magnet:?xt=urn:btih:222&dn=b"><span>高清</span></a>
  </td></tr></table>
</body></html>"#;

const SEARCH_HTML: &str = r#"<html><body>
  <div id="post_container"><ul>
    <li class="post"><div class="post-box"><a href="/p/1.html"></a><h2>电影甲</h2></div></li>
  </ul></div>
</body></html>"#;

const ANON_JSON: &str = r#"{"share_name":"测试分享","share_id":"abc123","file_infos":[]}"#;
const TOKEN_JSON: &str = r#"{"share_token":"tok123","expire_time":4102416000}"#;
const LIST_JSON: &str = r#"{"items":[
  {"file_id":"f1","file_name":"S01E01.mp4","type":"file","size":1024},
  {"file_id":"f2","file_name":"S01E02.mp4","type":"file","size":2048}
],"next_marker":""}"#;
const DOWNLOAD_JSON: &str = r#"{"url":"https://cdn.test/ali/ep1.mp4"}"#;

fn respond(request: tiny_http::Request, body: &str, ctype: &str) {
    let header = Header::from_bytes("Content-Type", ctype).expect("ctype");
    request
        .respond(Response::from_string(body.to_string()).with_header(header))
        .expect("respond");
}

fn respond_cookie(request: tiny_http::Request, body: &str) {
    let cookie = Header::from_bytes("Set-Cookie", "challenge=pass; Path=/").expect("cookie");
    let header = Header::from_bytes("Content-Type", "text/html; charset=utf-8").expect("ctype");
    request
        .respond(
            Response::from_string(body.to_string())
                .with_header(header)
                .with_header(cookie),
        )
        .expect("respond cookie");
}

/// 返回 (root, 请求日志)。日志元素形如 `no-cookie /`、`cookie /`、`cookie /jump-node`。
fn start_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = Server::http("127.0.0.1:0").expect("server");
    let root = format!("http://{}", server.server_addr());
    let log = Arc::new(Mutex::new(Vec::new()));
    let ret = root.clone();
    let log2 = Arc::clone(&log);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            let has_cookie = request
                .headers()
                .iter()
                .any(|h| h.field.equiv("Cookie") && h.value.as_str().contains("challenge=pass"));
            log2.lock()
                .expect("log")
                .push(format!("{} {url}", if has_cookie { "cookie" } else { "no-cookie" }));
            if url.contains("adrive/v3/share_link/get_share_by_anonymous") {
                respond(request, ANON_JSON, "application/json");
            } else if url.contains("get_share_token") {
                respond(request, TOKEN_JSON, "application/json");
            } else if url.contains("adrive/v3/file/list") {
                respond(request, LIST_JSON, "application/json");
            } else if url.contains("get_share_link_download_url") {
                respond(request, DOWNLOAD_JSON, "application/json");
            } else if url.contains("/jump-node") {
                respond_cookie(request, "ok");
            } else if url.contains("?s=") {
                respond(request, SEARCH_HTML, "text/html; charset=utf-8");
            } else if url.contains("/movie/page/") {
                respond(request, LIST_HTML, "text/html; charset=utf-8");
            } else if url.contains("/p/") {
                respond(request, DETAIL_HTML, "text/html; charset=utf-8");
            } else if has_cookie {
                respond(request, HOME_HTML, "text/html; charset=utf-8");
            } else {
                respond(request, CHALLENGE_HTML, "text/html; charset=utf-8");
            }
        }
    });
    (ret, log)
}

fn config() -> TvConfig {
    let mut cfg = TvConfig::default();
    cfg.http.timeout = 5_000;
    cfg
}

fn site(root: &str) -> Site {
    // ext 段：[0] 原 Ali 配置占位、[1] 基址、[2] 代理、[3] VIP 开关、[4] 自定义 JSON
    let pan = json!({ "ali_base": format!("{root}/ali/") }).to_string();
    let ext = ["null", root, "", "0", pan.as_str()].join("$$$");
    Site {
        key: "gaoqing".into(),
        name: "高清网".into(),
        api: "csp_Gaoqing".into(),
        kind: 3,
        ext: Some(json!(ext)),
        ..Default::default()
    }
}

#[test]
fn dispatch_and_registry_recognise_gaoqing() {
    assert_eq!(
        vbox_core::spider::catvod::families::family_of("csp_Gaoqing"),
        Some("gaoqing")
    );
    assert!(vbox_core::spider::catvod::registry::is_native_ready(
        "csp_Gaoqing"
    ));
    assert!(vbox_core::spider::catvod::registry::blocked_reason("csp_Gaoqing").is_none());
}

#[test]
fn home_categories_and_challenge_retry() {
    let (root, log) = start_server();
    let cfg = config();
    let s = site(&root);
    let cats = spider::list_categories(&cfg, &s).expect("categories");
    assert_eq!(cats.len(), 2, "跳过「首页」");
    assert_eq!(cats[0].type_name, "电影");
    assert_eq!(cats[0].type_id, "/movie");
    let home = vbox_core::spider::catvod::families::gaoqing::home_videos(&cfg, &s).expect("home");
    assert_eq!(home.len(), 2);
    assert_eq!(home[0].name, "电影甲");
    assert_eq!(home[0].cover.as_deref(), Some(format!("{root}/i/1.jpg").as_str()));
    assert_eq!(home[0].remark.as_deref(), Some("HD 1080P"));
    let log = log.lock().expect("log");
    let homepage: Vec<_> = log.iter().filter(|l| l.ends_with(" /")).collect();
    assert!(homepage.len() >= 3, "{:?}", *log);
    assert!(homepage.iter().any(|l| l.starts_with("no-cookie")), "{:?}", *log);
    assert!(homepage.iter().any(|l| l.starts_with("cookie")), "{:?}", *log);
    assert!(log.iter().any(|l| l.contains("/jump-node")));
}

#[test]
fn category_videos_use_page_segment() {
    let (root, log) = start_server();
    let cfg = config();
    let s = site(&root);
    let items = spider::list_videos(&cfg, &s, "/movie", 2).expect("videos");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "第三部");
    let log = log.lock().expect("log");
    assert!(log.iter().any(|l| l.contains("/movie/page/2")), "{:?}", *log);
}

#[test]
fn detail_parses_magnets_and_pan_episodes() {
    let (root, _log) = start_server();
    let cfg = config();
    let s = site(&root);
    let d = spider::get_detail(&cfg, &s, "/p/1.html").expect("detail");
    assert_eq!(d.name, "测试影片");
    assert_eq!(d.cover.as_deref(), Some(format!("{root}/c.jpg").as_str()));
    let magnet_groups: Vec<_> = d.episodes.iter().filter(|g| g.group.starts_with("磁力")).collect();
    assert_eq!(magnet_groups.len(), 2, "内容磁力 1 条 + 表格磁力 1 条");
    assert_eq!(magnet_groups[0].group, "磁力1");
    assert_eq!(magnet_groups[0].items[0].url, "magnet:?xt=urn:btih:111");
    assert_eq!(magnet_groups[1].group, "磁力2");
    assert_eq!(magnet_groups[1].items[0].url, "magnet:?xt=urn:btih:222");
    let ali = d
        .episodes
        .iter()
        .find(|g| g.group == "阿里")
        .expect("阿里线路");
    assert_eq!(ali.items.len(), 2);
    assert!(ali.items[0].title.contains("S01E01"));
    assert!(d.episodes.iter().all(|g| g.group != "迅雷"), "迅雷无桌面引擎应跳过");
}

#[test]
fn search_passes_urlencoded_keyword() {
    let (root, log) = start_server();
    let cfg = config();
    let s = site(&root);
    let items = spider::search(&cfg, &s, "沙 丘", false).expect("search");
    assert_eq!(items.len(), 1);
    let log = log.lock().expect("log");
    assert!(
        log.iter().any(|l| l.contains("?s=%E6%B2%99+%E4%B8%98")),
        "{:?}",
        *log
    );
}

#[test]
fn player_magnet_and_ed2k_direct() {
    let (root, _log) = start_server();
    let cfg = config();
    let s = site(&root);
    let v = spider::player_content(&cfg, &s, "磁力1", "magnet:?xt=urn:btih:111&dn=a", &[])
        .expect("magnet player");
    assert_eq!(v["parse"], 0);
    assert_eq!(v["url"], "magnet:?xt=urn:btih:111");
    let e = spider::player_content(&cfg, &s, "", "ed2k://|file|a.mkv|1|H|/", &[]).expect("ed2k");
    assert_eq!(e["url"], "ed2k://|file|a.mkv|1|H|/");
}

#[test]
fn player_pan_episode_resolves_alias_download_url() {
    let (root, _log) = start_server();
    let cfg = config();
    let s = site(&root);
    let id = "https://www.alipan.com/s/abc123@@f1";
    let v = spider::player_content(&cfg, &s, "阿里", id, &[]).expect("ali player");
    assert_eq!(v["parse"], 0);
    assert_eq!(v["url"], "https://cdn.test/ali/ep1.mp4");
}

#[test]
fn player_rejects_illegal_id() {
    let (root, _log) = start_server();
    let cfg = config();
    let s = site(&root);
    let err = spider::player_content(&cfg, &s, "", "not-a-share", &[]).expect_err("非法 id");
    assert!(format!("{err}").contains("catvod."), "{err}");
}
