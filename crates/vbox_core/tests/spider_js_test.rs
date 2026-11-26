//! type=3 JS spider 集成测试
//!
//! 全部脚本内联（`inline:`）或由本地 tiny_http mock 提供，**不访问真实网络**。
//! 覆盖：全局函数 / `__jsEvalReturn` / `export default` / `req`（GET/POST+headers+data）/
//! 脚本缺失函数时的降级 / 脚本异常转错误 / http(s) 与 file:// 两种脚本来源。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::json;
use vbox_core::model::{Site, TvConfig};
use vbox_core::spider::js;

/* ============================================================
 * 测试夹具
 * ============================================================ */

fn test_site(api: &str) -> Site {
    Site {
        key: "test".into(),
        name: "测试站".into(),
        kind: 3,
        api: api.into(),
        ..Default::default()
    }
}

fn empty_cfg() -> TvConfig {
    TvConfig::default()
}

fn inline(code: &str) -> String {
    format!("inline:{code}")
}

/* ============================================================
 * 本地 mock HTTP 服务
 * ============================================================ */

#[derive(Debug, Clone)]
struct Captured {
    method: String,
    path: String,
    body: String,
    headers: HashMap<String, String>,
}

/// 一次 mock 响应
struct MockReply {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl MockReply {
    fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: body.into(),
        }
    }

    fn js(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "application/javascript; charset=utf-8",
            body: body.into(),
        }
    }
}

type Handler = Arc<dyn Fn(&Captured) -> MockReply + Send + Sync>;

struct MockServer {
    base: String,
    server: Arc<tiny_http::Server>,
    captured: Arc<Mutex<Vec<Captured>>>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(&Captured) -> MockReply + Send + Sync + 'static,
    {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("启动 mock 服务"));
        let addr = server.server_addr().to_ip().expect("mock 服务 IP 地址");
        let base = format!("http://{addr}");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let handler: Handler = Arc::new(handler);

        let listener = server.clone();
        let captured_thread = captured.clone();
        let handle = std::thread::spawn(move || {
            for mut request in listener.incoming_requests() {
                let method = request.method().as_str().to_ascii_uppercase();
                let path = request.url().to_string();
                let mut headers = HashMap::new();
                for h in request.headers() {
                    headers.insert(
                        format!("{}", h.field).to_ascii_lowercase(),
                        h.value.as_str().to_string(),
                    );
                }
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);

                let meta = Captured {
                    method,
                    path,
                    body,
                    headers,
                };
                captured_thread
                    .lock()
                    .expect("mock 记录锁")
                    .push(meta.clone());

                let reply = handler(&meta);
                let response = tiny_http::Response::from_string(reply.body)
                    .with_status_code(reply.status)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            reply.content_type.as_bytes(),
                        )
                        .expect("合法响应头"),
                    );
                let _ = request.respond(response);
            }
        });

        Self {
            base,
            server,
            captured,
            handle: Some(handle),
        }
    }

    fn captured(&self) -> Vec<Captured> {
        self.captured.lock().expect("mock 记录锁").clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        // 让 incoming_requests() 返回 None，accept 线程退出
        self.server.unblock();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/* ============================================================
 * 1. 全局函数形态（home/category/detail/search/play/live/sniffer/isVideo）
 * ============================================================ */

const GLOBALS_SCRIPT: &str = r##"
var logHit = "";

function home(filter) {
    log("home filter=", filter);
    return JSON.stringify({class: [
        {type_id: "1", type_name: "电影"},
        {type_id: "2", type_name: "电视剧"}
    ]});
}

function category(tid, pg, filter, extend) {
    if (filter !== true) throw new Error("filter 参数应为 true");
    return {list: [
        {vod_id: tid + "-" + pg, vod_name: "片" + tid, vod_pic: "https://img/1.jpg", vod_remarks: "更新至2集"}
    ]};
}

function detail(id) {
    return {list: [{
        vod_id: id,
        vod_name: "详情片",
        vod_pic: "https://img/d.jpg",
        vod_content: "简介",
        vod_year: "2024",
        vod_area: "内地",
        vod_director: "导演",
        vod_actor: "演员",
        vod_play_from: "线路A$$$线路B",
        vod_play_url: "第1集$https://a/1.m3u8#第2集$https://a/2.m3u8$$$第1集$https://b/1.mp4"
    }]};
}

function search(wd, quick) {
    return {list: [{vod_id: "s-" + wd, vod_name: wd, vod_remarks: quick ? "快搜" : "全搜"}]};
}

function play(flag, id, vipFlags) {
    return {url: "https://a/" + id + ".m3u8", header: {"User-Agent": "UA"}, parse: 0, flag: flag, vip: vipFlags.length};
}

function live(url) {
    return "直播,#genre#\nCCTV1," + url + "#https://live/2.m3u8";
}

function sniffer() { return true; }
function isVideo(url) { return url.indexOf(".m3u8") > 0; }
"##;

#[test]
fn globals_home_and_category() {
    let site = test_site(&inline(GLOBALS_SCRIPT));
    let cfg = empty_cfg();

    let cats = js::categories(&site, &cfg).expect("home() 分类");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[1].type_name, "电视剧");

    let items = js::category(&site, &cfg, "2", 3).expect("category() 列表");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "2-3");
    assert_eq!(items[0].name, "片2");
    assert_eq!(items[0].cover.as_deref(), Some("https://img/1.jpg"));
    assert_eq!(items[0].remark.as_deref(), Some("更新至2集"));
}

