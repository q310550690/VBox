//! parses 解析链：把"播放页 URL"转换为真实直链
//!
//! 对齐 jun 版 `PlayFragment.doParse` / `SuperParse` / `JsonParallel` / `Utils.jsonParse`
//! 的真实语义：
//! - type=0：WebView 嗅探。桌面端返回 `parse.url + 播放页` 并置 `sniff=true`，交给抓包模块；
//! - type=1：解析接口。`GET parse.url(可带 cat_ext) + URLEncoder(播放页)`，响应归一成
//!   `{data:{url,header,parse}}` 或根字段；`parse=1` 表示返回的仍是播放页，需要再嗅探；
//! - type=2/3：jar 内 `Json<url>` / `Mix<url>`（桌面端无法运行 Android dex，返回明确错误）；
//! - type=4：超级解析。并发所有 type1 接口（并发上限 5），首个有效直链先到先得；
//!   没有 type1 时回退 type0 播放页嗅探。
//!
//! 本模块只依赖 `vbox_core::http`，保持 blocking；并发用后台线程 + `std::thread::scope`。
//! 与 TVBox 不同的是：桌面端拿到首个直链后立即返回，不在主线程等待落败的慢接口
//! （scope 留在一个 detached 的调度线程里收尾；未完成的任务不会 panic）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use serde_json::{Map, Value};

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::{Parser, PlayResult, Site, TvConfig};
use crate::progress::{shorten, ProgressSink};
use crate::url_norm::java_url_encode;

/// 解析总超时（对齐 `PlayFragment.MSG_PARSE_TIMEOUT = 20s`）
pub const PARSE_TIMEOUT: Duration = Duration::from_secs(20);

/// 超解析并发上限（对齐 `JsonParallel` 的 `newFixedThreadPool(5)`）
pub const SUPER_PARSE_CONCURRENCY: usize = 5;

/// TVBox 本地代理端口（`ControlManager` 9978；桌面端后续可改成动态端口）
pub const LOCAL_PROXY_PORT: u16 = 9978;

/// 运行解析链。
///
/// - `preferred`: 用户选择的解析器 name（`default_parse` 或前端换解析）
/// - `base_headers`: 站点/播放信息返回的请求头
/// - `site`: 当前站点（type3 的 spider 判定/嗅探规则会用到；可为空）
pub fn run_parse_chain(
    cfg: &TvConfig,
    play_url: &str,
    preferred: Option<&str>,
    base_headers: &HashMap<String, String>,
    site: Option<&Site>,
) -> Result<PlayResult> {
    run_parse_chain_with(
        cfg,
        play_url,
        preferred,
        base_headers,
        site,
        &ProgressSink::none(),
    )
}

