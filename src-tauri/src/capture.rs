//! 媒体地址抓包（WebView 嗅探）
//!
//! 目标：加载解析页/播放页，捕获其中的媒体请求（m3u8/mp4 等），返回第一个候选地址。
//!
//! 桌面端实现分两层：
//! 1. 隐藏 Tauri WebView 加载页面，执行点击脚本（click/rules.script）
//! 2. 读取初始化脚本记录的 fetch/XHR/`performance.getEntriesByType('resource')`
//!    以及 `<video>/<source>` 的 src
//!
//! 说明：这是"轻量抓包"，能覆盖 fetch/XHR/媒体标签产生的请求；对 blob:、MSE、
//! 加密分片或严格 CSP 页面可能拿不到。完整 Network 级抓包需 WebView2 CDP（后续增强）。
//!
//! # 线程模型（重要）
//!
//! Tauri 2 官方文档明确：在**同步命令**（主线程）里调用 `WebviewWindowBuilder::build`
//! 在 Windows 上会死锁。因此本模块提供：
//! - [`sniff_async`]：真正的实现，由 `commands::sniff`（已改为 async）直接调用；
//! - [`sniff`]：同步包装，仅供后台线程/测试使用，主线程调用返回可执行的错误提示。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{WebviewUrl, WebviewWindowBuilder};
use vbox_core::model::PlayResult;
use vbox_core::progress::{shorten, ProgressSink};

/// 抓包时在页面里安装的初始化脚本。
///
/// 约束：只在主 frame 注入（跨域 iframe 内的请求读不到，见模块说明）；
/// 记录所有 http(s) 资源，Rust 侧再按媒体扩展名过滤。
const SNIFF_SCRIPT: &str = r#"
(function () {
  if (window.__vbox_installed__) return;
  window.__vbox_installed__ = true;
  var M = window.__vbox_media__ = [];
  var S = window.__vbox_seen__ = Object.create(null);
  function push(u, t) {
    try {
      if (!u) return;
      u = String(u);
      if (u.indexOf('blob:') === 0 || u.indexOf('data:') === 0 || u.indexOf('javascript:') === 0) return;
      if (u.indexOf('//') === 0) u = location.protocol + u;
      try { u = new URL(u, location.href).href; } catch (e) { return; }
      if (S[u]) return;
      S[u] = 1;
      if (M.length < 300) M.push({ t: t || '', u: u });
    } catch (e) {}
  }
  window.__vbox_push__ = push;
  try {
    var of = window.fetch;
    if (of) {
      window.fetch = function (input, init) {
        try { push(typeof input === 'string' ? input : (input && input.url), 'fetch'); } catch (e) {}
        return of.apply(this, arguments);
      };
    }
  } catch (e) {}
  try {
    var xo = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (method, url) {
      try { push(url, 'xhr'); } catch (e) {}
      return xo.apply(this, arguments);
    };
  } catch (e) {}
  function fromDom() {
    try {
      document.querySelectorAll('video,source,audio').forEach(function (n) {
        push(n.currentSrc || n.src, 'dom');
      });
    } catch (e) {}
  }
  function fromPerf() {
    try {
      performance.getEntriesByType('resource').forEach(function (e) { push(e.name, 'res'); });
    } catch (e) {}
  }
  function collect() { fromDom(); fromPerf(); }
  window.__vbox_collect__ = collect;
  try { setInterval(collect, 500); } catch (e) {}
  try {
    if (document.documentElement) {
      new MutationObserver(collect).observe(document.documentElement, { childList: true, subtree: true });
    }
  } catch (e) {}
  collect();
})();
"#;

/// 轮询表达式：先触发一次收集，再把候选数组序列化返回。
const COLLECT_JS: &str = "(function(){try{window.__vbox_collect__&&window.__vbox_collect__()}catch(e){}return JSON.stringify(window.__vbox_media__||[]);})()";

/// 页面记录的一条资源。
#[derive(Debug, Clone, serde::Deserialize)]
struct CollectedMedia {
    #[serde(default)]
    t: String,
    u: String,
}

/* ---------------- WebView2 网络层抓包 ---------------- */