#[test]
fn globals_detail_search_play() {
    let site = test_site(&inline(GLOBALS_SCRIPT));
    let cfg = empty_cfg();

    let detail = js::detail(&site, &cfg, "42").expect("detail()");
    assert_eq!(detail.id, "42");
    assert_eq!(detail.name, "详情片");
    assert_eq!(detail.year.as_deref(), Some("2024"));
    assert_eq!(detail.episodes.len(), 2);
    assert_eq!(detail.episodes[0].group, "线路A");
    assert_eq!(detail.episodes[0].items.len(), 2);
    assert_eq!(detail.episodes[0].items[1].title, "第2集");
    assert_eq!(detail.episodes[0].items[1].url, "https://a/2.m3u8");
    assert_eq!(detail.episodes[1].items[0].url, "https://b/1.mp4");

    let hits = js::search(&site, &cfg, "关键词", true).expect("search()");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "s-关键词");
    assert_eq!(hits[0].remark.as_deref(), Some("快搜"));

    let play =
        js::player_content(&site, &cfg, "线路A", "ep1", &["flag1".to_string()]).expect("play()");
    assert_eq!(
        play.get("url").and_then(|v| v.as_str()),
        Some("https://a/ep1.m3u8")
    );
    assert_eq!(play.get("flag").and_then(|v| v.as_str()), Some("线路A"));
    assert_eq!(play.get("vip").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(
        play.get("header")
            .and_then(|v| v.get("User-Agent"))
            .and_then(|v| v.as_str()),
        Some("UA")
    );

    let live = js::live(&site, &cfg, "https://live/1.m3u8").expect("live()");
    assert!(live.contains("CCTV1"));
    assert!(live.contains("https://live/1.m3u8"));

    assert!(js::sniffer(&site, &cfg).expect("sniffer()"));
    assert!(js::is_video(&site, &cfg, "https://a/1.m3u8").expect("isVideo()"));
    assert!(!js::is_video(&site, &cfg, "https://a/1.html").expect("isVideo()"));
}

/* ============================================================
 * 2. __jsEvalReturn 对象形态（cat 风格）与 init(cfg)
 * ============================================================ */