/// 带进度回调的解析链：桌面端用它把「尝试了哪个解析器 / 谁命中 / 请求耗时」推给 UI。
///
/// 参数与 [`run_parse_chain`] 完全一致，只是多一个 [`ProgressSink`]；空 sink
/// （[`ProgressSink::none`]）时行为与旧入口完全相同。
pub fn run_parse_chain_with(
    cfg: &TvConfig,
    play_url: &str,
    preferred: Option<&str>,
    base_headers: &HashMap<String, String>,
    site: Option<&Site>,
    progress: &ProgressSink,
) -> Result<PlayResult> {
    let play_url = play_url.trim();
    if play_url.is_empty() {
        progress.emit("prepare", "fail", "播放地址为空");
        return Err(CoreError::Config("播放地址为空".into()));
    }
    progress.emit_detail(
        "prepare",
        "start",
        "开始解析播放地址",
        Some(shorten(play_url, 180)),
    );

    // 已经是直链：直接返回（TVBox 同样先做直链判定）
    if is_direct_media_url(play_url) {
        progress.emit_detail(
            "prepare",
            "ok",
            "已是媒体直链，无需解析",
            Some(shorten(play_url, 180)),
        );
        return Ok(PlayResult {
            url: play_url.to_string(),
            headers: base_headers.clone(),
            ..Default::default()
        });
    }

    let parsers = ordered_parsers(cfg, preferred);
    if parsers.is_empty() {
        progress.emit("prepare", "fail", "配置里没有可用的解析器");
        return Err(CoreError::NoPlayUrl);
    }
    progress.emit_detail(
        "prepare",
        "info",
        format!("共 {} 个解析器，优先 {}", parsers.len(), parsers[0].name),
        Some(format!("目标：{}", shorten(play_url, 160))),
    );

    let mut trace = Vec::new();
    let mut last_err: Option<CoreError> = None;

    for parser in parsers {
        if progress.cancelled() {
            progress.emit("error", "fail", "已取消解析");
            return Err(CoreError::Config("解析已取消".into()));
        }
        progress.emit_detail(
            "parser",
            "start",
            format!("尝试解析器 {}", parser.name),
            Some(format!(
                "type={} {}",
                parser.kind,
                shorten(parser.url_raw(), 120)
            )),
        );
        trace.push(format!(
            "尝试解析器: {} (type={})",
            parser.name, parser.kind
        ));
        match run_one(cfg, parser, play_url, base_headers, site, progress) {
            Ok(mut result) => {
                progress.emit_detail(
                    "parser",
                    "ok",
                    format!("解析器 {} 命中", parser.name),
                    Some(shorten(&result.url, 180)),
                );
                let mut merged = std::mem::take(&mut trace);
                merged.extend(result.trace);
                result.trace = merged;
                if result.parse_name.is_none() {
                    result.parse_name = Some(parser.name.clone());
                }
                return Ok(result);
            }
            Err(e) => {
                progress.emit_detail(
                    "parser",
                    "fail",
                    format!("解析器 {} 失败", parser.name),
                    Some(e.to_string()),
                );
                trace.push(format!("解析器 {} 失败: {}", parser.name, e));
                last_err = Some(e);
            }
        }
    }

    let err = last_err.unwrap_or(CoreError::NoPlayUrl);
    progress.emit_detail("error", "fail", "全部解析器均未命中", Some(err.to_string()));
    Err(err)
}

/// 按"用户选择优先 + 配置顺序"排序
///
/// TVBox 的超级解析是 `ApiConfig.addSuperParse()` 运行时插到 index 0 的，配置里通常没有
/// type=4 条目；本层不做隐式注入（需要 config 层配合），因此顺序完全由配置决定。
fn ordered_parsers<'a>(cfg: &'a TvConfig, preferred: Option<&str>) -> Vec<&'a Parser> {
    let mut parsers: Vec<&Parser> = cfg.parses.iter().collect();
    if let Some(name) = preferred {
        if let Some(pos) = parsers.iter().position(|p| p.name == name) {
            let p = parsers.remove(pos);
            parsers.insert(0, p);
        }
    }
    parsers
}

fn run_one(
    cfg: &TvConfig,
    parser: &Parser,
    play_url: &str,
    base_headers: &HashMap<String, String>,
    site: Option<&Site>,
    progress: &ProgressSink,
) -> Result<PlayResult> {
    match parser.kind {
        0 => run_sniff_parser(parser, play_url, base_headers),
        1 => request_json_parser(cfg, parser, play_url, base_headers, "http:", progress),
        2 | 3 => Err(CoreError::Config(format!(
            "解析器 {} 使用 jar 内 Json/Mix 扩展（type={}），桌面端无法运行 Android dex",
            parser.name, parser.kind
        ))),
        4 => run_super_parse(cfg, parser, play_url, base_headers, site, progress),
        k => Err(CoreError::Config(format!(
            "解析器 {} 的类型 type={} 不受支持",
            parser.name, k
        ))),
    }
}

/// type=0：返回 `parse.url + 播放页`，`sniff=true`
///
/// 与 TVBox `loadWebView(pb.getUrl() + webUrl)` 一致，不做任何 HTTP 请求；真正的
/// m3u8/mp4 识别由桌面端的「抓包」模块（Tauri WebView 子资源拦截）完成。
fn run_sniff_parser(
    parser: &Parser,
    play_url: &str,
    base_headers: &HashMap<String, String>,
) -> Result<PlayResult> {
    let mut headers = base_headers.clone();
    headers.extend(parser.header_map());
    let headers = normalize_headers(headers);

    let prefix = check_replace_proxy(parser.url.trim());
    Ok(PlayResult {
        url: format!("{prefix}{play_url}"),
        headers,
        sniff: true,
        parse_name: Some(parser.name.clone()),
        ..Default::default()
    })
}

