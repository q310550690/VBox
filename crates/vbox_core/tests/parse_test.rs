//! 集成测试：parses 解析链（type1 接口 / type4 超解析 / type0 嗅探）
//!
//! 全部使用本地 tiny_http mock（127.0.0.1:0），不访问真实网络。覆盖：
//! - type1 正常响应：`{data:{url,parse,header}}`、ext.header 合并、cat_ext 透传；
//! - type1 `parse=1`：返回播放页并 `sniff=true`；
//! - type1 失败（HTTP 500）降级到下一个解析器；
//! - type4 两个接口一慢一快：取首个直链，慢接口不阻塞返回；
//! - type0：返回 `parse.url + 播放页` 且 `sniff=true`，不发 HTTP；
//! - type4 没有 type1 时回退 type0；
//! - 协议前缀归一：`//`、`video://`、`proxy://`、header 大小写。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use tiny_http::{Header, Response, Server, StatusCode};

use vbox_core::model::{Parser, Rules, TvConfig};
use vbox_core::parse::{
    is_direct_media_url, parse_json_play_result, run_parse_chain, run_parse_chain_with,
};
use vbox_core::progress::{ProgressEvent, ProgressSink};

/* ============================================================
 * 本地 mock HTTP server
 * ============================================================ */

#[derive(Clone)]
struct Route {
    status: u16,
    delay: Duration,
    body: String,
}

impl Route {
    fn json(body: &str) -> Self {
        Self {
            status: 200,
            delay: Duration::ZERO,
            body: body.to_string(),
        }
    }

    fn slow_json(delay: Duration, body: &str) -> Self {
        Self {
            status: 200,
            delay,
            body: body.to_string(),
        }
    }