#[test]
fn js_eval_return_object_and_init() {
    let script = r#"
var inited = false;
var spider = {
    init: function(cfg) {
        inited = !!(cfg && cfg.stype === 3 && cfg.skey === "test");
    },
    home: function(filter) {
        return {class: [{type_id: "c1", type_name: inited ? "已初始化" : "未初始化"}]};
    },
    category: function(tid, pg) {
        return {list: [{vod_id: "c-" + tid, vod_name: "cat"}]};
    },
    detail: function(id) {
        return {list: [{vod_id: id, vod_name: "detail", vod_play_from: "", vod_play_url: ""}]};
    },
    play: function(flag, id, vipFlags) {
        return {url: "https://p/" + id, parse: 1};
    }
};
function __jsEvalReturn() { return spider; }
"#;
    let site = test_site(&inline(script));
    let cfg = empty_cfg();

    let cats = js::categories(&site, &cfg).expect("__jsEvalReturn home()");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "c1");
    assert_eq!(cats[0].type_name, "已初始化", "init(cfg) 应收到 stype/skey");

    let items = js::category(&site, &cfg, "9", 1).expect("category()");
    assert_eq!(items[0].id, "c-9");

    let detail = js::detail(&site, &cfg, "d1").expect("detail()");
    assert_eq!(detail.name, "detail");
    assert!(detail.episodes.is_empty());

    let play = js::player_content(&site, &cfg, "f", "p1", &[]).expect("play()");
    assert_eq!(
        play.get("url").and_then(|v| v.as_str()),
        Some("https://p/p1")
    );
    assert_eq!(play.get("parse").and_then(|v| v.as_i64()), Some(1));
}

/* ============================================================
 * 3. export default 形态（加载前重写）
 * ============================================================ */

#[test]
fn export_default_object() {
    let script = r#"
export default {
    home: function(filter) { return {class: [{type_id: "e1", type_name: "导出"}]}; },
    category: function(tid, pg) { return {list: [{vod_id: "e-" + tid, vod_name: "导出列表"}]}; }
};
"#;
    let site = test_site(&inline(script));
    let cfg = empty_cfg();

    let cats = js::categories(&site, &cfg).expect("export default home()");
    assert_eq!(cats[0].type_id, "e1");

    let items = js::category(&site, &cfg, "e", 1).expect("category()");
    assert_eq!(items[0].id, "e-e");
}

/* ============================================================
 * 4. 缺函数 / 异常的行为
 * ============================================================ */

#[test]
fn missing_functions_degrade_as_designed() {
    let script = r#"
function home(filter) { return {list: []}; }
function category(tid, pg) { return {list: []}; }
"#;
    let site = test_site(&inline(script));
    let cfg = empty_cfg();

    // search 缺失 → 空数组（不算错误）
    assert!(js::search(&site, &cfg, "x", false)
        .expect("search 缺函数应为空")
        .is_empty());
    // detail 缺失 → 明确错误
    let err = js::detail(&site, &cfg, "1").expect_err("detail 缺函数应报错");
    assert!(err.to_string().contains("未实现 detail"), "实际: {err}");
    // play 缺失 → 明确错误
    let err = js::player_content(&site, &cfg, "f", "1", &[]).expect_err("play 缺函数应报错");
    assert!(err.to_string().contains("未实现 play"), "实际: {err}");
}

#[test]
fn script_without_home_errors() {
    let site = test_site(&inline("function category(tid, pg) { return {list: []}; }"));
    let err = js::categories(&site, &empty_cfg()).expect_err("没有 home 应报错");
    assert!(err.to_string().contains("未实现 home"), "实际: {err}");
}

#[test]
fn script_exception_becomes_error() {
    let script = r#"
function home(filter) { throw new Error("boom-home"); }
function category(tid, pg) { return {list: []}; }
"#;
    let site = test_site(&inline(script));
    let err = js::categories(&site, &empty_cfg()).expect_err("抛异常应报错");
    assert!(err.to_string().contains("boom-home"), "实际: {err}");
}

/* ============================================================
 * 5. req：GET + 响应头/状态码/内容
 * ============================================================ */