/// 网络层抓到的 URL 集合（跨 iframe / Web Worker 也能看到）。
///
/// 背景：Android 的 `WebViewClient.shouldInterceptRequest` 在网络栈上拦截所有子资源，
/// 而 WebView2 的初始化脚本只在主 frame 生效 —— 第三方解析页/播放页常把播放器放在
/// iframe 或 Web Worker 里，只靠 JS hook 会漏掉真正的 m3u8。这里用
/// `ICoreWebView2::add_WebResourceRequested` 做同等能力的拦截。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
struct NetCapture {
    seen: HashSet<String>,
    urls: Vec<String>,
    /// 每个 URL 的播放相关请求头（UA / Referer / Origin / Cookie）
    headers: HashMap<String, HashMap<String, String>>,
}

/// 最多记录 500 个网络 URL，避免长页面把内存撑爆
#[cfg_attr(not(windows), allow(dead_code))]
const NET_CAPTURE_MAX: usize = 500;

#[cfg_attr(not(windows), allow(dead_code))]
impl NetCapture {
    fn record(&mut self, url: String, headers: HashMap<String, String>) {
        if !headers.is_empty() {
            let slot = self.headers.entry(url.clone()).or_default();
            for (key, value) in headers {
                slot.entry(key).or_insert(value);
            }
        }
        if self.urls.len() >= NET_CAPTURE_MAX {
            return;
        }
        if self.seen.insert(url.clone()) {
            self.urls.push(url);
        }
    }
}

/// 注册 WebView2 网络请求监听（仅 Windows；其它平台是空实现）。
///
/// 注册失败不影响主流程：页面内 JS hook 仍然工作，只是覆盖范围退化为主 frame。
#[cfg(windows)]
fn install_net_capture(window: &tauri::WebviewWindow, shared: Arc<Mutex<NetCapture>>) {
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
    use webview2_com::{take_pwstr, WebResourceRequestedEventHandler};
    use windows_core::{HSTRING, PCWSTR, PWSTR};

    let result = window.with_webview(move |platform| {
        let controller = platform.controller();
        let core = match unsafe { controller.CoreWebView2() } {
            Ok(core) => core,
            Err(err) => {
                eprintln!("[vbox][sniff] 取 WebView2 CoreWebView2 失败: {err}");
                return;
            }
        };
        let filter = HSTRING::from("*");
        if let Err(err) = unsafe {
            core.AddWebResourceRequestedFilter(
                PCWSTR::from_raw(filter.as_ptr()),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            )
        } {
            eprintln!("[vbox][sniff] 注册 WebResourceRequested 过滤器失败: {err}");
            return;
        }
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
            if let Some(args) = args {
                if let Ok(request) = unsafe { args.Request() } {
                    let mut uri = PWSTR::null();
                    if unsafe { request.Uri(&mut uri) }.is_ok() {
                        let url = take_pwstr(uri);
                        if !url.is_empty() {
                            // 记录 UA/Referer/Cookie：播放 m3u8 分片时通常需要它们
                            let headers = unsafe { playback_headers(&request) };
                            if let Ok(mut guard) = shared.lock() {
                                guard.record(url, headers);
                            }
                        }
                    }
                }
            }
            Ok(())
        }));
        let mut token = 0i64;
        if let Err(err) = unsafe { core.add_WebResourceRequested(&handler, &mut token) } {
            eprintln!("[vbox][sniff] 注册 WebResourceRequested 事件失败: {err}");
        }
    });
    if let Err(err) = result {
        eprintln!("[vbox][sniff] with_webview 调度失败: {err}");
    }
}