    fn status(code: u16) -> Self {
        Self {
            status: code,
            delay: Duration::ZERO,
            body: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct SeenRequest {
    url: String,
    headers: HashMap<String, String>,
}

impl SeenRequest {
    fn query(&self) -> &str {
        self.url.split_once('?').map(|(_, q)| q).unwrap_or("")
    }

    fn query_param(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}=");
        self.query()
            .split('&')
            .find_map(|kv| kv.strip_prefix(prefix.as_str()))
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn path(&self) -> &str {
        self.url.split('?').next().unwrap_or("")
    }
}

/// 极简 mock server：每个请求一个 handler 线程，慢接口不会阻塞快接口。
struct MockServer {
    base: String,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
    shutdown: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl MockServer {
    fn start(routes: HashMap<&'static str, Route>) -> Self {
        let routes: HashMap<String, Route> = routes
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let server = Server::http("127.0.0.1:0").expect("启动 mock server");
        let port = server.server_addr().to_ip().expect("TCP 监听地址").port();
        let base = format!("http://127.0.0.1:{port}");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let routes = Arc::new(routes);

        let accept = {
            let seen = Arc::clone(&seen);
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                while !shutdown.load(Ordering::SeqCst) {
                    match server.recv_timeout(Duration::from_millis(20)) {
                        Ok(Some(req)) => {
                            let routes = Arc::clone(&routes);
                            let seen = Arc::clone(&seen);
                            std::thread::spawn(move || handle_request(req, routes, seen));
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            })
        };

        Self {
            base,
            seen,
            shutdown,
            accept: Some(accept),
        }
    }

    fn url(&self, path_and_query: &str) -> String {
        format!("{}{}", self.base, path_and_query)
    }

    fn requests(&self) -> Vec<SeenRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }
    }
}

fn handle_request(
    req: tiny_http::Request,
    routes: Arc<HashMap<String, Route>>,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
) {
    let url = req.url().to_string();
    let mut headers = HashMap::new();
    for header in req.headers() {
        headers.insert(header.field.to_string(), header.value.as_str().to_string());
    }
    seen.lock().unwrap().push(SeenRequest {
        url: url.clone(),
        headers,
    });

    let path = url.split('?').next().unwrap_or("").to_string();
    let Some(route) = routes.get(&path).cloned() else {
        let _ = req.respond(Response::from_string("not found").with_status_code(StatusCode(404)));
        return;
    };
    if !route.delay.is_zero() {
        std::thread::sleep(route.delay);
    }
    let content_type = Header::from_bytes(
        &b"Content-Type"[..],
        &b"application/json; charset=utf-8"[..],
    )
    .unwrap();
    let response = Response::from_string(route.body)
        .with_status_code(StatusCode(route.status))
        .with_header(content_type);
    let _ = req.respond(response);
}

/* ============================================================
 * 测试配置构造
 * ============================================================ */

fn test_rules() -> Rules {
    Rules {
        timeout: 3000,
        retry: 0,
        ua: "VBoxParseTest/1.0".to_string(),
        headers: HashMap::new(),
    }
}

fn test_cfg(parses: Vec<Parser>) -> TvConfig {
    TvConfig {
        parses,
        http: test_rules(),
        ..Default::default()
    }
}

fn parser(name: &str, kind: i32, url: String, ext: Option<Value>) -> Parser {
    Parser {
        name: name.to_string(),
        kind,
        url,
        ext,
    }
}

/* ============================================================
 * type=1 解析接口
 * ============================================================ */

#[test]
fn type1_data_url_headers_and_cat_ext() {
    let server = MockServer::start(HashMap::from([(
        "/parse",
        Route::json(
            r#"{"data":{"url":"https://cdn.example.com/movie.m3u8","header":{"Referer":"https://vip.example.com/","User-Agent":"RespUA"},"parse":0}}"#,
        ),
    )]));
    let ext = json!({"header": {"User-Agent": "ExtUA", "X-Token": "abc"}});
    let cfg = test_cfg(vec![parser(
        "接口甲",
        1,
        server.url("/parse?url="),
        Some(ext.clone()),
    )]);
    let base_headers = HashMap::from([("X-Base".to_string(), "1".to_string())]);

    let result = run_parse_chain(
        &cfg,
        "https://vip.example.com/play/1",
        None,
        &base_headers,
        None,
    )
    .expect("type1 解析应成功");

    assert_eq!(result.url, "https://cdn.example.com/movie.m3u8");
    assert!(!result.sniff, "parse=0 不应标记嗅探");
    assert_eq!(result.parse_name.as_deref(), Some("接口甲"));
    assert_eq!(result.headers.get("X-Base").map(String::as_str), Some("1"));
    assert_eq!(
        result.headers.get("Referer").map(String::as_str),
        Some("https://vip.example.com/")
    );
    assert_eq!(
        result.headers.get("User-Agent").map(String::as_str),
        Some("RespUA")
    );

    let requests = server.requests();
    let req = requests
        .iter()
        .find(|r| r.path() == "/parse")
        .expect("应收到解析接口请求");
    // ext.header 合并进请求头（大小写归一）
    assert_eq!(req.header("User-Agent"), Some("ExtUA"));
    assert_eq!(req.header("X-Token"), Some("abc"));
    // Java URLEncoder(播放页)
    assert_eq!(
        req.query_param("url"),
        Some("https%3A%2F%2Fvip.example.com%2Fplay%2F1")
    );
    // cat_ext 透传：Base64URL(ext JSON)
    let cat_ext = req.query_param("cat_ext").expect("应携带 cat_ext");
    let decoded = base64::engine::general_purpose::URL_SAFE
        .decode(cat_ext)
        .expect("cat_ext 应为 URL_SAFE base64");
    let decoded_json: Value = serde_json::from_slice(&decoded).expect("cat_ext 应解出 JSON");
    assert_eq!(decoded_json, ext);
}

#[test]
fn type1_parse_flag_returns_sniff_play_page() {
    let server = MockServer::start(HashMap::from([(
        "/p",
        Route::json(r#"{"url":"https://page.example.com/play?id=1","parse":1}"#),
    )]));
    let cfg = test_cfg(vec![parser("接口乙", 1, server.url("/p?url="), None)]);

    let result = run_parse_chain(
        &cfg,
        "https://vip.example.com/play/2",
        None,
        &HashMap::new(),
        None,
    )
    .expect("parse=1 应返回播放页");

    assert!(result.sniff, "parse=1 必须标记 sniff");
    assert_eq!(result.url, "https://page.example.com/play?id=1");
    assert_eq!(result.parse_name.as_deref(), Some("接口乙"));
}

#[test]
fn type1_failure_falls_back_to_next_parser() {
    let server = MockServer::start(HashMap::from([
        ("/bad", Route::status(500)),
        (
            "/ok",
            Route::json(r#"{"data":{"url":"https://cdn.example.com/ok.mp4"}}"#),
        ),
    ]));
    let cfg = test_cfg(vec![
        parser("坏接口", 1, server.url("/bad?url="), None),
        parser("好接口", 1, server.url("/ok?url="), None),
    ]);

    let result = run_parse_chain(
        &cfg,
        "https://vip.example.com/play/3",
        None,
        &HashMap::new(),
        None,
    )
    .expect("应降级到下一个解析器");

    assert_eq!(result.url, "https://cdn.example.com/ok.mp4");
    assert_eq!(result.parse_name.as_deref(), Some("好接口"));
    assert!(
        result
            .trace
            .iter()
            .any(|t| t.contains("坏接口") && t.contains("失败")),
        "trace 应记录首个解析器失败: {:?}",
        result.trace
    );
    let paths: Vec<String> = server
        .requests()
        .into_iter()
        .map(|r| r.path().to_string())
        .collect();
    assert!(paths.contains(&"/bad".to_string()));
    assert!(paths.contains(&"/ok".to_string()));
}

/* ============================================================
 * type=4 超级解析
 * ============================================================ */

#[test]
fn type4_super_parse_returns_first_direct_link() {
    let server = MockServer::start(HashMap::from([
        (
            "/fast",
            Route::json(r#"{"data":{"url":"https://cdn.example.com/fast.m3u8"}}"#),
        ),
        (
            "/slow",
            Route::slow_json(
                Duration::from_millis(1000),
                r#"{"data":{"url":"https://cdn.example.com/slow.m3u8"}}"#,
            ),
        ),
    ]));
    let cfg = test_cfg(vec![
        parser("超级解析", 4, "SuperParse".to_string(), None),
        parser("快接口", 1, server.url("/fast?url="), None),
        parser("慢接口", 1, server.url("/slow?url="), None),
    ]);

    let started = Instant::now();
    let result = run_parse_chain(
        &cfg,
        "https://vip.example.com/play/4",
        None,
        &HashMap::new(),
        None,
    )
    .expect("超解析应命中直链");
    let elapsed = started.elapsed();

    assert_eq!(result.url, "https://cdn.example.com/fast.m3u8");
    assert!(!result.sniff);
    assert_eq!(result.parse_name.as_deref(), Some("超级解析"));
    assert_eq!(result.jx_from.as_deref(), Some("快接口"));
    assert!(
        elapsed < Duration::from_millis(600),
        "慢接口不应阻塞返回，实际耗时 {elapsed:?}"
    );
    assert!(
        result.trace.iter().any(|t| t.contains("超级解析")),
        "trace 应记录超解析: {:?}",
        result.trace
    );
}

#[test]
fn type4_without_type1_falls_back_to_type0_sniff() {
    let cfg = test_cfg(vec![
        parser("超级解析", 4, "SuperParse".to_string(), None),
        parser(
            "嗅探解析",
            0,
            "https://jx.example.com/?url=".to_string(),
            None,
        ),
    ]);

    let result = run_parse_chain(
        &cfg,
        "https://vip.example.com/play/6",
        None,
        &HashMap::new(),
        None,
    )
    .expect("没有 type1 时应回退 type0");

    assert!(result.sniff);
    assert_eq!(
        result.url,
        "https://jx.example.com/?url=https://vip.example.com/play/6"
    );
    assert_eq!(result.parse_name.as_deref(), Some("超级解析"));
    assert_eq!(result.jx_from.as_deref(), Some("嗅探解析"));
}

/* ============================================================
 * type=0 嗅探 / JSON 归一
 * ============================================================ */

#[test]
fn type0_returns_sniff_play_page_without_http() {
    let cfg = test_cfg(vec![parser(
        "嗅探解析",
        0,
        "https://jx.example.com/?url=".to_string(),
        None,
    )]);

    let result = run_parse_chain(
        &cfg,
        "https://vip.example.com/play/5",
        Some("嗅探解析"),
        &HashMap::new(),
        None,
    )
    .expect("type0 应直接返回播放页");

    assert!(result.sniff);
    assert_eq!(
        result.url,
        "https://jx.example.com/?url=https://vip.example.com/play/5"
    );
    assert_eq!(result.parse_name.as_deref(), Some("嗅探解析"));
}

#[test]
fn json_result_protocol_and_header_normalization() {
    let headers = HashMap::new();

    // `//` 补 http:
    let r = parse_json_play_result(
        &json!({"data": {"url": "//cdn.example.com/x.m3u8"}}),
        "input",
        &headers,
    )
    .expect("// URL 应可用");
    assert_eq!(r.url, "http://cdn.example.com/x.m3u8");
    assert!(!r.sniff);

    // `video://` 去前缀并标记 sniff
    let r = parse_json_play_result(
        &json!({"url": "video://https://cdn.example.com/y.mp4"}),
        "input",
        &headers,
    )
    .expect("video:// URL 应可用");
    assert_eq!(r.url, "https://cdn.example.com/y.mp4");
    assert!(r.sniff);

    // `proxy://` 映射到本地 9978
    let r = parse_json_play_result(
        &json!({"url": "proxy://go=live&type=txt&ext=abc"}),
        "input",
        &headers,
    )
    .expect("proxy:// URL 应可用");
    assert_eq!(
        r.url,
        "http://127.0.0.1:9978/proxy?go=live&type=txt&ext=abc"
    );

    // header 大小写归一 + parse=1 + 根级 jxFrom
    let r = parse_json_play_result(
        &json!({
            "data": {"url": "https://cdn.example.com/z.m3u8"},
            "headers": {"referer": "https://a.com", "user-agent": "UA2"},
            "parse": 1,
            "jxFrom": "某接口"
        }),
        "input",
        &headers,
    )
    .expect("header 归一后应可用");
    assert!(r.sniff);
    assert_eq!(
        r.headers.get("Referer").map(String::as_str),
        Some("https://a.com")
    );
    assert_eq!(r.headers.get("User-Agent").map(String::as_str), Some("UA2"));
    assert_eq!(r.jx_from.as_deref(), Some("某接口"));

    // 非 http(s)/data:application 丢弃
    assert!(parse_json_play_result(&json!({"url": "ftp://x/y.mp4"}), "input", &headers).is_none());
    assert!(parse_json_play_result(&json!({"data": {}}), "input", &headers).is_none());
}

#[test]
fn direct_media_url_short_circuits_without_parsers() {
    let cfg = test_cfg(vec![]);
    let result = run_parse_chain(
        &cfg,
        "https://cdn.example.com/a.M3U8?token=1",
        None,
        &HashMap::new(),
        None,
    )
    .expect("直链应直接返回");
    assert_eq!(result.url, "https://cdn.example.com/a.M3U8?token=1");
    assert!(!result.sniff);
    assert!(is_direct_media_url("https://cdn.example.com/a.mp4#t=1"));
    assert!(!is_direct_media_url("https://vip.example.com/play/1"));
}

/* ============================================================
 * 解析进度（parse-progress 的事件源）+ 取消
 * ============================================================ */

fn collect_sink() -> (ProgressSink, Arc<Mutex<Vec<ProgressEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = ProgressSink::new({
        let events = Arc::clone(&events);
        Arc::new(move |event: ProgressEvent| {
            events.lock().unwrap().push(event);
        })
    });
    (sink, events)
}

fn find_event<'a>(events: &'a [ProgressEvent], phase: &str, status: &str) -> &'a ProgressEvent {
    events
        .iter()
        .find(|e| e.phase == phase && e.status == status)
        .unwrap_or_else(|| panic!("应发出 {phase}/{status} 事件: {events:?}"))
}

#[test]
fn progress_events_report_prepare_request_and_hit() {
    let server = MockServer::start(HashMap::from([(
        "/ok",
        Route::json(r#"{"data":{"url":"https://cdn.example.com/p.m3u8"}}"#),
    )]));
    let cfg = test_cfg(vec![parser("接口丙", 1, server.url("/ok?url="), None)]);
    let (sink, events) = collect_sink();

    let result = run_parse_chain_with(
        &cfg,
        "https://vip.example.com/play/9",
        None,
        &HashMap::new(),
        None,
        &sink,
    )
    .expect("带进度回调的解析应成功");
    assert_eq!(result.url, "https://cdn.example.com/p.m3u8");

    let events = events.lock().unwrap();
    // 关键节点：开始 → 解析器尝试 → 请求接口 → 接口返回 → 解析器命中
    assert!(find_event(&events, "prepare", "start").title.contains("开始解析"));
    let parser_start = find_event(&events, "parser", "start");
    assert!(parser_start.title.contains("接口丙"), "{parser_start:?}");
    let request_start = find_event(&events, "request", "start");
    assert!(
        request_start
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("/ok"),
        "请求事件应带接口地址: {request_start:?}"
    );
    assert!(
        find_event(&events, "request", "ok").detail.is_some(),
        "返回事件应带耗时/字节数"
    );
    let hit = find_event(&events, "parser", "ok");
    assert!(hit.title.contains("接口丙") && hit.title.contains("命中"));
    assert_eq!(hit.detail.as_deref(), Some("https://cdn.example.com/p.m3u8"));

    // elapsed_ms 单调不减（事件从多个线程汇总，但不能倒退）
    let mut prev = 0;
    for event in events.iter() {
        assert!(event.elapsed_ms >= prev, "elapsed 不应倒退: {events:?}");
        prev = event.elapsed_ms;
    }
}

#[test]
fn cancelled_sink_stops_before_any_http_request() {
    let server = MockServer::start(HashMap::from([(
        "/ok",
        Route::json(r#"{"data":{"url":"https://cdn.example.com/c.m3u8"}}"#),
    )]));
    let cfg = test_cfg(vec![parser("接口丁", 1, server.url("/ok?url="), None)]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let cancel = Arc::new(AtomicBool::new(true));
    let sink = ProgressSink::new_with_cancel(
        {
            let events = Arc::clone(&events);
            Arc::new(move |event: ProgressEvent| {
                events.lock().unwrap().push(event);
            })
        },
        Arc::clone(&cancel),
    );

    let err = run_parse_chain_with(
        &cfg,
        "https://vip.example.com/play/10",
        None,
        &HashMap::new(),
        None,
        &sink,
    )
    .expect_err("取消后不应继续解析");
    assert!(err.to_string().contains("取消"), "错误文案应说明已取消: {err}");

    assert!(
        server.requests().iter().all(|r| r.path() != "/ok"),
        "取消后不应再发起解析接口请求"
    );
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.phase == "error" && e.status == "fail" && e.title.contains("取消")),
        "应发出取消事件: {events:?}"
    );
}