#[test]
fn req_get_hits_mock_server() {
    let server = MockServer::start(|req| {
        if req.path == "/api" {
            MockReply::json(
                r#"{"code":1,"list":[{"vod_id":"7","vod_name":"来自req","vod_pic":"p.jpg","vod_remarks":"备注"}]}"#,
            )
        } else {
            MockReply {
                status: 404,
                content_type: "text/plain",
                body: "not found".into(),
            }
        }
    });

    let script = format!(
        r#"
function category(tid, pg) {{
    var r = req("{}/api");
    if (r.code !== 200) return {{list: []}};
    var data = JSON.parse(r.content);
    var ct = r.headers["content-type"] || "";
    return {{list: [
        {{vod_id: data.list[0].vod_id, vod_name: data.list[0].vod_name, vod_pic: data.list[0].vod_pic, vod_remarks: ct}}
    ]}};
}}
"#,
        server.base
    );
    let site = test_site(&inline(&script));
    let items = js::category(&site, &empty_cfg(), "1", 1).expect("req GET");

    assert_eq!(items[0].id, "7");
    assert_eq!(items[0].name, "来自req");
    assert_eq!(items[0].cover.as_deref(), Some("p.jpg"));
    assert!(
        items[0]
            .remark
            .as_deref()
            .unwrap_or("")
            .contains("application/json"),
        "应带回 content-type 响应头: {:?}",
        items[0].remark
    );

    let captured = server.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].method, "GET");
    assert_eq!(captured[0].path, "/api");
}

/* ============================================================
 * 6. req：POST + 请求头 + data(JSON)
 * ============================================================ */

#[test]
fn req_post_with_headers_and_json_data() {
    let server = MockServer::start(|req| {
        if req.path == "/echo" {
            let token = req.headers.get("x-token").cloned().unwrap_or_default();
            MockReply::json(
                json!({
                    "method": req.method,
                    "token": token,
                    "body": req.body,
                })
                .to_string(),
            )
        } else {
            MockReply {
                status: 404,
                content_type: "text/plain",
                body: "not found".into(),
            }
        }
    });

    let script = format!(
        r#"
function category(tid, pg) {{
    var r = req("{}/echo", {{method: "post", headers: {{"X-Token": "abc"}}, data: {{a: 1, b: "x"}}, postType: "json"}});
    var j = JSON.parse(r.content);
    return {{list: [{{vod_id: String(r.code), vod_name: j.method + "|" + j.token + "|" + j.body}}]}};
}}
"#,
        server.base
    );
    let site = test_site(&inline(&script));
    let items = js::category(&site, &empty_cfg(), "1", 1).expect("req POST");

    assert_eq!(items[0].id, "200");
    let name = &items[0].name;
    assert!(name.starts_with("POST|abc|"), "实际: {name}");
    assert!(name.contains("\"a\":1"), "实际: {name}");
    assert!(name.contains("\"b\":\"x\""), "实际: {name}");

    let captured = server.captured();
    assert_eq!(captured[0].method, "POST");
    assert!(captured[0]
        .headers
        .get("content-type")
        .map(|v| v.contains("application/json"))
        .unwrap_or(false));
}

/* ============================================================
 * 7. 脚本来源：http(s) 与 file://
 * ============================================================ */

#[test]
fn script_loaded_from_http() {
    let script = r#"
function home(filter) { return {class: [{type_id: "h1", type_name: "HTTP源"}]}; }
function category(tid, pg) { return {list: [{vod_id: "http-" + pg, vod_name: "HTTP列表"}]}; }
"#
    .to_string();

    let server = MockServer::start(move |req| match req.path.as_str() {
        "/spider.js" => MockReply::js(script.clone()),
        _ => MockReply {
            status: 404,
            content_type: "text/plain",
            body: "not found".into(),
        },
    });

    let site = test_site(&format!("{}/spider.js", server.base));
    let cfg = empty_cfg();

    let cats = js::categories(&site, &cfg).expect("http 脚本 home()");
    assert_eq!(cats[0].type_id, "h1");

    let items = js::category(&site, &cfg, "1", 2).expect("http 脚本 category()");
    assert_eq!(items[0].id, "http-2");
}