/// 读取一条网络请求里与播放相关的头（UA / Referer / Origin / Cookie）。
///
/// 对齐 Android 的 shouldInterceptRequest：命中的媒体地址往往需要原来的 Referer /
/// Cookie 才能播（TVBox 也是把这些头透传给播放器）。
#[cfg(windows)]
unsafe fn playback_headers(
    request: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2WebResourceRequest,
) -> HashMap<String, String> {
    use webview2_com::take_pwstr;
    use windows_core::{BOOL, PWSTR};

    let mut out = HashMap::new();
    let Ok(headers) = (unsafe { request.Headers() }) else {
        return out;
    };
    let Ok(iterator) = (unsafe { headers.GetIterator() }) else {
        return out;
    };
    let mut has_current = BOOL(0);
    if unsafe { iterator.HasCurrentHeader(&mut has_current) }.is_err() || !has_current.as_bool() {
        return out;
    }
    loop {
        let mut name = PWSTR::null();
        let mut value = PWSTR::null();
        if unsafe { iterator.GetCurrentHeader(&mut name, &mut value) }.is_ok() {
            let name = take_pwstr(name);
            let value = take_pwstr(value);
            let lower = name.to_ascii_lowercase();
            if matches!(lower.as_str(), "user-agent" | "referer" | "origin" | "cookie")
                && !value.trim().is_empty()
            {
                out.insert(name, value);
            }
        }
        let mut has_next = BOOL(0);
        if unsafe { iterator.MoveNext(&mut has_next) }.is_err() || !has_next.as_bool() {
            break;
        }
    }
    out
}

#[cfg(not(windows))]
fn install_net_capture(_window: &tauri::WebviewWindow, _shared: Arc<Mutex<NetCapture>>) {}

/// 嗅探媒体地址（同步入口）。
///
/// 当前 `commands::sniff` 已是 async 并直接调用 [`sniff_async`]；本函数保留给
/// 后台线程/测试使用，主线程调用会返回明确错误（Windows 上同步阻塞建窗会死锁）。
#[allow(dead_code)]
pub fn sniff(
    app: &tauri::AppHandle,
    url: &str,
    headers: HashMap<String, String>,
    click: Option<String>,
    timeout_ms: u64,
) -> Result<PlayResult, String> {
    let on_main_thread = std::thread::current()
        .name()
        .map(|name| name == "main")
        .unwrap_or(false);
    if on_main_thread {
        return Err(
            "WebView 抓包需要异步命令：请把 commands::sniff 改为 `async fn` \
             并调用 capture::sniff_async(...)，否则在 Windows 主线程创建隐藏窗口会死锁"
                .into(),
        );
    }
    let sink = ProgressSink::none();
    tauri::async_runtime::block_on(sniff_async(app, url, headers, click, timeout_ms, &sink))
}

/// 嗅探媒体地址（异步实现）。
///
/// 必须在 Tauri async runtime 上执行（`#[tauri::command] async fn` 或
/// `tauri::async_runtime::spawn`），不能阻塞主线程。
///
/// 线程模型：建窗必须留在 async 上下文（Windows 上主线程同步建窗会死锁），
/// 而 300ms 轮询 + `std::thread::sleep` 是纯阻塞代码，必须丢给 `spawn_blocking`，
/// 否则会长期占住一个 tokio worker（APP 其它异步任务都会被拖慢）。
pub async fn sniff_async(
    app: &tauri::AppHandle,
    url: &str,
    headers: HashMap<String, String>,
    click: Option<String>,
    timeout_ms: u64,
    progress: &ProgressSink,
) -> Result<PlayResult, String> {
    let timeout_ms = timeout_ms.clamp(1_000, 120_000);
    let parsed: tauri::Url = url
        .parse()
        .map_err(|e| format!("抓包 URL 非法: {url}（{e}）"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        progress.emit_detail(
            "sniff",
            "fail",
            "抓包失败：只支持 http(s) 播放页",
            Some(format!("收到: {url}")),
        );
        return Err(format!("抓包只支持 http(s) 页面，收到: {url}"));
    }

    // 输入本身就是媒体直链时无需开窗口。
    if let Some(media) = media_priority(url) {
        if media <= 2 {
            progress.emit_detail(
                "sniff",
                "ok",
                "输入已是媒体地址，跳过 WebView 抓包",
                Some(shorten(url, 180)),
            );
            return Ok(PlayResult {
                url: url.to_string(),
                headers,
                subtitles: Vec::new(),
                sniff: false,
                parse_name: None,
                jx_from: None,
                trace: vec!["输入已是媒体地址，跳过 WebView 抓包".into()],
            });
        }
    }

    let click_script = click.as_deref().and_then(|raw| plan_click(url, raw));
    let mut trace: Vec<String> = Vec::new();
    if click.is_some() && click_script.is_none() {
        trace.push("click 条件与当前 URL 不匹配，已跳过点击".into());
    }
    if !headers.is_empty() {
        trace.push(format!(
            "WebView 仅透传 UA，其余请求头随 PlayResult 返回给播放器（{} 项）",
            headers.len()
        ));
    }

    progress.emit_detail(
        "sniff",
        "start",
        "打开隐藏 WebView 加载播放页",
        Some(shorten(url, 200)),
    );

    let label = unique_label();
    let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::External(parsed))
        .title("VBox Sniff")
        .visible(false)
        .inner_size(1280.0, 720.0)
        .initialization_script(SNIFF_SCRIPT);
    if let Some(ua) = header_value(&headers, "user-agent") {
        if !ua.trim().is_empty() {
            builder = builder.user_agent(ua);
        }
    }

    let window = match builder.build() {
        Ok(window) => window,
        Err(e) => {
            progress.emit_detail(
                "sniff",
                "fail",
                "创建隐藏 WebView 失败",
                Some(e.to_string()),
            );
            return Err(format!("创建隐藏 WebView 失败: {e}"));
        }
    };
    trace.push(format!("隐藏 WebView 已加载: {url}"));
    progress.emit_detail(
        "sniff",
        "info",
        "隐藏 WebView 已创建，等待页面加载资源",
        Some("网络层 + 页面内 fetch/XHR 双重监听".to_string()),
    );

    // 网络层抓包：注册到 WebView2，能覆盖 iframe / Web Worker 里的媒体请求
    let net = Arc::new(Mutex::new(NetCapture::default()));
    install_net_capture(&window, Arc::clone(&net));

    let poll_progress = progress.clone();
    let join = tauri::async_runtime::spawn_blocking(move || {
        poll_for_media(
            window,
            click_script,
            timeout_ms,
            trace,
            headers,
            poll_progress,
            net,
        )
    })
    .await;
    match join {
        Ok(result) => result,
        Err(e) => Err(format!("抓包任务执行失败: {e}")),
    }
}