/// type=1 / 超解析的统一请求：拼接、发送、解析响应
///
/// `double_slash_prefix` 用于 `//` 开头的相对协议 URL：
/// - type1（`PlayFragment.jsonParse`）补 `http:`；
/// - 超解析（`Utils.jsonParse`）补 `https:`。
fn request_json_parser(
    cfg: &TvConfig,
    parser: &Parser,
    play_url: &str,
    base_headers: &HashMap<String, String>,
    double_slash_prefix: &str,
    progress: &ProgressSink,
) -> Result<PlayResult> {
    let mut request_headers = base_headers.clone();
    request_headers.extend(parser.header_map());
    let request_headers = normalize_headers(request_headers);

    // TVBox：`pb.getUrl() + URLEncoder.encode(webUrl)`；`pb.getUrl()` 先做 proxy:// 替换，
    // 有 ext 时再注入 cat_ext（ParseBean.mixUrl）。
    let base = parser_request_url(parser);
    let url = format!("{}{}", base, java_url_encode(play_url));

    progress.emit_detail(
        "request",
        "start",
        format!("请求解析接口（{}）", parser.name),
        Some(shorten(&url, 220)),
    );
    let started = Instant::now();
    let client = http::client_for(&cfg.http).map_err(|e| {
        CoreError::Config(format!("解析器 {} 创建 HTTP 客户端失败: {e}", parser.name))
    })?;
    let text = http::get_text(&client, &url, &request_headers, cfg.http.retry).map_err(|e| {
        progress.emit_detail(
            "request",
            "fail",
            format!("解析接口请求失败（{}）", parser.name),
            Some(e.to_string()),
        );
        CoreError::Config(format!("解析器 {} 请求失败: {e}", parser.name))
    })?;
    progress.emit_detail(
        "request",
        "ok",
        format!("解析接口返回（{}）", parser.name),
        Some(format!(
            "耗时 {} ms · {} 字节",
            started.elapsed().as_millis(),
            text.len()
        )),
    );
    let json: Value = serde_json::from_str(text.trim()).map_err(|e| {
        progress.emit_detail(
            "request",
            "fail",
            format!("解析接口返回不是 JSON（{}）", parser.name),
            Some(shorten(text.trim(), 120)),
        );
        CoreError::Config(format!("解析器 {} 返回不是 JSON: {e}", parser.name))
    })?;
    let mut result =
        parse_json_play_result_with(&json, play_url, base_headers, double_slash_prefix)
            .ok_or_else(|| {
                CoreError::Config(format!(
                    "解析器 {} 的返回中没有可用的 http(s)/data:application 地址",
                    parser.name
                ))
            })?;
    result.parse_name = Some(parser.name.clone());
    Ok(result)
}

/// 解析器请求基址：`proxy://` 替换 + `cat_ext` 注入
fn parser_request_url(parser: &Parser) -> String {
    let base = check_replace_proxy(parser.url_raw());
    let ext = parser.ext_string();
    mix_url(&base, ext.trim())
}

/// 对齐 `ParseBean.mixUrl()`：ext 非空且 URL 含 `?` 时，在 `?` 后插入
/// `cat_ext=<Base64URL(ext JSON)>`，把 ext（含 header）透传给解析接口。
pub fn mix_url(url: &str, ext: &str) -> String {
    let ext = ext.trim();
    if ext.is_empty() || ext == "null" || url.contains("cat_ext=") {
        return url.to_string();
    }
    match url.find('?') {
        Some(idx) if idx > 0 => format!(
            "{}cat_ext={}&{}",
            &url[..=idx],
            URL_SAFE.encode(ext.as_bytes()),
            &url[idx + 1..]
        ),
        _ => url.to_string(),
    }
}

/// 对齐 `DefaultConfig.checkReplaceProxy`：`proxy://` → 本地 9978 代理地址
pub fn check_replace_proxy(url: &str) -> String {
    match url.strip_prefix("proxy://") {
        Some(rest) => format!("http://127.0.0.1:{LOCAL_PROXY_PORT}/proxy?{rest}"),
        None => url.to_string(),
    }
}

/// 解析接口/超解析统一响应解析：`data.url` 或根 `url`，header/headers，
/// `user-agent`/`referer`，`parse=1` 表示还要嗅探。
///
/// 该函数保持旧签名（type1 的 `//` 语义补 `http:`），超解析内部走
/// [`parse_json_play_result_with`] 以使用 `https:`。
pub fn parse_json_play_result(
    json: &Value,
    play_url: &str,
    base_headers: &HashMap<String, String>,
) -> Option<PlayResult> {
    parse_json_play_result_with(json, play_url, base_headers, "http:")
}