/// 真实链路：source 分发层（type=3 + .js api）→ sources/spider.rs → spider/js.rs
#[test]
fn routes_through_source_dispatch() {
    let script = r#"
function home(filter) { return {class: [{type_id: "d1", type_name: "分发"}]}; }
function category(tid, pg) { return {list: [{vod_id: "d-" + pg, vod_name: "分发列表"}]}; }
"#
    .to_string();

    let server = MockServer::start(move |req| match req.path.as_str() {
        "/spider.js" => MockReply::js(script.clone()),
        _ => MockReply {
            status: 404,
            content_type: "text/plain",
            body: "not found".into(),
        },
    });

    let site = test_site(&format!("{}/spider.js", server.base));
    let cfg = TvConfig {
        sites: vec![site],
        ..Default::default()
    };

    let cats = vbox_core::source::list_categories(&cfg, "test").expect("source 分发 home()");
    assert_eq!(cats[0].type_id, "d1");

    let items =
        vbox_core::source::list_videos(&cfg, "test", "1", 1).expect("source 分发 category()");
    assert_eq!(items[0].id, "d-1");
}

#[test]
fn script_loaded_from_file() {
    let dir = std::env::temp_dir().join(format!(
        "vbox_js_spider_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let path = dir.join("spider.js");
    std::fs::write(
        &path,
        "function home(filter){ return {class:[{type_id:\"f1\",type_name:\"文件源\"}]}; }\n",
    )
    .expect("写临时脚本");

    let api = format!("file:///{}", path.to_string_lossy().replace('\\', "/"));
    let site = test_site(&api);
    let cats = js::categories(&site, &empty_cfg()).expect("file 脚本 home()");
    assert_eq!(cats[0].type_id, "f1");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

/* ============================================================
 * 8. 产物 / API：md5、base64、joinUrl、local、setTimeout
 * ============================================================ */

#[test]
fn host_helpers_md5_base64_join_local() {
    let script = r#"
var _deferred = "";
function home(filter) {
    local.set("k", "v1");
    setTimeout(function(){ _deferred = "timer"; }, 10);
    return {
        class: [
            {type_id: md5("abc"), type_name: base64Encode("电影")},
            {type_id: base64Decode(base64Encode("hello")), type_name: joinUrl("https://a.com/x/y.html", "../z.html")},
            {type_id: local.get("k") + "-" + local.get("missing", "def") + "-" + _deferred, type_name: "本地"}
        ]
    };
}
function category(tid, pg) { return {list: []}; }
"#;
    let site = test_site(&inline(script));
    let cats = js::categories(&site, &empty_cfg()).expect("helpers");
    assert_eq!(cats[0].type_id, "900150983cd24fb0d6963f7d28e17f72", "md5");
    assert_eq!(cats[0].type_name, "55S15b2x", "base64Encode(电影)");
    assert_eq!(cats[1].type_id, "hello");
    assert_eq!(cats[1].type_name, "https://a.com/z.html");
    assert_eq!(cats[2].type_id, "v1-def-timer");
}

/* ============================================================
 * async / Promise 入口
 * ============================================================ */

#[test]
fn async_home_with_await_req() {
    let mock = MockServer::start(|req| {
        if req.path.starts_with("/home") {
            MockReply::json(r#"{"class":[{"type_id":"1","type_name":"异步"}]}"#)
        } else {
            MockReply { status: 404, content_type: "text/plain", body: "not found".into() }
        }
    });
    let base = mock.base.clone();
    let script = format!(
        r#"
async function home() {{
    const r = await req("{base}/home", {{}});
    return JSON.parse(r.content);
}}
function category(tid, pg) {{ return {{list: []}}; }}
"#
    );
    let site = test_site(&inline(&script));
    let cats = js::categories(&site, &empty_cfg()).expect("async home() 分类");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "1");
    assert_eq!(cats[0].type_name, "异步");
}

#[test]
fn async_entry_promise_object() {
    let script = r#"
function __jsEvalReturn() {
    return Promise.resolve({
        home: function () { return {class: [{type_id: "9", type_name: "Promise入口"}]}; },
        category: function (tid, pg) { return {list: []}; }
    });
}
"#;
    let site = test_site(&inline(script));
    let cats = js::categories(&site, &empty_cfg()).expect("Promise 入口对象");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "9");
}

#[test]
fn async_rejection_becomes_error() {
    let script = r#"
async function home() { throw new Error("boom"); }
function category(tid, pg) { return {list: []}; }
"#;
    let site = test_site(&inline(script));
    let err = js::categories(&site, &empty_cfg()).expect_err("拒绝的 Promise 应报错");
    let text = format!("{err}");
    assert!(text.contains("boom") || text.contains("Promise"), "实际错误: {text}");
}

/* ============================================================
 * ES module / CommonJS 模块加载
 * ============================================================ */

#[test]
fn esm_relative_import_and_named_export() {
    let mock = MockServer::start(|req| match req.path.as_str() {
        "/esm/entry.js" => MockReply::js(
            r#"
import { title, types } from "./lib/meta.js";

function home() {
    return { class: types.map(function (t) { return { type_id: t, type_name: title + ":" + t }; }) };
}
function category(tid, pg) { return { list: [] }; }
"#,
        ),
        "/esm/lib/meta.js" => MockReply::js(
            r#"export const title = "ESM标题";
export const types = ["电影", "剧集"];
export default { title: title, types: types };"#,
        ),
        _ => MockReply {
            status: 404,
            content_type: "text/plain",
            body: "404".into(),
        },
    });
    let site = test_site(&format!("{}/esm/entry.js", mock.base));
    let cats = js::categories(&site, &empty_cfg()).expect("ESM 入口分类");
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0].type_id, "电影");
    assert_eq!(cats[0].type_name, "ESM标题:电影");
    assert_eq!(cats[1].type_name, "ESM标题:剧集");
    assert!(
        mock.captured().iter().any(|c| c.path == "/esm/lib/meta.js"),
        "依赖模块应被请求: {:?}",
        mock.captured().iter().map(|c| c.path.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn cjs_require_relative_module() {
    let mock = MockServer::start(|req| match req.path.as_str() {
        "/cjs/entry.js" => MockReply::js(
            r#"const lib = require("./lib/util.js");
function home() { return { class: lib.classes() }; }
function category(tid, pg) { return { list: [] }; }"#,
        ),
        "/cjs/lib/util.js" => MockReply::js(
            r#"module.exports = { classes: function () { return [{ type_id: "c1", type_name: "CJS" }]; } };"#,
        ),
        _ => MockReply {
            status: 404,
            content_type: "text/plain",
            body: "404".into(),
        },
    });
    let site = test_site(&format!("{}/cjs/entry.js", mock.base));
    let cats = js::categories(&site, &empty_cfg()).expect("CJS require 分类");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "c1");
    assert_eq!(cats[0].type_name, "CJS");
}

#[test]
fn entry_module_exports_maps_to_js_spider() {
    let mock = MockServer::start(|req| match req.path.as_str() {
        "/mod/entry.js" => MockReply::js(
            r#"const meta = require("./lib/meta.js");
module.exports = {
    home: function () { return { class: [{ type_id: "m1", type_name: meta.name }] }; },
    category: function () { return { list: [] }; }
};"#,
        ),
        "/mod/lib/meta.js" => MockReply::js(r#"module.exports = { name: "模块导出" };"#),
        _ => MockReply {
            status: 404,
            content_type: "text/plain",
            body: "404".into(),
        },
    });
    let site = test_site(&format!("{}/mod/entry.js", mock.base));
    let cats = js::categories(&site, &empty_cfg()).expect("module.exports 入口");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "m1");
    assert_eq!(cats[0].type_name, "模块导出");
}

#[test]
fn builtin_cat_js_named_exports() {
    let mock = MockServer::start(|req| match req.path.as_str() {
        "/cat/entry.js" => MockReply::js(
            r#"
import { load, text } from "cat.js";
function home() {
    return { class: [{ type_id: typeof load, type_name: typeof text }] };
}
function category(tid, pg) { return { list: [] }; }
"#,
        ),
        _ => MockReply {
            status: 404,
            content_type: "text/plain",
            body: "404".into(),
        },
    });
    let site = test_site(&format!("{}/cat/entry.js", mock.base));
    let cats = js::categories(&site, &empty_cfg()).expect("cat.js 内置 lib 导入");
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0].type_id, "function");
}