/// 捕获页面运行时发出的、包含指定子串的请求 URL（异步实现）。
///
/// 场景：某些站点的关键参数（如 Panyq 的 `sign`）不是静态算法产物，而是页面
/// 初始化脚本在浏览器里现算后拼进请求的。此时只能「开一个隐藏 WebView 让页面
/// 自己跑一遍，再把它发出的请求 URL 捞出来」。
///
/// 与 [`sniff_async`] 同构：建窗 → 网络层抓包 → 轮询命中子串 → 关窗。
/// 与 `sniff_async` 的差别只有终点判定：这里匹配 URL 子串，而不是媒体扩展名。
///
/// 非 Windows 平台没有 WebView2 网络层抓包（[`install_net_capture`] 为空实现），
/// 直接返回 `None`，调用方应自行降级。
pub async fn capture_api_url_async(
    app: &tauri::AppHandle,
    page_url: &str,
    needle: &str,
    timeout_ms: u64,
) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let parsed: tauri::Url = page_url.parse().ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    let timeout_ms = timeout_ms.clamp(1_000, 60_000);
    let needle = needle.to_string();

    let label = unique_label();
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::External(parsed))
        .title("VBox Capture")
        .visible(false)
        .inner_size(1280.0, 720.0)
        .initialization_script(SNIFF_SCRIPT)
        .build()
        .ok()?;
    let net = Arc::new(Mutex::new(NetCapture::default()));
    install_net_capture(&window, Arc::clone(&net));

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let hit = tauri::async_runtime::spawn_blocking(move || {
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(150));
            let matched = {
                let guard = net.lock().ok()?;
                guard
                    .urls
                    .iter()
                    .find(|url| url.contains(&needle))
                    .cloned()
            };
            if let Some(url) = matched {
                let _ = window.close();
                return Some(url);
            }
        }
        let _ = window.close();
        None
    })
    .await
    .ok()
    .flatten();
    hit
}