fn parse_json_play_result_with(
    json: &Value,
    play_url: &str,
    base_headers: &HashMap<String, String>,
    double_slash_prefix: &str,
) -> Option<PlayResult> {
    let obj = json.as_object()?;
    let data = obj.get("data").and_then(Value::as_object).unwrap_or(obj);
    let raw_url = data
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            obj.get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })?;

    let mut url = raw_url.to_string();
    if let Some(rest) = url.strip_prefix("//") {
        url = format!("{double_slash_prefix}//{rest}");
    }
    let mut sniff = false;
    if let Some(rest) = url.strip_prefix("video://") {
        url = rest.to_string();
        sniff = true;
    }
    url = check_replace_proxy(&url);
    // PlayFragment.jsonParse：只接受 http(s) 或 data:application
    if !url.starts_with("http://")
        && !url.starts_with("https://")
        && !url.starts_with("data:application")
    {
        return None;
    }

    // data.parse 优先于根 parse（`data.optInt("parse", root.optInt("parse", 0))`）
    let parse_flag = read_parse_flag(data)
        .or_else(|| read_parse_flag(obj))
        .unwrap_or(false);
    if parse_flag {
        sniff = true;
    }

    let mut headers = base_headers.clone();
    for key in ["header", "headers"] {
        // 根字段先合并，data 覆盖（对齐 PlayFragment：先 root 后 data）
        append_headers(&mut headers, obj.get(key));
        append_headers(&mut headers, data.get(key));
    }
    if let Some(ua) = first_non_empty_str(data, obj, "user-agent") {
        headers.insert("User-Agent".to_string(), ua.trim().to_string());
    }
    if let Some(referer) = first_non_empty_str(data, obj, "referer") {
        headers.insert("Referer".to_string(), referer.trim().to_string());
    }
    let headers = normalize_headers(headers);

    let jx_from = first_non_empty_str(data, obj, "jxFrom").map(|s| s.to_string());

    let _ = play_url;
    Some(PlayResult {
        url,
        headers,
        sniff,
        jx_from,
        ..Default::default()
    })
}

fn read_parse_flag(obj: &Map<String, Value>) -> Option<bool> {
    obj.get("parse").map(|v| match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64() == Some(1),
        Value::String(s) => matches!(s.trim(), "1" | "true" | "TRUE" | "True"),
        _ => false,
    })
}