/// 阻塞式轮询页面资源，直到命中媒体地址 / 超时 / 被取消（在 `spawn_blocking` 线程里跑）。
fn poll_for_media(
    window: tauri::WebviewWindow,
    click_script: Option<String>,
    timeout_ms: u64,
    mut trace: Vec<String>,
    headers: HashMap<String, String>,
    progress: ProgressSink,
    net: Arc<Mutex<NetCapture>>,
) -> Result<PlayResult, String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let started = Instant::now();
    let first_click_at = started + Duration::from_millis(1_200);
    let retry_click_at = started + Duration::from_millis(3_500);
    let mut click_stage: u8 = if click_script.is_some() { 0 } else { 2 };
    let mut collected: Vec<CollectedMedia> = Vec::new();
    let mut last_report = started;
    let mut last_reported_len = 0usize;

    while Instant::now() < deadline {
        if progress.cancelled() {
            trace.push("用户取消抓包".into());
            let _ = window.close();
            progress.emit("sniff", "fail", "已取消抓包");
            return Err("抓包已取消".into());
        }
        std::thread::sleep(Duration::from_millis(300));

        if click_stage == 0 && Instant::now() >= first_click_at {
            click_stage = 1;
            if let Some(js) = &click_script {
                match window.eval(js.clone()) {
                    Ok(()) => {
                        trace.push(format!("已执行 click 脚本: {js}"));
                        progress.emit_detail(
                            "sniff",
                            "info",
                            "已执行页面点击脚本",
                            Some(shorten(js, 120)),
                        );
                    }
                    Err(e) => trace.push(format!("click 脚本执行失败: {e}")),
                }
            }
        } else if click_stage == 1 && Instant::now() >= retry_click_at && collected.is_empty() {
            // 页面加载较慢时首次点击可能落空；未捕获到任何资源时再补一次。
            click_stage = 2;
            if let Some(js) = &click_script {
                match window.eval(js.clone()) {
                    Ok(()) => {
                        trace.push(format!("未捕获资源，重试 click 脚本: {js}"));
                        progress.emit_detail(
                            "sniff",
                            "info",
                            "未捕获资源，重试点击脚本",
                            Some(shorten(js, 120)),
                        );
                    }
                    Err(e) => trace.push(format!("click 重试失败: {e}")),
                }
            }
        }

        let batch = collect_from_window(&window);
        for media in batch {
            if !collected.iter().any(|old| old.u == media.u) {
                collected.push(media);
            }
        }

        // 网络层抓包结果：iframe / Worker 里的 m3u8 只有这里能看到
        if let Ok(guard) = net.lock() {
            for url in &guard.urls {
                if !collected.iter().any(|old| old.u == *url) {
                    collected.push(CollectedMedia {
                        t: "net".into(),
                        u: url.clone(),
                    });
                }
            }
        }

        if let Some(best) = pick_media(&collected) {
            append_candidates_trace(&mut trace, &collected);
            trace.push(format!("命中媒体地址: {best}"));
            crate::logs::push(
                "info",
                format!("抓包命中媒体地址: {best}（候选 {} 个）", collected.len()),
            );
            let _ = window.close();
            progress.emit_detail(
                "sniff",
                "ok",
                "捕获到媒体地址",
                Some(shorten(&best, 200)),
            );
            // 把播放器需要的请求头（Referer/Cookie/UA）从命中请求里带出去
            let mut headers = headers;
            if let Ok(guard) = net.lock() {
                if let Some(extra) = guard.headers.get(&best) {
                    for (key, value) in extra {
                        headers.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                    if !extra.is_empty() {
                        trace.push(format!("命中请求头: {}", extra.keys().cloned().collect::<Vec<_>>().join(",")));
                    }
                }
            }
            return Ok(PlayResult {
                url: best,
                headers,
                subtitles: Vec::new(),
                sniff: false,
                parse_name: None,
                jx_from: None,
                trace,
            });
        }

        // 每 2.5s 或候选数变化时汇报一次，让 UI 知道「还在扫、已扫到几个」
        if last_report.elapsed() >= Duration::from_millis(2_500)
            || (collected.len() != last_reported_len && !collected.is_empty())
        {
            last_report = Instant::now();
            last_reported_len = collected.len();
            let latest = collected.last().map(|media| media.u.as_str()).unwrap_or("");
            progress.emit_detail(
                "sniff",
                "info",
                format!(
                    "正在扫描页面资源（已发现 {} 个候选，{}s）",
                    collected.len(),
                    started.elapsed().as_secs()
                ),
                (!latest.is_empty()).then(|| shorten(latest, 160)),
            );
        }
    }

    let _ = window.close();
    append_candidates_trace(&mut trace, &collected);
    crate::logs::push(
        "warn",
        format!("抓包超时（{timeout_ms} ms），未命中媒体地址，候选 {} 个", collected.len()),
    );
    // 排障：超时时把候选打到终端（UI 里 detail 会被截断，这里保留完整列表）
    eprintln!(
        "[vbox][sniff] 抓包超时，候选 {} 个:\n{}",
        collected.len(),
        collected
            .iter()
            .take(40)
            .map(|media| format!("  [{}] {}", if media.t.is_empty() { "res" } else { media.t.as_str() }, media.u))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let hint = if collected.iter().any(|m| m.u.starts_with("blob:")) {
        "；只发现 blob: 地址，页面可能使用 MSE 播放，需要 CDP 级抓包"
    } else {
        ""
    };
    let message = format!(
        "抓包超时（{timeout_ms} ms）：页面未产生可识别的媒体请求{suffix}（候选 {} 个）",
        collected.len(),
        suffix = hint
    );
    progress.emit_detail(
        "sniff",
        "fail",
        "抓包超时：页面没有产生可识别的媒体请求",
        Some(format!("候选 {} 个{}", collected.len(), hint)),
    );
    Err(message)
}

/// 触发页面内收集脚本并读取候选；eval 回调在 WebView 主线程执行，最多等 2s。
fn collect_from_window(window: &tauri::WebviewWindow) -> Vec<CollectedMedia> {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if window
        .eval_with_callback(COLLECT_JS, move |result| {
            let _ = tx.send(result);
        })
        .is_err()
    {
        return Vec::new();
    }
    match rx.recv_timeout(Duration::from_millis(2_000)) {
        Ok(raw) => parse_collected(&raw),
        Err(_) => Vec::new(),
    }
}

/// `eval_with_callback` 返回的是 JSON 序列化结果；字符串结果可能被再包一层引号。
fn parse_collected(raw: &str) -> Vec<CollectedMedia> {
    fn parse_value(value: serde_json::Value) -> Vec<CollectedMedia> {
        match value {
            serde_json::Value::String(inner) => serde_json::from_str::<serde_json::Value>(&inner)
                .ok()
                .map(parse_value)
                .unwrap_or_default(),
            serde_json::Value::Array(_) => {
                serde_json::from_value::<Vec<CollectedMedia>>(value).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .map(parse_value)
        .unwrap_or_default()
}

/// 在候选里挑一个媒体 URL：m3u8 > mpd > mp4 > flv/mkv/... > ts。
fn pick_media(collected: &[CollectedMedia]) -> Option<String> {
    let mut ranked: Vec<(u8, usize, &str)> = collected
        .iter()
        .enumerate()
        .filter_map(|(idx, media)| {
            media_priority(&media.u).map(|prio| (prio, idx, media.u.as_str()))
        })
        .collect();
    ranked.sort_by_key(|(prio, idx, _)| (*prio, *idx));
    ranked.first().map(|(_, _, url)| (*url).to_string())
}

/// 返回媒体优先级；`None` 表示不是可播放的媒体直链。
fn media_priority(url: &str) -> Option<u8> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("blob:")
        || lower.starts_with("data:")
        || lower.starts_with("javascript:")
        || lower.starts_with("about:")
    {
        return None;
    }
    let before_query = lower.split(['?', '#']).next().unwrap_or(lower.as_str());
    // 对齐 TVBox `checkVideoFormat`：`url=http...` / `.html` 是播放页/代理页，不是媒体
    if lower.contains("url=http") || before_query.ends_with(".html") || before_query.ends_with(".htm") {
        return None;
    }
    let ext = before_query.rsplit('.').next().unwrap_or("");
    let priority = match ext {
        "m3u8" | "m3u" => 0,
        "mpd" => 1,
        "mp4" | "m4v" => 2,
        "flv" | "mkv" | "mov" | "webm" | "avi" | "wmv" | "rm" | "rmvb" | "mpg" | "mpeg" => 3,
        // TVBox 的 snifferMatch 也把音频当媒体（部分源只给音轨直链）
        "ts" | "m4s" | "aac" | "mp3" | "m4a" => 4,
        // 无扩展名但路径里带 m3u8 的也收（部分源用 /m3u8/xxx 形式）。
        _ => {
            if lower.contains("m3u8") {
                0
            } else {
                return None;
            }
        }
    };
    if is_asset_url(before_query) {
        return None;
    }
    Some(priority)
}

/// html/js/css/图片等非媒体资源（即使查询串里带 m3u8 也不当作播放地址）。
fn is_asset_url(before_query: &str) -> bool {
    let ext = before_query.rsplit('.').next().unwrap_or("");
    matches!(
        ext,
        "html"
            | "htm"
            | "js"
            | "css"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "svg"
            | "ico"
            | "json"
            | "woff"
            | "woff2"
            | "ttf"
            | "map"
            | "txt"
            | "xml"
    )
}

/// 把 click 配置解析成要执行的 JS：
/// - `包含子串;JS`：仅当页面 URL 含该子串时执行 JS；
/// - 其他形式：整段当 JS 执行。
fn plan_click(page_url: &str, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some((cond, js)) = raw.split_once(';') {
        let cond = cond.trim();
        let js = js.trim();
        // 形如 `ddrk.me;#id`：左侧是 URL 子串条件而不是 JS 代码。
        if !cond.is_empty()
            && !js.is_empty()
            && !cond.contains(['(', ')', '{', '}', '='])
            && !cond.starts_with("document")
            && !cond.starts_with("window")
        {
            return if page_url.contains(cond) {
                Some(js.to_string())
            } else {
                None
            };
        }
    }
    Some(raw.to_string())
}

fn append_candidates_trace(trace: &mut Vec<String>, collected: &[CollectedMedia]) {
    for media in collected.iter().take(10) {
        let kind = if media.t.is_empty() {
            "res"
        } else {
            media.t.as_str()
        };
        trace.push(format!("候选[{kind}] {}", media.u));
    }
    if collected.len() > 10 {
        trace.push(format!("… 其余 {} 个候选省略", collected.len() - 10));
    }
}

fn header_value<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn unique_label() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!(
        "vbox-sniff-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_priority_filters_assets_and_ranks_m3u8() {
        assert_eq!(media_priority("https://a/x.m3u8?token=1"), Some(0));
        assert_eq!(media_priority("https://a/x.mp4"), Some(2));
        assert_eq!(media_priority("https://a/live/stream"), None);
        assert_eq!(media_priority("https://a/x.js?type=m3u8"), None);
        assert_eq!(media_priority("https://a/cover.jpg"), None);
        assert_eq!(media_priority("blob:https://a/1"), None);
        assert_eq!(media_priority("https://a/playlist/m3u8/index"), Some(0));
    }

    #[test]
    fn pick_prefers_m3u8_over_mp4() {
        let collected = vec![
            CollectedMedia {
                t: "res".into(),
                u: "https://a/v.mp4".into(),
            },
            CollectedMedia {
                t: "fetch".into(),
                u: "https://a/v.m3u8".into(),
            },
            CollectedMedia {
                t: "res".into(),
                u: "https://a/pic.png".into(),
            },
        ];
        assert_eq!(pick_media(&collected).as_deref(), Some("https://a/v.m3u8"));
    }

    #[test]
    fn parse_collected_handles_plain_and_quoted_json() {
        let plain = r#"[{"t":"fetch","u":"https://a/x.m3u8"}]"#;
        assert_eq!(parse_collected(plain).len(), 1);

        let quoted = serde_json::to_string(plain).unwrap();
        let parsed = parse_collected(&quoted);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].u, "https://a/x.m3u8");
    }

    #[test]
    fn plan_click_condition_and_plain_js() {
        assert_eq!(
            plan_click("https://ddrk.me/play/1", "ddrk.me;#id .click()").as_deref(),
            Some("#id .click()")
        );
        assert!(plan_click("https://other.site/1", "ddrk.me;#id .click()").is_none());
        assert_eq!(
            plan_click("https://a/1", "document.querySelector('video').play()").as_deref(),
            Some("document.querySelector('video').play()")
        );
        assert!(plan_click("https://a/1", "").is_none());
    }
}