fn first_non_empty_str<'a>(
    data: &'a Map<String, Value>,
    root: &'a Map<String, Value>,
    key: &str,
) -> Option<&'a str> {
    for obj in [data, root] {
        if let Some(s) = obj.get(key).and_then(Value::as_str) {
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// 合并 `header` / `headers`：对象或 JSON 字符串都兼容，键名归一化
fn append_headers(headers: &mut HashMap<String, String>, raw: Option<&Value>) {
    let Some(raw) = raw else { return };
    let parsed: Value = match raw {
        Value::Object(_) => raw.clone(),
        Value::String(s) if !s.trim().is_empty() => match serde_json::from_str(s.trim()) {
            Ok(v) => v,
            Err(_) => return,
        },
        _ => return,
    };
    let Some(map) = parsed.as_object() else {
        return;
    };
    for (k, v) in map {
        let key = normalize_header_name(k);
        if key.is_empty() {
            continue;
        }
        let value = match v {
            Value::String(s) => s.trim().to_string(),
            Value::Null => continue,
            other => other.to_string(),
        };
        if value.is_empty() {
            continue;
        }
        headers.insert(key, value);
    }
}

/// `user-agent` / `referer` / `origin` / `cookie` 归一成标准大小写，
/// 避免播放器/reqwest 因大小写重复或读不到。
fn normalize_header_name(name: &str) -> String {
    let name = name.trim();
    if name.eq_ignore_ascii_case("user-agent") || name.eq_ignore_ascii_case("useragent") {
        "User-Agent".to_string()
    } else if name.eq_ignore_ascii_case("referer") || name.eq_ignore_ascii_case("referrer") {
        "Referer".to_string()
    } else if name.eq_ignore_ascii_case("origin") {
        "Origin".to_string()
    } else if name.eq_ignore_ascii_case("cookie") {
        "Cookie".to_string()
    } else {
        name.to_string()
    }
}

fn normalize_headers(headers: HashMap<String, String>) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(headers.len());
    for (k, v) in headers {
        let key = normalize_header_name(&k);
        if key.is_empty() {
            continue;
        }
        out.insert(key, v.trim().to_string());
    }
    out
}

fn run_super_parse(
    cfg: &TvConfig,
    parser: &Parser,
    play_url: &str,
    base_headers: &HashMap<String, String>,
    _site: Option<&Site>,
    progress: &ProgressSink,
) -> Result<PlayResult> {
    super_parse::run(cfg, parser, play_url, base_headers, progress)
}

/// 超级解析（type=4）：并发跑所有 type1 接口，取第一个有效直链。
///
/// 与 TVBox `SuperParse.parse` + `JsonParallel.parse` 的对应关系：
/// - 只挑 `kind==1` 且 url 非空的解析器（对应 `jsonJx`）；
/// - 并发上限 5、首个非 sniff 结果赢、命中后取消其余任务（对应 `completionService.take()`
///   + `cancelAll` + `shutdownNow`）；
/// - 保留 `jxFrom`（命中的接口名）；
/// - 总超时 20s；
/// - 没有 type1 时，若有 type0 则返回其播放页 `sniff=true`（对应 `SuperParse` 的 webJx 腿）。
///
/// 差异：TVBox 的 `SuperParse.parse()` 只有在存在 type0 时才返回聚合页、否则 json 腿根本
/// 不会启动（报告 §6.3 的已知限制）；桌面端直接发 json 竞速，type0 降级只作为兜底。
pub mod super_parse {
    use super::*;

    /// 并发池的消息
    enum Msg {
        /// 有效直链（非 sniff）
        Direct(PlayResult),
        /// 接口返回的仍是播放页（parse=1），桌面端需要再抓包
        Sniff(PlayResult),
        /// 单个接口失败
        Failed(String),
    }

    pub fn run(
        cfg: &TvConfig,
        parser: &Parser,
        play_url: &str,
        base_headers: &HashMap<String, String>,
        progress: &ProgressSink,
    ) -> Result<PlayResult> {
        let json_parsers: Vec<Parser> = cfg
            .parses
            .iter()
            .filter(|p| p.kind == 1 && !p.url.trim().is_empty())
            .cloned()
            .collect();

        // 没有 type1：回退 type0 播放页嗅探（SuperParse 的 webJx 腿）
        if json_parsers.is_empty() {
            progress.emit("super", "info", "没有 type=1 解析接口，回退 type=0 嗅探");
            return fallback_type0(
                cfg,
                parser,
                play_url,
                base_headers,
                "没有 type=1 解析接口",
                progress,
            );
        }

        let total = json_parsers.len();
        let worker_count = SUPER_PARSE_CONCURRENCY.min(total).max(1);
        let (tx, rx) = mpsc::channel::<Msg>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cfg_shared = Arc::new(cfg.clone());
        let headers_shared = Arc::new(base_headers.clone());
        let play_shared = Arc::new(play_url.to_string());
        progress.emit_detail(
            "super",
            "start",
            format!("超级解析：{total} 个接口并发竞速"),
            Some(format!(
                "并发上限 {worker_count}，总超时 {}s",
                PARSE_TIMEOUT.as_secs()
            )),
        );

        // detached 调度线程：内部用 scope 管住 worker；主线程拿到首个结果即可返回，
        // 不必等待慢接口（scope 在后台线程 join，不会在主路径 panic）。
        let spawn = std::thread::Builder::new()
            .name("vbox-super-parse".to_string())
            .spawn({
                let cancel = Arc::clone(&cancel);
                let cfg_shared = Arc::clone(&cfg_shared);
                let headers_shared = Arc::clone(&headers_shared);
                let play_shared = Arc::clone(&play_shared);
                let progress_shared = progress.clone();
                move || {
                    let next = AtomicUsize::new(0);
                    std::thread::scope(|scope| {
                        for _ in 0..worker_count {
                            let tx = tx.clone();
                            let cancel = Arc::clone(&cancel);
                            let cfg_shared = Arc::clone(&cfg_shared);
                            let headers_shared = Arc::clone(&headers_shared);
                            let play_shared = Arc::clone(&play_shared);
                            let progress_worker = progress_shared.clone();
                            let parsers = &json_parsers;
                            let next = &next;
                            scope.spawn(move || loop {
                                if cancel.load(Ordering::SeqCst) {
                                    break;
                                }
                                let idx = next.fetch_add(1, Ordering::SeqCst);
                                let Some(p) = parsers.get(idx) else { break };
                                match request_json_parser(
                                    cfg_shared.as_ref(),
                                    p,
                                    play_shared.as_str(),
                                    headers_shared.as_ref(),
                                    "https:",
                                    &progress_worker,
                                ) {
                                    Ok(mut result) => {
                                        result.jx_from = Some(p.name.clone());
                                        if result.sniff {
                                            // 播放页结果只作为兜底，不打断竞速
                                            let _ = tx.send(Msg::Sniff(result));
                                        } else if super_parse_accept(play_shared.as_str(), &result) {
                                            // 首个直链：先抢 cancel 再发结果，避免重复命中
                                            if !cancel.swap(true, Ordering::SeqCst) {
                                                let _ = tx.send(Msg::Direct(result));
                                            }
                                            break;
                                        } else {
                                            let _ = tx.send(Msg::Failed(format!(
                                                "接口 {} 返回的地址与播放页相同或命中广告/非视频规则",
                                                p.name
                                            )));
                                        }
                                    }
                                    Err(e) => {
                                        let _ = tx.send(Msg::Failed(e.to_string()));
                                    }
                                }
                            });
                        }
                    });
                }
            });
        if let Err(e) = spawn {
            return Err(CoreError::Config(format!("超级解析启动并发线程失败: {e}")));
        }

        let deadline = Instant::now() + PARSE_TIMEOUT;
        let mut finished = 0usize;
        let mut sniff_fallback: Option<PlayResult> = None;
        let mut failures: Vec<String> = Vec::new();
        let mut direct: Option<PlayResult> = None;

        while finished < total {
            // 取消 / 超时都要能及时返回：把 20s 的等待切成 250ms 的片，
            // 每片检查一次取消令牌（recv_timeout 本身不可中断）。
            if progress.cancelled() {
                progress.emit("error", "fail", "已取消超级解析");
                cancel.store(true, Ordering::SeqCst);
                return Err(CoreError::Config("解析已取消".into()));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                failures.push(format!("等待 {} 个接口超时", total - finished));
                progress.emit_detail(
                    "super",
                    "fail",
                    "超级解析等待超时",
                    Some(format!("{} 个接口仍未返回", total - finished)),
                );
                break;
            }
            match rx.recv_timeout(remaining.min(Duration::from_millis(250))) {
                Ok(Msg::Direct(result)) => {
                    let from = result.jx_from.clone().unwrap_or_default();
                    progress.emit_detail(
                        "super",
                        "ok",
                        format!("超级解析命中接口 {from}"),
                        Some(shorten(&result.url, 180)),
                    );
                    direct = Some(result);
                    break;
                }
                Ok(Msg::Sniff(result)) => {
                    if sniff_fallback.is_none() {
                        sniff_fallback = Some(result);
                    }
                    finished += 1;
                }
                Ok(Msg::Failed(msg)) => {
                    failures.push(msg);
                    finished += 1;
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // 通知后台 worker 停止领取新任务（在途请求会自然结束，结果被丢弃）
        cancel.store(true, Ordering::SeqCst);

        if let Some(mut result) = direct {
            result.parse_name = Some(parser.name.clone());
            result.trace.push(format!(
                "超级解析：{} 个 type=1 接口并发竞速，命中 {}",
                total,
                result.jx_from.clone().unwrap_or_default()
            ));
            return Ok(result);
        }

        if let Some(mut result) = sniff_fallback {
            progress.emit("super", "info", "接口只返回播放页（parse=1），转 WebView 抓包");
            result.parse_name = Some(parser.name.clone());
            result.trace.push(format!(
                "超级解析：{} 个 type=1 接口只返回播放页（parse=1），交由桌面端抓包",
                total
            ));
            return Ok(result);
        }

        // 所有 type1 都失败：有 type0 就退回嗅探页（桌面端至少还能抓包）
        if cfg.parses.iter().any(|p| p.kind == 0) {
            progress.emit("super", "info", "type=1 接口全部失败，回退 type=0 嗅探");
            return fallback_type0(
                cfg,
                parser,
                play_url,
                base_headers,
                "type=1 接口全部失败",
                progress,
            );
        }

        let summary = failures
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("；");
        progress.emit_detail(
            "error",
            "fail",
            format!("超级解析：{total} 个接口均未返回有效直链"),
            (!summary.is_empty()).then(|| summary.clone()),
        );
        Err(CoreError::Config(if summary.is_empty() {
            format!("超级解析：{total} 个 type=1 接口均未返回有效直链")
        } else {
            format!("超级解析：{total} 个 type=1 接口均未返回有效直链（{summary}）")
        }))
    }

    fn fallback_type0(
        cfg: &TvConfig,
        parser: &Parser,
        play_url: &str,
        base_headers: &HashMap<String, String>,
        reason: &str,
        progress: &ProgressSink,
    ) -> Result<PlayResult> {
        let p0 = cfg.parses.iter().find(|p| p.kind == 0).ok_or_else(|| {
            CoreError::Config(format!(
                "超级解析无法执行：{reason}，且配置中没有 type=0 嗅探解析器"
            ))
        })?;
        let mut result = run_sniff_parser(p0, play_url, base_headers)?;
        progress.emit_detail(
            "super",
            "ok",
            format!("回退 type=0 嗅探 {}", p0.name),
            Some(format!("{reason}；播放页将交给抓包")),
        );
        result.parse_name = Some(parser.name.clone());
        result.jx_from = Some(p0.name.clone());
        result
            .trace
            .push(format!("超级解析：{reason}，回退 type=0 嗅探 {}", p0.name));
        Ok(result)
    }
}

/// 超解析结果过滤（对齐 `Utils.jsonParse`）：广告域、以及"返回 URL 等于播放页"时
/// 按 VIP 域 / 视频格式再筛一次，避免把 iframe 页面当成直链。
fn super_parse_accept(input: &str, result: &PlayResult) -> bool {
    if result.url.contains("973973.xyz") || result.url.contains(".fit:") {
        return false;
    }
    if result.url == input && (is_vip_host(&result.url) || !looks_like_video(&result.url)) {
        return false;
    }
    true
}

fn is_vip_host(url: &str) -> bool {
    [
        "iqiyi.com",
        "v.qq.com",
        "youku.com",
        "le.com",
        "tudou.com",
        "mgtv.com",
        "sohu.com",
        "acfun.cn",
        "bilibili.com",
        "baofeng.com",
        "pptv.com",
    ]
    .iter()
    .any(|host| url.contains(host))
}

fn looks_like_video(url: &str) -> bool {
    if url.contains("url=http")
        || url.contains(".js")
        || url.contains(".css")
        || url.contains(".html")
    {
        return false;
    }
    is_direct_media_url(url) || url.contains("video/tos")
}

pub fn is_direct_media_url(u: &str) -> bool {
    let lower = u.split(['?', '#']).next().unwrap_or(u).to_lowercase();
    [".m3u8", ".mp4", ".flv", ".mkv", ".mpd", ".ts", ".m3u"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_url_inserts_cat_ext_after_question_mark() {
        let mixed = mix_url(
            "https://parse.example.com/api?url=",
            r#"{"header":{"a":"b"}}"#,
        );
        assert!(mixed.starts_with("https://parse.example.com/api?cat_ext="));
        assert!(mixed.contains("&url="));
        // 没有 ? 时不注入（TVBox 同样不自动补参数名）
        assert_eq!(
            mix_url("https://parse.example.com/api", "{}"),
            "https://parse.example.com/api"
        );
        // 已有 cat_ext 不重复注入
        let already = "https://p.example.com/a?cat_ext=x&url=";
        assert_eq!(mix_url(already, "{}"), already);
    }

    #[test]
    fn direct_media_detection() {
        assert!(is_direct_media_url(
            "https://cdn.example.com/a.M3U8?token=1"
        ));
        assert!(is_direct_media_url("https://cdn.example.com/a.mp4#t=1"));
        assert!(!is_direct_media_url("https://vip.example.com/play/1"));
        assert!(!is_direct_media_url("https://cdn.example.com/index.html"));
    }
}
