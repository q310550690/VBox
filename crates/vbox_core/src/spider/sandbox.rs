//! JS 沙箱封装（基于 rquickjs 0.9 的受控执行环境）
//!
//! 目标：在隔离的 QuickJS Runtime 中执行不可信的 type=3 `.js` spider 脚本，
//! 并把「脚本入口函数 → JSON 值」这一层封装好，供 `spider/js.rs` 使用。
//!
//! 安全边界：
//! - 不提供 `fs` / 原生 socket 绑定；脚本访问网络只能通过宿主注入的 `req`/`fetch`（仅 http(s)）
//! - 每次调用新建 Runtime/Context，执行完即销毁，不跨线程共享；
//!   脚本模块级变量不跨调用保留，跨调用数据用注入的 `local.get/set`
//! - 线程 + 通道做超时（默认 20s）；同时用 QuickJS 中断处理器打断死循环
//! - 单个响应体上限 10MB；QuickJS 堆上限 128MB
//!
//! 注入的宿主 API（对齐 jun 版 catvod `Global` / `Connect` / `local` 的子集）：
//! - `req(url, options)` → `{code, headers, content}`；`fetch` / `http` 为同义别名
//! - `log(...)`、`console.log/info/warn/error`
//! - `joinUrl(parent, child)` / `join`
//! - `md5(text)`、`base64Encode` / `base64Decode`
//! - `local.get/set/delete`（进程内内存表，不落盘）
//! - `setTimeout(fn, ms)`（同步立即执行，最多 3 层；`clearTimeout` 为空实现）
//!
//! async/Promise：驱动 QuickJS job 队列，支持 `await req(...)` 这类同步宿主可完成的 Promise；
//! `pdfh/pdfa/pd/pdfla` 由 `html_rule` 提供。
//! ES module：脚本来源为 http(s)/file 时按其 URL 解析相对 `import` / CommonJS `require`，
//! 依赖模块会被拉取并缓存；`cat.js`/`cheerio`/`utils` 等内置 lib 优先使用随包资产。
//! 明确不支持：`//bb`/`//DRPY` QuickJS 字节码、`jsapi` dex、
//! 需要真实事件循环的异步 I/O（定时器回调不会在 await 后继续推进）。

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::error::CoreError;
use crate::model::Rules;
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyInit, KeyIvInit};
use aes::Aes128;
use base64::Engine as _;
use encoding_rs::{Encoding, GBK, UTF_8};
use rquickjs::function::Args;
use rquickjs::prelude::{IntoJs, Opt, Rest};
use rquickjs::{CatchResultExt, CaughtError, Context, Ctx, Function, Object, Runtime, Value};
use serde_json::Value as Json;

/// 单次脚本调用默认超时（jun 的 home 30s / 播放 15s，这里取统一 20s）
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
/// 单个 HTTP 响应体上限：10MB
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;
/// QuickJS 堆上限：128MB
const MEMORY_LIMIT: usize = 128 * 1024 * 1024;
/// QuickJS 栈上限：4MB
const STACK_SIZE: usize = 4 * 1024 * 1024;
/// `setTimeout` 立即执行的最大嵌套层数（防脚本自递归爆栈）
const MAX_TIMER_DEPTH: u32 = 3;
/// ES module 加载：单次调用最多依赖的模块数
const MAX_MODULES: usize = 64;
/// ES module 加载：单次调用依赖模块总体积上限（8MB）
const MAX_MODULE_BYTES: usize = 8 * 1024 * 1024;
/// 模块文本缓存 TTL（跨调用复用，避免相同 lib 反复拉取）
const MODULE_CACHE_TTL: Duration = Duration::from_secs(600);
/// 模块文本缓存条目上限
const MODULE_CACHE_LIMIT: usize = 256;

/// 入口脚本执行后，把 `module.exports` 映射回旧入口约定（`__JS_SPIDER__`）
const ENTRY_EXPORTS_GLUE: &str = r#"(function(){
  try {
    var e = globalThis.module && globalThis.module.exports;
    if (!e) return;
    var has = false;
    for (var k in e) { has = true; break; }
    if (!has) return;
    if (!globalThis.__JS_SPIDER__ && typeof globalThis.__jsEvalReturn !== "function") {
      globalThis.__JS_SPIDER__ = (e.default !== undefined) ? e.default : e;
    }
  } catch (_) {}
})();"#;

/// 把 `__pdfaRaw`/`__pdflaRaw` 的 JSON 文本包装成 JS 数组。
/// 这样 Rust 侧不需要在闭包里构造带生命周期的 JSArray。
const PDF_ARRAY_PRELUDE: &str = r#"
globalThis.pdfa = function(html, rule) {
  try { return JSON.parse(globalThis.__pdfaRaw(String(html), String(rule)) || '[]'); }
  catch (e) { return []; }
};
globalThis.pdfla = function(html, p1, list_text, list_url, add_url) {
  try { return JSON.parse(globalThis.__pdflaRaw(String(html), String(p1), String(list_text), String(list_url), String(add_url || '')) || '[]'); }
  catch (e) { return []; }
};
"#;

/* ============================================================
 * 错误 / 参数
 * ============================================================ */

/// 沙箱错误：区分「脚本没实现该函数」与「脚本执行失败」，供上层做不同降级
#[derive(Debug)]
pub enum SandboxError {
    /// 脚本没有实现所需入口函数（不是异常）
    Missing(String),
    /// 脚本编译/执行/超时/JS 异常
    Script(String),
    /// 宿主侧（HTTP 客户端、线程创建等）错误
    Host(String),
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxError::Missing(func) => write!(f, "脚本未实现 {func}() 函数"),
            SandboxError::Script(msg) => write!(f, "{msg}"),
            SandboxError::Host(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for SandboxError {}

impl From<SandboxError> for CoreError {
    fn from(e: SandboxError) -> Self {
        CoreError::Config(e.to_string())
    }
}

/// 结果别名
pub type SandboxResult<T> = std::result::Result<T, SandboxError>;

/// 传给脚本入口函数的一个实参（只支持 PV 需要的几种简单类型）
#[derive(Debug, Clone)]
pub enum JsArg {
    Null,
    Bool(bool),
    Str(String),
    Num(f64),
    /// 复杂参数（对象 / 数组），用 JSON 文本转成 JS 值
    Json(Json),
}

/* ============================================================
 * 进程内 local 存储（对齐 jun 的 Hawk `jsRuntime_` 前缀）
 * ============================================================ */

static LOCAL_STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn local_store() -> &'static Mutex<HashMap<String, String>> {
    LOCAL_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn local_key(namespace: &str, key: &str) -> String {
    format!("jsRuntime_{namespace}_{key}")
}

thread_local! {
    /// `setTimeout` 立即执行时的嵌套深度
    static TIMER_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/* ============================================================
 * 对外的沙箱句柄
 * ============================================================ */

/// 一次脚本调用的完整配置。`call` 会新建线程 + Runtime，函数返回后全部销毁。
pub struct Sandbox {
    code: String,
    name: String,
    rules: Rules,
    /// 追加到每个 `req` 上的默认请求头（站点 ext 的 referer/cookie 等）
    headers: HashMap<String, String>,
    /// 脚本的 `init(...)` 入参；`None` 表示不调用 init
    init: Option<Json>,
    timeout: Duration,
    max_body: usize,
    /// 脚本来源 URL：用于解析 ES module 的相对 import / require
    base_url: Option<String>,
    /// 站点 key：`js2Proxy` 生成的代理 URL 会带上 `siteKey`，本地服务据此路由。
    site_key: Option<String>,
}

impl Sandbox {
    /// 创建沙箱；`name` 仅用于错误信息与 local 命名空间
    pub fn new(code: impl Into<String>, name: impl Into<String>, rules: Rules) -> Self {
        Self {
            code: code.into(),
            name: name.into(),
            rules,
            headers: HashMap::new(),
            init: None,
            timeout: DEFAULT_TIMEOUT,
            max_body: MAX_BODY_BYTES,
            base_url: None,
            site_key: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    /// 设置 `init(...)` 的入参；脚本没有 init 时静默跳过
    pub fn with_init(mut self, init: Json) -> Self {
        self.init = Some(init);
        self
    }

    pub fn with_max_body(mut self, max_body: usize) -> Self {
        self.max_body = max_body;
        self
    }

    /// 设置脚本来源 URL（解析 ES module 的相对 `import` / `require` 时作为基准）
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// 设置站点 key：`js2Proxy` 生成的代理 URL 会带上 `siteKey`，本地服务据此路由。
    pub fn with_site_key(mut self, site_key: impl Into<String>) -> Self {
        self.site_key = Some(site_key.into());
        self
    }

    /// 调用脚本入口函数，返回值归一为 `serde_json::Value`。
    ///
    /// 归一规则（对齐 TVBox「字符串优先当 JSON 解析」）：
    /// - JS 字符串 → 先尝试 JSON 解析，失败则原样返回字符串
    /// - JS 对象/数组 → `JSON.stringify` 后解析
    /// - 数字/布尔/null → 对应 JSON；undefined / 函数 → null
    pub fn call(&self, func: &str, args: &[JsArg]) -> SandboxResult<Json> {
        let job = Job {
            code: self.code.clone(),
            name: self.name.clone(),
            rules: self.rules.clone(),
            headers: self.headers.clone(),
            init: self.init.clone(),
            func: func.to_string(),
            args: args.to_vec(),
            max_body: self.max_body,
            timeout: self.timeout,
            base_url: self.base_url.clone(),
            site_key: self.site_key.clone(),
            stop: Arc::new(AtomicBool::new(false)),
        };

        // 看门狗：超时前 200ms 触发 QuickJS 中断，避免 `while(true)` 一直占 CPU
        let (done_tx, done_rx) = mpsc::channel::<()>();
        {
            let stop = job.stop.clone();
            let wait = self.timeout.saturating_sub(Duration::from_millis(200));
            std::thread::Builder::new()
                .name("vbox-js-watchdog".into())
                .spawn(move || {
                    if done_rx.recv_timeout(wait).is_err() {
                        stop.store(true, Ordering::SeqCst);
                    }
                })
                .map_err(|e| SandboxError::Host(format!("启动 JS 超时监控线程失败: {e}")))?;
        }

        let (tx, rx) = mpsc::channel::<SandboxResult<Json>>();
        std::thread::Builder::new()
            .name("vbox-js-spider".into())
            .stack_size(STACK_SIZE)
            .spawn(move || {
                let result = run_worker(job);
                let _ = tx.send(result);
            })
            .map_err(|e| SandboxError::Host(format!("启动 JS 执行线程失败: {e}")))?;

        let received = rx.recv_timeout(self.timeout);
        // 让看门狗线程退出（recv 成功或超时后都安全）
        drop(done_tx);
        match received {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => Err(SandboxError::Script(format!(
                "脚本执行超时（>{}），已放弃等待",
                secs_label(self.timeout)
            ))),
            Err(RecvTimeoutError::Disconnected) => Err(SandboxError::Script(
                "JS 执行线程异常退出（可能 panic）".into(),
            )),
        }
    }
}

/* ============================================================
 * 工作线程
 * ============================================================ */

struct Job {
    code: String,
    name: String,
    rules: Rules,
    headers: HashMap<String, String>,
    init: Option<Json>,
    func: String,
    args: Vec<JsArg>,
    max_body: usize,
    timeout: Duration,
    base_url: Option<String>,
    site_key: Option<String>,
    stop: Arc<AtomicBool>,
}

/// 宿主环境：一次调用内共享的 HTTP 客户端与上下文
struct HostEnv {
    /// 构造失败的客户端在真正发请求时才报错，避免脚本完全不可用
    client: std::result::Result<reqwest::blocking::Client, String>,
    extra_headers: HashMap<String, String>,
    script_name: String,
    max_body: usize,
    /// 本地代理桥根地址（`getProxy` / `js2Proxy` 生成回环 URL 用）
    local_base: Option<String>,
    /// 当前站点 key（`js2Proxy` 生成的代理 URL 会带 `siteKey`）
    site_key: Option<String>,
}

fn run_worker(job: Job) -> SandboxResult<Json> {
    // rquickjs 在误用时会 panic，这里兜底转成可读错误
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_inner(job)));
    match out {
        Ok(r) => r,
        Err(payload) => Err(SandboxError::Script(format!(
            "JS 运行时 panic: {}",
            panic_message(payload.as_ref())
        ))),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "未知 panic".into()
    }
}

fn run_inner(job: Job) -> SandboxResult<Json> {
    let rt =
        Runtime::new().map_err(|e| SandboxError::Script(format!("创建 JS Runtime 失败: {e}")))?;
    rt.set_memory_limit(MEMORY_LIMIT);
    rt.set_max_stack_size(STACK_SIZE);
    {
        let stop = job.stop.clone();
        rt.set_interrupt_handler(Some(Box::new(move || stop.load(Ordering::SeqCst))));
    }
    let ctx = Context::full(&rt)
        .map_err(|e| SandboxError::Script(format!("创建 JS Context 失败: {e}")))?;

    let out = ctx.with(|ctx| invoke(&ctx, &job));
    if job.stop.load(Ordering::SeqCst) && out.is_err() {
        return Err(SandboxError::Script(format!(
            "脚本执行超时（>{}），已被中断",
            secs_label(job.timeout)
        )));
    }
    out
}

fn invoke<'js>(ctx: &Ctx<'js>, job: &Job) -> SandboxResult<Json> {
    let env = Arc::new(HostEnv {
        client: crate::http::client_for(&job.rules).map_err(|e| e.to_string()),
        extra_headers: job.headers.clone(),
        script_name: job.name.clone(),
        max_body: job.max_body,
        local_base: crate::proxy::local_base(),
        site_key: job.site_key.clone(),
    });
    install(ctx, env.clone()).map_err(|e| js_err(ctx, e))?;
    // 把 Rust 侧返回的 JSON 数组包装成真正的 JS Array（pdfa/pdfla）
    ctx.eval::<(), _>(PDF_ARRAY_PRELUDE)
        .catch(ctx)
        .map_err(|e| SandboxError::Host(err_text(ctx, e)))?;

    // 1) 执行脚本：优先走 ES module / CommonJS 模块加载（相对 URL 依赖 + 内置 lib），
    //    模块构建失败时回退到旧的「剥离 import/export」全局脚本模式
    let bundle = if job.base_url.is_some() && has_module_syntax(&job.code) {
        match build_module_bundle(&env, &job.code, job.base_url.as_deref(), &job.stop) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    target: "vbox_core::spider_js",
                    "脚本 {} 的模块加载失败，回退到全局脚本模式: {}",
                    job.name,
                    e
                );
                None
            }
        }
    } else {
        None
    };
    match bundle {
        Some((prelude, entry)) => {
            if let Err(e) = ctx.eval::<Value, _>(prelude.as_str()).catch(ctx) {
                return Err(SandboxError::Script(format!(
                    "脚本模块预加载失败（{}）: {}",
                    job.name,
                    err_text(ctx, e)
                )));
            }
            if let Err(e) = ctx.eval::<Value, _>(entry.as_str()).catch(ctx) {
                return Err(SandboxError::Script(format!(
                    "脚本加载失败（{}）: {}",
                    job.name,
                    err_text(ctx, e)
                )));
            }
            if let Err(e) = ctx.eval::<Value, _>(ENTRY_EXPORTS_GLUE).catch(ctx) {
                tracing::warn!(
                    target: "vbox_core::spider_js",
                    "脚本 {} 的入口导出映射失败: {}",
                    job.name,
                    err_text(ctx, e)
                );
            }
        }
        None => {
            let code = sanitize_module_syntax(&job.code);
            if let Err(e) = ctx.eval::<Value, _>(code.as_str()).catch(ctx) {
                return Err(SandboxError::Script(format!(
                    "脚本加载失败（{}）: {}",
                    job.name,
                    err_text(ctx, e)
                )));
            }
        }
    }
    drain_jobs(ctx);

    // 2) 解析入口对象：__jsEvalReturn() / __JS_SPIDER__ / 全局函数
    let globals = ctx.globals();
    let entry = resolve_entry(ctx, &job.name)?;

    // 3) init(...)：jun 对 init 的异常是吞掉的，这里保持一致（失败只记日志）
    if let Some(init) = &job.init {
        if let Some(init_fn) = find_function(ctx, entry.as_ref(), &globals, "init")? {
            match ctx.json_parse(init.to_string()) {
                Ok(arg) => {
                    if let Err(e) = init_fn.call::<_, Value>((arg,)).catch(ctx) {
                        tracing::warn!(
                            target: "vbox_core::spider_js",
                            "脚本 {} 的 init() 执行失败（已忽略）: {}",
                            job.name,
                            err_text(ctx, e)
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        target: "vbox_core::spider_js",
                        "脚本 {} 的 init 参数不是合法 JSON（已忽略）: {}",
                        job.name,
                        js_err(ctx, e)
                    );
                }
            }
        }
    }
    drain_jobs(ctx);

    // 4) 找到目标函数（入口对象优先，其次全局）
    let func = find_function(ctx, entry.as_ref(), &globals, &job.func)?
        .ok_or_else(|| SandboxError::Missing(job.func.clone()))?;

    // 5) 逐个压入实参并调用
    let mut js_args = Args::new(ctx.clone(), job.args.len());
    for a in &job.args {
        js_args
            .push_arg(js_arg_to_value(ctx, a)?)
            .map_err(|e| js_err(ctx, e))?;
    }
    let ret: Value = js_args.apply(&func).catch(ctx).map_err(|e| {
        SandboxError::Script(format!(
            "脚本 {} 的 {}() 执行异常: {}",
            job.name,
            job.func,
            err_text(ctx, e)
        ))
    })?;

    // 6) Promise 解包 + 归一为 JSON
    let ret = resolve_async(ctx, ret)?;
    js_to_json(ctx, &ret)
}

/* ============================================================
 * 入口函数解析
 * ============================================================ */

/// 解析入口对象：优先 `__jsEvalReturn()`，其次 `__JS_SPIDER__`，都没有则用全局对象
fn resolve_entry<'js>(ctx: &Ctx<'js>, script_name: &str) -> SandboxResult<Option<Object<'js>>> {
    let globals = ctx.globals();

    let eval_return: Value<'js> = globals.get("__jsEvalReturn").map_err(|e| js_err(ctx, e))?;
    if let Some(f) = eval_return.as_function() {
        let ret: Value<'js> = f.call::<(), Value<'js>>(()).catch(ctx).map_err(|e| {
            SandboxError::Script(format!(
                "脚本 {} 的 __jsEvalReturn() 执行失败: {}",
                script_name,
                err_text(ctx, e)
            ))
        })?;
        let ret = resolve_async(ctx, ret)?;
        if let Some(obj) = ret.as_object() {
            return Ok(Some(obj.clone()));
        }
    }

    let spider: Value<'js> = globals.get("__JS_SPIDER__").map_err(|e| js_err(ctx, e))?;
    if let Some(f) = spider.as_function() {
        let ret: Value<'js> = f.call::<(), Value<'js>>(()).catch(ctx).map_err(|e| {
            SandboxError::Script(format!(
                "脚本 {} 的 __JS_SPIDER__() 执行失败: {}",
                script_name,
                err_text(ctx, e)
            ))
        })?;
        let ret = resolve_async(ctx, ret)?;
        if let Some(obj) = ret.as_object() {
            return Ok(Some(obj.clone()));
        }
    } else if let Some(obj) = spider.as_object() {
        return Ok(Some(obj.clone()));
    }

    Ok(None)
}

/// 在入口对象里找函数；找不到再回退到全局对象
fn find_function<'js>(
    ctx: &Ctx<'js>,
    entry: Option<&Object<'js>>,
    globals: &Object<'js>,
    name: &str,
) -> SandboxResult<Option<Function<'js>>> {
    if let Some(entry) = entry {
        let v: Value<'js> = entry.get(name).map_err(|e| js_err(ctx, e))?;
        if let Some(f) = v.as_function() {
            return Ok(Some(f.clone()));
        }
    }
    let v: Value<'js> = globals.get(name).map_err(|e| js_err(ctx, e))?;
    Ok(v.as_function().cloned())
}

/* ============================================================
 * 值转换
 * ============================================================ */

fn js_arg_to_value<'js>(ctx: &Ctx<'js>, arg: &JsArg) -> SandboxResult<Value<'js>> {
    Ok(match arg {
        JsArg::Null => Value::new_null(ctx.clone()),
        JsArg::Bool(b) => Value::new_bool(ctx.clone(), *b),
        JsArg::Str(s) => s.as_str().into_js(ctx).map_err(|e| js_err(ctx, e))?,
        JsArg::Num(n) => n.into_js(ctx).map_err(|e| js_err(ctx, e))?,
        // 用 JSON.parse 而不是手写对象，避免转义/类型细节出错
        JsArg::Json(v) => ctx.json_parse(v.to_string()).map_err(|e| js_err(ctx, e))?,
    })
}

/// 驱动 QuickJS 微任务队列（Promise 的 then/await 续体）。
fn drain_jobs(ctx: &Ctx<'_>) {
    while ctx.execute_pending_job() {}
}

/// 若脚本返回 Promise，则驱动 job 队列直至完成并取出结果；否则原样返回。
fn resolve_async<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> SandboxResult<Value<'js>> {
    if !value.is_promise() {
        return Ok(value);
    }
    let promise = value
        .into_promise()
        .ok_or_else(|| SandboxError::Script("Promise 转换失败".into()))?;
    match promise.finish::<Value>() {
        Ok(v) => Ok(v),
        Err(rquickjs::Error::WouldBlock) => Err(SandboxError::Script(
            "脚本返回了一个未完成的 Promise（需要真实事件循环的异步任务在同步宿主下无法完成）"
                .into(),
        )),
        Err(e) => Err(js_err(ctx, e)),
    }
}

fn js_to_json<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> SandboxResult<Json> {
    if v.is_null() || v.is_undefined() {
        return Ok(Json::Null);
    }
    if let Some(b) = v.as_bool() {
        return Ok(Json::Bool(b));
    }
    if let Some(n) = v.as_number() {
        return Ok(number_to_json(n));
    }
    if let Some(s) = v.as_string() {
        let text = s.to_string().map_err(|e| js_err(ctx, e))?;
        return Ok(parse_json_or_string(&text));
    }
    match ctx.json_stringify(v.clone()) {
        Ok(Some(s)) => {
            let text = s.to_string().map_err(|e| js_err(ctx, e))?;
            Ok(parse_json_or_string(&text))
        }
        // JSON.stringify 对函数/undefined 返回 undefined
        Ok(None) => Ok(Json::Null),
        Err(e) => Err(js_err(ctx, e)),
    }
}

fn number_to_json(n: f64) -> Json {
    if !n.is_finite() {
        return Json::Null;
    }
    if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 {
        return Json::from(n as i64);
    }
    serde_json::Number::from_f64(n)
        .map(Json::Number)
        .unwrap_or(Json::Null)
}


/* ============================================================
 * ES module / CommonJS 模块加载
 * ============================================================ */

fn module_cache() -> &'static Mutex<HashMap<String, (Instant, String)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, String)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn js_string_literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// 说明符的文件名部分（MisaTV 策略：`.js` 名称命中内置 lib 时优先使用内置资产）
fn module_basename(spec: &str) -> &str {
    spec.rsplit('/').next().unwrap_or(spec)
}

/// 内置 lib（随二进制编译；来源见 `assets/js_lib/README.md`）
fn builtin_module(name: &str) -> Option<&'static str> {
    match name {
        "cat.js" => Some(include_str!("../../assets/js_lib/cat.js")),
        "cheerio.min.js" => Some(include_str!("../../assets/js_lib/cheerio.min.js")),
        "crypto-js.js" => Some(include_str!("../../assets/js_lib/crypto-js.js")),
        "gbk.js" => Some(include_str!("../../assets/js_lib/gbk.js")),
        "net.js" => Some(include_str!("../../assets/js_lib/net.js")),
        "similarity.js" => Some(include_str!("../../assets/js_lib/similarity.js")),
        "utils.js" => Some(include_str!("../../assets/js_lib/utils.js")),
        "模板.js" => Some(include_str!("../../assets/js_lib/模板.js")),
        _ => None,
    }
}

/// 把模块说明符解析成 `lib:<name>` 或绝对 URL；`None` 表示无法解析
fn resolve_module_spec(base: Option<&str>, spec: &str) -> Option<String> {
    let raw = spec.trim();
    if raw.is_empty() {
        return None;
    }
    let clean = raw.split(['?', '#']).next().unwrap_or(raw).trim();
    if clean.is_empty() {
        return None;
    }
    let name = module_basename(clean);
    if clean.ends_with(".js") && builtin_module(name).is_some() {
        return Some(format!("lib:{name}"));
    }
    if let Some(rest) = clean.strip_prefix("assets://js/lib/") {
        return Some(match builtin_module(rest) {
            Some(_) => format!("lib:{rest}"),
            None => format!("https://raw.githubusercontent.com/hjdhnx/dr_py/main/libs/{rest}"),
        });
    }
    if clean.starts_with("http://") || clean.starts_with("https://") {
        return Some(clean.to_string());
    }
    if let Some(rest) = clean.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    let base = base?;
    let joined = url::Url::parse(base).ok()?.join(clean).ok()?;
    Some(joined.to_string())
}

fn spec_key(base: Option<&str>, spec: &str, children: &mut Vec<String>) -> Option<String> {
    let key = resolve_module_spec(base, spec)?;
    if !children.iter().any(|k| k == &key) {
        children.push(key.clone());
    }
    Some(key)
}

/// 取引号内的文本（支持单双引号）
fn extract_quoted(text: &str) -> Option<String> {
    let t = text.trim();
    let first = t.chars().next()?;
    if first != '\'' && first != '"' {
        return None;
    }
    let rest = &t[first.len_utf8()..];
    let end = rest.find(first)?;
    Some(rest[..end].to_string())
}

fn split_from(text: &str) -> Option<(&str, String)> {
    let idx = text.find(" from ")?;
    let clause = text[..idx].trim();
    let after = &text[idx + " from ".len()..];
    let spec = extract_quoted(after)?;
    Some((clause, spec))
}

fn is_js_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c == '$' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
}

/// 重写单行 `import` 语句（`rest` 为 `import` 之后的部分）
fn rewrite_import_line(
    rest: &str,
    base: Option<&str>,
    children: &mut Vec<String>,
) -> Option<String> {
    let body = rest.trim();
    if body.is_empty() {
        return None;
    }
    if let Some((clause, spec)) = split_from(body) {
        let key = spec_key(base, &spec, children)?;
        let key_lit = js_string_literal(&key);
        let clause = clause.trim();
        if clause.is_empty() {
            return Some(format!("__req({key_lit});"));
        }
        if clause.starts_with('{') && clause.ends_with('}') {
            let names = &clause[1..clause.len() - 1];
            let mut binds: Vec<String> = Vec::new();
            for part in names.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let mut it = part.split_whitespace();
                let Some(imported) = it.next() else { continue };
                let local = match it.next() {
                    Some("as") => it.next().unwrap_or(imported),
                    _ => imported,
                };
                if local == imported {
                    binds.push(local.to_string());
                } else {
                    binds.push(format!("{imported}: {local}"));
                }
            }
            return Some(format!("var {{ {} }} = __req({key_lit});", binds.join(", ")));
        }
        if let Some(ns) = clause.strip_prefix("* as ") {
            let ns = ns.trim();
            if is_js_ident(ns) {
                return Some(format!("var {ns} = __req({key_lit});"));
            }
            return None;
        }
        if is_js_ident(clause) {
            return Some(format!(
                "var __def = __req({key_lit}); var {clause} = (__def && __def.default !== undefined) ? __def.default : __def;"
            ));
        }
        return None;
    }
    let spec = extract_quoted(body)?;
    let key = spec_key(base, &spec, children)?;
    Some(format!("__req({});", js_string_literal(&key)))
}

fn export_decl_name(body: &str) -> Option<&'static str> {
    for kw in ["async function", "function", "class", "const", "let", "var"] {
        let Some(rest) = body.strip_prefix(kw) else { continue };
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('*').unwrap_or(rest).trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| *c == '_' || *c == '$' || c.is_ascii_alphanumeric())
            .collect();
        if name.is_empty() {
            continue;
        }
        return Some(match kw {
            "class" => "class",
            "const" => "const",
            "let" => "let",
            "var" => "var",
            _ => "function",
        });
    }
    None
}

/// 重写单行 `export` 语句（`rest` 为 `export` 之后的部分）
fn rewrite_export_line(
    rest: &str,
    base: Option<&str>,
    children: &mut Vec<String>,
    exported_names: &mut Vec<String>,
) -> Option<String> {
    let body = rest.trim();
    if body.is_empty() {
        return None;
    }
    if let Some(after) = body.strip_prefix("default ") {
        return Some(format!("module.exports.default = {};", after.trim()));
    }
    if body.starts_with('*') {
        let spec = split_from(body).map(|(_, s)| s)?;
        let key = spec_key(base, &spec, children)?;
        return Some(format!(
            "var __re = __req({}); for (var __rk in __re) {{ if (__rk !== \"default\") module.exports[__rk] = __re[__rk]; }}",
            js_string_literal(&key)
        ));
    }
    if body.starts_with('{') {
        let close = body.find('}')?;
        let names = &body[1..close];
        let after = body[close + 1..].trim();
        let mut entries: Vec<(String, String)> = Vec::new();
        for part in names.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let mut it = part.split_whitespace();
            let Some(orig) = it.next() else { continue };
            let exported = match it.next() {
                Some("as") => it.next().unwrap_or(orig).to_string(),
                _ => orig.to_string(),
            };
            entries.push((orig.to_string(), exported));
        }
        if let Some(spec) = after.strip_prefix("from ") {
            let spec = extract_quoted(spec)?;
            let key = spec_key(base, &spec, children)?;
            let mut code = format!("var __re = __req({});", js_string_literal(&key));
            for (orig, exported) in entries {
                code.push_str(&format!(" module.exports.{exported} = __re.{orig};"));
            }
            return Some(code);
        }
        if entries.is_empty() {
            return Some(String::new());
        }
        let mut code = String::new();
        for (orig, exported) in entries {
            code.push_str(&format!("module.exports.{exported} = {orig}; "));
        }
        return Some(code);
    }
    let kw = export_decl_name(body)?;
    let rest2 = body[kw.len()..].trim_start();
    let rest2 = rest2.strip_prefix('*').unwrap_or(rest2).trim_start();
    let name: String = rest2
        .chars()
        .take_while(|c| *c == '_' || *c == '$' || c.is_ascii_alphanumeric())
        .collect();
    if name.is_empty() {
        return None;
    }
    exported_names.push(name);
    let _ = kw;
    Some(body.to_string())
}

/// 把行内 `require('spec')` / `require("spec")` 替换为 `__req("<key>")`
fn rewrite_requires_in_line(
    line: &str,
    base: Option<&str>,
    children: &mut Vec<String>,
) -> Option<String> {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    let mut changed = false;
    while let Some(idx) = rest.find("require") {
        let prev_ok = match rest[..idx].chars().next_back() {
            None => true,
            Some(prev) => !(prev == '_' || prev == '$' || prev.is_ascii_alphanumeric()),
        };
        if !prev_ok {
            out.push_str(&rest[..idx + "require".len()]);
            rest = &rest[idx + "require".len()..];
            continue;
        }
        out.push_str(&rest[..idx]);
        let after = &rest[idx + "require".len()..];
        let after_trim = after.trim_start();
        if let Some(after_paren) = after_trim.strip_prefix('(') {
            let inner = after_paren.trim_start();
            if let Some(spec) = extract_quoted(inner) {
                let consumed = (after.len() - after_trim.len())
                    + 1
                    + (after_paren.len() - inner.len())
                    + 1;
                let spec_len = spec.len() + 2;
                if let Some(key) = spec_key(base, &spec, children) {
                    out.push_str(&format!("__req({})", js_string_literal(&key)));
                    rest = &after[consumed + spec_len..];
                    changed = true;
                    continue;
                }
                out.push_str(&rest[idx..idx + consumed + spec_len]);
                rest = &after[consumed + spec_len..];
                continue;
            }
        }
        out.push_str(&rest[idx..idx + "require".len()]);
        rest = after;
    }
    out.push_str(rest);
    if changed {
        Some(out)
    } else {
        None
    }
}

/// 是否包含需要模块加载的语法（避免把普通全局脚本也走模块管线）
fn has_module_syntax(code: &str) -> bool {
    code.contains("import ")
        || code.contains("import{")
        || code.contains("import*")
        || code.contains("export ")
        || code.contains("export{")
        || code.contains("require(")
}

/// 处理压缩/内联写法里的 `export{...}`（整行扫描无法覆盖 cat.js 这类单行 bundle）
fn rewrite_inline_exports(code: &str) -> String {
    let mut text = code.to_string();
    let Ok(re) = regex::Regex::new(r"\bexport\s*\{([^{}]*)\}") else {
        return text;
    };
    let mut replacements: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for cap in re.captures_iter(&text) {
        let Some(whole) = cap.get(0) else { continue };
        let Some(names) = cap.get(1) else { continue };
        let mut emitted = String::new();
        for part in names.as_str().split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let mut it = part.split_whitespace();
            let Some(local) = it.next() else { continue };
            let exported = match it.next() {
                Some("as") => it.next().unwrap_or(local),
                _ => local,
            };
            emitted.push_str(&format!("module.exports.{exported} = {local}; "));
        }
        if !emitted.is_empty() {
            replacements.push((whole.range(), emitted));
        }
    }
    for (range, replaced) in replacements.into_iter().rev() {
        text.replace_range(range, &replaced);
    }
    if let Ok(re_default) = regex::Regex::new(r"\bexport\s+default\s+") {
        text = re_default
            .replace_all(&text, "module.exports.default = ")
            .to_string();
    }
    text
}

/// 把模块源码重写为可执行的 CommonJS 函数体，并收集依赖（`children`）
fn rewrite_module_code(code: &str, base: Option<&str>, children: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(code.len() + 512);
    let mut exported_names: Vec<String> = Vec::new();
    for line in code.lines() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        let mut handled = false;
        let after_kw = |kw: &str| -> Option<&str> {
            let rest = trimmed.strip_prefix(kw)?;
            let head = rest.chars().next();
            let stmt = match head {
                None => true,
                Some(c) => c.is_whitespace() || c == '{' || c == '*' || c == '\'' || c == '"',
            };
            if stmt { Some(rest) } else { None }
        };
        if let Some(rest) = after_kw("import") {
            if let Some(rewritten) = rewrite_import_line(rest, base, children) {
                out.push_str(indent);
                out.push_str(&rewritten);
                out.push('\n');
                handled = true;
            }
        } else if let Some(rest) = after_kw("export") {
            if let Some(rewritten) =
                rewrite_export_line(rest, base, children, &mut exported_names)
            {
                if !rewritten.trim().is_empty() {
                    out.push_str(indent);
                    out.push_str(&rewritten);
                    out.push('\n');
                }
                handled = true;
            }
        }
        if handled {
            continue;
        }
        if let Some(rewritten) = rewrite_requires_in_line(line, base, children) {
            out.push_str(&rewritten);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    for name in exported_names {
        out.push_str(&format!(" module.exports.{name} = {name};\n"));
    }
    if out.contains("export") {
        rewrite_inline_exports(&out)
    } else {
        out
    }
}

/// 加载一个模块的源码：内置 lib、file://、http(s)（带 10 分钟缓存）
fn load_module_source(env: &HostEnv, key: &str, stop: &AtomicBool) -> SandboxResult<String> {
    if let Some(name) = key.strip_prefix("lib:") {
        return builtin_module(name)
            .map(|s| s.to_string())
            .ok_or_else(|| SandboxError::Host(format!("内置模块不存在: {name}")));
    }
    if stop.load(Ordering::SeqCst) {
        return Err(SandboxError::Script("模块加载被中断（脚本超时）".into()));
    }
    if key.starts_with("file://") {
        let path = url::Url::parse(key)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .ok_or_else(|| SandboxError::Host(format!("非法模块地址: {key}")))?;
        return std::fs::read_to_string(&path)
            .map_err(|e| SandboxError::Host(format!("读取模块失败 {}: {e}", path.display())));
    }
    if !(key.starts_with("http://") || key.starts_with("https://")) {
        return Err(SandboxError::Host(format!("不支持的模块地址: {key}")));
    }
    {
        let cache = module_cache()
            .lock()
            .map_err(|e| SandboxError::Host(format!("模块缓存锁失败: {e}")))?;
        if let Some((at, text)) = cache.get(key) {
            if at.elapsed() < MODULE_CACHE_TTL {
                return Ok(text.clone());
            }
        }
    }
    let client = env
        .client
        .as_ref()
        .map_err(|e| SandboxError::Host(format!("HTTP 客户端不可用: {e}")))?;
    let mut req = client.get(key).timeout(Duration::from_secs(15));
    for (k, v) in &env.extra_headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req
        .send()
        .map_err(|e| SandboxError::Host(format!("加载模块失败 {key}: {e}")))?;
    if !resp.status().is_success() {
        return Err(SandboxError::Host(format!(
            "加载模块失败 {key}: HTTP {}",
            resp.status()
        )));
    }
    let mut body = String::new();
    let mut reader = resp.take(env.max_body as u64);
    reader
        .read_to_string(&mut body)
        .map_err(|e| SandboxError::Host(format!("读取模块失败 {key}: {e}")))?;
    if body.trim().is_empty() {
        return Err(SandboxError::Host(format!("模块内容为空: {key}")));
    }
    if let Ok(mut cache) = module_cache().lock() {
        if cache.len() >= MODULE_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key.to_string(), (Instant::now(), body.clone()));
    }
    Ok(body)
}

/// 收集入口脚本的全部 ESM/CJS 依赖并生成：
/// 1) 模块定义 prelude（`__mods`/`__req` 两个全局）；2) 重写后的入口脚本
fn build_module_bundle(
    env: &HostEnv,
    entry_code: &str,
    base_url: Option<&str>,
    stop: &AtomicBool,
) -> SandboxResult<Option<(String, String)>> {
    let Some(base) = base_url else {
        return Ok(None);
    };
    let entry_key = base.to_string();
    let mut visited: Vec<String> = vec![entry_key.clone()];
    let mut queue: VecDeque<(String, String, bool)> = VecDeque::new();
    queue.push_back((entry_key, entry_code.to_string(), true));
    let mut modules: Vec<(String, String)> = Vec::new();
    let mut entry_rewritten = String::new();
    let mut total_bytes = entry_code.len();
    while let Some((key, code, is_entry)) = queue.pop_front() {
        if stop.load(Ordering::SeqCst) {
            return Err(SandboxError::Script("模块加载被中断（脚本超时）".into()));
        }
        let module_base = if key.starts_with("lib:") {
            None
        } else {
            Some(key.as_str())
        };
        let mut children: Vec<String> = Vec::new();
        let rewritten = rewrite_module_code(&code, module_base, &mut children);
        for child in children {
            if visited.iter().any(|k| k == &child) {
                continue;
            }
            if visited.len() > MAX_MODULES {
                return Err(SandboxError::Host(format!(
                    "ES 模块依赖过多（>{MAX_MODULES}），已放弃模块加载"
                )));
            }
            let src = load_module_source(env, &child, stop)?;
            total_bytes += src.len();
            if total_bytes > MAX_MODULE_BYTES {
                return Err(SandboxError::Host(format!(
                    "ES 模块总体积超过上限（>{MAX_MODULE_BYTES} 字节）"
                )));
            }
            visited.push(child.clone());
            queue.push_back((child, src, false));
        }
        if is_entry {
            entry_rewritten = rewritten;
        } else {
            modules.push((key, rewritten));
        }
    }
    let mut prelude = String::with_capacity(1024 + total_bytes);
    prelude.push_str("(function(){\nvar __mods = {};\nvar __cache = {};\n");
    for (key, code) in &modules {
        prelude.push_str("__mods[");
        prelude.push_str(&js_string_literal(key));
        prelude.push_str("] = function(module, exports, require){\n");
        prelude.push_str(code);
        prelude.push_str("\n};\n");
    }
    prelude.push_str(
        "function __req(__k){\n  var __c = __cache[__k]; if (__c) return __c.exports;\n  var __f = __mods[__k]; if (!__f) throw new Error(\"ES 模块未加载: \" + __k);\n  var __m = { exports: {} }; __cache[__k] = __m; __f(__m, __m.exports, __req); return __m.exports;\n}\nglobalThis.__req = __req;\nglobalThis.module = globalThis.module || { exports: {} };\nglobalThis.exports = globalThis.module.exports;\n})();\n",
    );
    Ok(Some((prelude, entry_rewritten)))
}

/// 字符串优先当 JSON 解析；明显不是 JSON 的文本原样保留
/// 去掉 ES module 的 import/export 外壳，把脚本降级为全局脚本执行。
/// catvod/drpy 的脚本大量使用 `import { ... } from './lib/cat.js'`，但宿主函数
/// 已经以全局形式提供；去掉 import 后多数脚本可直接运行。
fn sanitize_module_syntax(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("import ")
            || trimmed.starts_with("import{")
            || trimmed.starts_with("import*")
            || trimmed.starts_with("export {")
            || trimmed.starts_with("export{")
        {
            continue;
        }
        let line = if let Some(rest) = trimmed.strip_prefix("export default ") {
            format!(
                "{}globalThis.__JS_SPIDER__ = {}",
                &line[..line.len() - trimmed.len()],
                rest
            )
        } else if let Some(rest) = trimmed.strip_prefix("export ") {
            format!("{}{}", &line[..line.len() - trimmed.len()], rest)
        } else {
            line.to_string()
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn parse_json_or_string(text: &str) -> Json {
    let t = text.trim();
    if t.is_empty() {
        return Json::String(text.to_string());
    }
    let first = t.as_bytes()[0];
    let looks_json = matches!(first, b'{' | b'[' | b'"') || matches!(t, "true" | "false" | "null");
    if !looks_json {
        return Json::String(text.to_string());
    }
    serde_json::from_str::<Json>(t).unwrap_or_else(|_| Json::String(text.to_string()))
}

/// 把任意 JS 值转成日志文本
/// `aesX` 的 Rust 实现（AES-128 CBC/ECB PKCS7；覆盖 catvod 脚本常用形态）。
fn aes_x(
    mode: &str,
    encrypt: bool,
    input: &str,
    in_base64: bool,
    key: &str,
    iv: &str,
    out_base64: bool,
) -> String {
    let mode = mode.to_ascii_uppercase();
    let key = fit16(key);
    let iv = fit16(iv);
    let data = if in_base64 {
        match base64::engine::general_purpose::STANDARD.decode(clean_base64(input)) {
            Ok(v) => v,
            Err(_) => return String::new(),
        }
    } else {
        input.as_bytes().to_vec()
    };
    let out = if mode.contains("CBC") {
        if encrypt {
            cbc::Encryptor::<Aes128>::new_from_slices(&key, &iv)
                .map(|c| c.encrypt_padded_vec_mut::<Pkcs7>(&data))
                .unwrap_or_default()
        } else {
            cbc::Decryptor::<Aes128>::new_from_slices(&key, &iv)
                .ok()
                .and_then(|c| c.decrypt_padded_vec_mut::<Pkcs7>(&data).ok())
                .unwrap_or_default()
        }
    } else if mode.contains("ECB") {
        if encrypt {
            ecb::Encryptor::<Aes128>::new_from_slice(&key)
                .map(|c| c.encrypt_padded_vec_mut::<Pkcs7>(&data))
                .unwrap_or_default()
        } else {
            ecb::Decryptor::<Aes128>::new_from_slice(&key)
                .ok()
                .and_then(|c| c.decrypt_padded_vec_mut::<Pkcs7>(&data).ok())
                .unwrap_or_default()
        }
    } else {
        Vec::new()
    };
    if out_base64 {
        base64::engine::general_purpose::STANDARD.encode(out)
    } else {
        String::from_utf8_lossy(&out).into_owned()
    }
}

fn fit16(text: &str) -> [u8; 16] {
    let mut key = [b'0'; 16];
    for (i, b) in text.as_bytes().iter().take(16).enumerate() {
        key[i] = *b;
    }
    key
}

fn js_value_text<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> String {
    if let Some(s) = v.as_string() {
        return s.to_string().unwrap_or_default();
    }
    if v.is_null() {
        return "null".into();
    }
    if v.is_undefined() {
        return "undefined".into();
    }
    match ctx.json_stringify(v.clone()) {
        Ok(Some(s)) => s.to_string().unwrap_or_default(),
        _ => format!("{:?}", v.type_of()),
    }
}

fn err_text<'js>(ctx: &Ctx<'js>, e: CaughtError<'js>) -> String {
    match e {
        CaughtError::Exception(ex) => {
            let msg = ex.message().unwrap_or_default();
            let stack = ex.stack().unwrap_or_default();
            if stack.is_empty() {
                msg
            } else {
                // 栈通常已包含消息，只取前 4 行避免错误信息过长
                let brief: Vec<&str> = stack.lines().take(4).collect();
                format!("{} [{}]", msg, brief.join(" | "))
            }
        }
        CaughtError::Value(v) => js_value_text(ctx, &v),
        CaughtError::Error(e) => e.to_string(),
    }
}

fn js_err<'js>(ctx: &Ctx<'js>, e: rquickjs::Error) -> SandboxError {
    let caught = CaughtError::from_error(ctx, e);
    SandboxError::Script(err_text(ctx, caught))
}

/* ============================================================
 * 宿主 API 注入
 * ============================================================ */

fn install<'js>(ctx: &Ctx<'js>, env: Arc<HostEnv>) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    /* ---- req / fetch / http ---- */
    let req_env = env.clone();
    let req_fn = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, url: String, opts: Opt<Value<'js>>| -> rquickjs::Result<Value<'js>> {
            let options = match opts.0 {
                Some(v) if !v.is_undefined() && !v.is_null() => match ctx.json_stringify(v) {
                    Ok(Some(s)) => s
                        .to_string()
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok())
                        .unwrap_or(Json::Null),
                    _ => Json::Null,
                },
                _ => Json::Null,
            };

            let obj = Object::new(ctx.clone())?;
            match do_request(&req_env, &url, &options) {
                Ok(res) => {
                    obj.set("code", res.code)?;
                    let headers = Object::new(ctx.clone())?;
                    for (k, v) in &res.headers {
                        // 非法 header 名（空白/控制字符）直接跳过，不让脚本整体失败
                        let _ = headers.set(k.as_str(), v.as_str());
                    }
                    obj.set("headers", headers)?;
                    obj.set("content", res.content)?;
                }
                Err(err) => {
                    // 对齐 jun（Connect.error）：失败也返回对象，content 为空
                    tracing::warn!(target: "vbox_core::spider_js", "req 失败: {} -> {}", url, err);
                    obj.set("code", 0)?;
                    obj.set("headers", Object::new(ctx.clone())?)?;
                    obj.set("content", "")?;
                    obj.set("error", err)?;
                }
            }
            Ok(obj.into_value())
        },
    )?;
    globals.set("req", req_fn.clone())?;
    globals.set("fetch", req_fn.clone())?;
    // jun 会把全局 req 指向 net.js 的 http；这里同名别名走同一套同步实现
    globals.set("http", req_fn)?;

    /* ---- log / console ---- */
    let log_env = env.clone();
    let log_fn = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, args: Rest<Value<'js>>| -> rquickjs::Result<()> {
            let text = args
                .0
                .iter()
                .map(|v| js_value_text(&ctx, v))
                .collect::<Vec<_>>()
                .join(" ");
            tracing::debug!(
                target: "vbox_core::spider_js",
                "[{}] {}",
                log_env.script_name,
                truncate_chars(&text, 2000)
            );
            Ok(())
        },
    )?;
    globals.set("log", log_fn.clone())?;
    globals.set("print", log_fn.clone())?;
    let console = Object::new(ctx.clone())?;
    console.set("log", log_fn.clone())?;
    console.set("info", log_fn.clone())?;
    console.set("warn", log_fn.clone())?;
    console.set("error", log_fn)?;
    globals.set("console", console)?;

    /* ---- joinUrl / join ---- */
    let join_fn = Function::new(ctx.clone(), |parent: String, child: String| -> String {
        join_url(&parent, &child)
    })?;
    globals.set("joinUrl", join_fn.clone())?;
    globals.set("join", join_fn)?;

    /* ---- md5 / base64 ---- */
    globals.set(
        "md5",
        Function::new(ctx.clone(), |s: String| -> String {
            format!("{:x}", md5::compute(s.as_bytes()))
        })?,
    )?;
    globals.set(
        "base64Encode",
        Function::new(ctx.clone(), |s: String| -> String {
            base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
        })?,
    )?;
    globals.set(
        "base64Decode",
        Function::new(ctx.clone(), |s: String| -> String {
            match base64::engine::general_purpose::STANDARD.decode(clean_base64(&s)) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => String::new(),
            }
        })?,
    )?;

    /* ---- pdfh / pdfa / pd / pdfla（Jsoup 规则引擎） ---- */
    globals.set(
        "pdfh",
        Function::new(ctx.clone(), |html: String, rule: String| -> String {
            crate::html_rule::pdfh(&html, &rule)
        })?,
    )?;
    globals.set(
        "pd",
        Function::new(
            ctx.clone(),
            |html: String, rule: String, add_url: Opt<String>| -> String {
                crate::html_rule::pd(&html, &rule, add_url.0.as_deref().unwrap_or(""))
            },
        )?,
    )?;
    // JS 侧需要真正的 Array；Rust 侧返回 JSON 文本，prelude 里 JSON.parse 成数组
    globals.set(
        "__pdfaRaw",
        Function::new(ctx.clone(), |html: String, rule: String| -> String {
            serde_json::to_string(&crate::html_rule::pdfa(&html, &rule))
                .unwrap_or_else(|_| "[]".into())
        })?,
    )?;
    globals.set(
        "__pdflaRaw",
        Function::new(
            ctx.clone(),
            |html: String,
             p1: String,
             list_text: String,
             list_url: String,
             add_url: Opt<String>|
             -> String {
                serde_json::to_string(&crate::html_rule::pdfla(
                    &html,
                    &p1,
                    &list_text,
                    &list_url,
                    add_url.0.as_deref().unwrap_or(""),
                ))
                .unwrap_or_else(|_| "[]".into())
            },
        )?,
    )?;

    /* ---- crypto / proxy 兼容层 ---- */
    globals.set(
        "aesX",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, args: Rest<Value<'js>>| -> String {
                let text = |i: usize| {
                    args.0
                        .get(i)
                        .map(|v| js_value_text(&ctx, v))
                        .unwrap_or_default()
                };
                let flag = |i: usize| args.0.get(i).and_then(|v| v.as_bool()).unwrap_or(false);
                aes_x(
                    &text(0),
                    flag(1),
                    &text(2),
                    flag(3),
                    &text(4),
                    &text(5),
                    flag(6),
                )
            },
        )?,
    )?;
    let proxy_env = env.clone();
    globals.set(
        "getProxy",
        Function::new(ctx.clone(), move |url: Opt<String>| -> String {
            let base = proxy_env.local_base.clone().unwrap_or_default();
            if base.is_empty() {
                return url.0.unwrap_or_default();
            }
            match url.0.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                Some(target) => format!(
                    "{base}/proxy?do=js&from=catvod&url={}",
                    crate::proxy::encode_param(target)
                ),
                None => format!("{base}/proxy"),
            }
        })?,
    )?;
    let proxy_env = env.clone();
    globals.set(
        "js2Proxy",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, args: Rest<Value<'js>>| -> String {
                let base = match proxy_env.local_base.as_deref() {
                    Some(base) if !base.is_empty() => base.to_string(),
                    _ => return String::new(),
                };
                let text = |index: usize| -> String {
                    args.0
                        .get(index)
                        .map(|value| js_value_text(&ctx, value))
                        .unwrap_or_default()
                };
                let headers = args
                    .0
                    .get(2)
                    .and_then(|value| match ctx.json_stringify(value) {
                        Ok(Some(text)) => text.to_string().ok(),
                        _ => None,
                    })
                    .unwrap_or_else(|| "{}".to_string());
                let headers: Json = serde_json::from_str(&headers).unwrap_or(Json::Null);
                crate::proxy::js2_proxy_url_with_base(
                    &base,
                    args.0.get(0).and_then(|v| v.as_bool()).unwrap_or(false),
                    args.0.get(1).and_then(|v| v.as_bool()).unwrap_or(false),
                    &headers,
                    &text(3),
                    &text(4),
                    &text(5),
                    proxy_env.site_key.as_deref(),
                )
            },
        )?,
    )?;
    globals.set(
        "s2t",
        Function::new(ctx.clone(), |text: String| -> String { text })?,
    )?;
    globals.set(
        "t2s",
        Function::new(ctx.clone(), |text: String| -> String { text })?,
    )?;

    /* ---- local.get/set/delete ---- */
    let ns = env.script_name.clone();
    let local = Object::new(ctx.clone())?;
    {
        let ns = ns.clone();
        local.set(
            "get",
            Function::new(
                ctx.clone(),
                move |key: String, default: Opt<String>| -> String {
                    let found = local_store()
                        .lock()
                        .ok()
                        .and_then(|m| m.get(&local_key(&ns, &key)).cloned());
                    found.unwrap_or_else(|| default.0.unwrap_or_default())
                },
            )?,
        )?;
    }
    {
        let ns = ns.clone();
        local.set(
            "set",
            Function::new(ctx.clone(), move |key: String, value: String| -> () {
                if let Ok(mut m) = local_store().lock() {
                    m.insert(local_key(&ns, &key), value);
                }
            })?,
        )?;
    }
    {
        let ns = ns.clone();
        local.set(
            "delete",
            Function::new(ctx.clone(), move |key: String| -> () {
                if let Ok(mut m) = local_store().lock() {
                    m.remove(&local_key(&ns, &key));
                }
            })?,
        )?;
    }
    globals.set("local", local)?;

    /* ---- setTimeout / clearTimeout ---- */
    globals.set(
        "setTimeout",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>,
                  func: Opt<Function<'js>>,
                  _delay: Opt<f64>|
                  -> rquickjs::Result<()> {
                let Some(func) = func.0 else {
                    return Ok(());
                };
                let depth = TIMER_DEPTH.with(|d| d.get());
                if depth >= MAX_TIMER_DEPTH {
                    return Ok(());
                }
                TIMER_DEPTH.with(|d| d.set(depth + 1));
                let res = func.call::<(), Value<'js>>(());
                TIMER_DEPTH.with(|d| d.set(depth));
                if let Err(e) = res.catch(&ctx) {
                    tracing::debug!(
                        target: "vbox_core::spider_js",
                        "setTimeout 回调异常（已忽略）: {}",
                        err_text(&ctx, e)
                    );
                }
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "clearTimeout",
        Function::new(ctx.clone(), |_id: Opt<Value<'js>>| -> () {})?,
    )?;

    Ok(())
}

/* ============================================================
 * req 实现
 * ============================================================ */

struct HostResponse {
    code: i32,
    headers: Vec<(String, String)>,
    content: String,
}

#[derive(Debug, Default)]
struct ReqOptions {
    method: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    data: Option<Json>,
    post_type: String,
    timeout_ms: u64,
    buffer: i64,
}

fn parse_req_options(raw: &Json) -> ReqOptions {
    let mut out = ReqOptions {
        method: "get".into(),
        post_type: "json".into(),
        timeout_ms: 15_000,
        ..Default::default()
    };
    let Some(obj) = raw.as_object() else {
        return out;
    };
    if let Some(m) = obj.get("method").and_then(|v| v.as_str()) {
        if !m.trim().is_empty() {
            out.method = m.trim().to_ascii_lowercase();
        }
    }
    if let Some(h) = obj.get("headers").and_then(|v| v.as_object()) {
        for (k, v) in h {
            let val = match v {
                Json::String(s) => s.clone(),
                Json::Number(n) => n.to_string(),
                Json::Bool(b) => b.to_string(),
                _ => continue,
            };
            out.headers.push((k.clone(), val));
        }
    }
    if let Some(b) = obj.get("body") {
        out.body = match b {
            Json::String(s) => Some(s.clone()),
            Json::Null => None,
            other => Some(other.to_string()),
        };
    }
    if let Some(d) = obj.get("data") {
        if !d.is_null() {
            out.data = Some(d.clone());
        }
    }
    if let Some(t) = obj.get("postType").and_then(|v| v.as_str()) {
        if !t.trim().is_empty() {
            out.post_type = t.trim().to_ascii_lowercase();
        }
    }
    if let Some(t) = obj.get("timeout").and_then(|v| v.as_u64()) {
        out.timeout_ms = t.clamp(1_000, 60_000);
    }
    if let Some(b) = obj.get("buffer").and_then(|v| v.as_i64()) {
        out.buffer = b;
    }
    out
}

fn do_request(
    env: &HostEnv,
    raw_url: &str,
    raw_opts: &Json,
) -> std::result::Result<HostResponse, String> {
    let url = normalize_req_url(raw_url);
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("req 仅支持 http(s) 地址: {url}"));
    }
    let client = env.client.as_ref().map_err(|e| e.clone())?;
    let opts = parse_req_options(raw_opts);

    let mut builder = match opts.method.as_str() {
        "post" => client.post(&url),
        "head" => client.head(&url),
        _ => client.get(&url),
    };
    // 站点级默认头先加，脚本显式头后加（后者覆盖前者）
    for (k, v) in &env.extra_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    for (k, v) in &opts.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    builder = builder.timeout(Duration::from_millis(opts.timeout_ms));
    // 说明：reqwest 的 blocking RequestBuilder 没有 per-request redirect 开关，
    // TVBox 的 `redirect:0` 选项在桌面端忽略（始终跟随重定向）。

    if opts.method == "post" {
        if let Some(data) = &opts.data {
            match opts.post_type.as_str() {
                "form" | "form-data" => {
                    // 注意：reqwest 未启用 multipart 特性，form-data 降级为表单编码
                    let mut pairs: Vec<(String, String)> = Vec::new();
                    if let Some(obj) = data.as_object() {
                        for (k, v) in obj {
                            let val = match v {
                                Json::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            pairs.push((k.clone(), val));
                        }
                    }
                    builder = builder.form(&pairs);
                }
                _ => {
                    if let Some(s) = data.as_str() {
                        builder = builder.body(s.to_string());
                    } else {
                        builder = builder.json(data);
                    }
                }
            }
        } else if let Some(body) = &opts.body {
            builder = builder.body(body.clone());
        } else {
            builder = builder.body(String::new());
        }
    }

    let mut resp = builder.send().map_err(|e| format!("请求失败: {e}"))?;
    let code = resp.status().as_u16() as i32;

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_type: Option<String> = None;
    for (k, v) in resp.headers().iter() {
        let val = v.to_str().unwrap_or("").to_string();
        if k.as_str().eq_ignore_ascii_case("content-type") {
            content_type = Some(val.clone());
        }
        headers.push((k.as_str().to_string(), val));
    }

    // 流式读取并截断，避免超大响应撑爆内存
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut limited = resp.by_ref().take(env.max_body as u64 + 1);
        limited
            .read_to_end(&mut buf)
            .map_err(|e| format!("读取响应体失败: {e}"))?;
    }
    let truncated = buf.len() > env.max_body;
    if truncated {
        tracing::warn!(
            target: "vbox_core::spider_js",
            "req 响应体超过 {} 字节，已截断: {}",
            env.max_body,
            url
        );
        buf.truncate(env.max_body);
    }
    let text = decode_body(content_type.as_deref(), &buf);
    let content = if opts.buffer == 2 {
        base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
    } else {
        text
    };

    Ok(HostResponse {
        code,
        headers,
        content,
    })
}

/// `req` 的地址归一：补协议相对地址，其余原样（非 http(s) 由调用方报错）
fn normalize_req_url(url: &str) -> String {
    let u = url.trim();
    if let Some(rest) = u.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        u.to_string()
    }
}

/// 复用 http.rs 的解码策略：按 Content-Type charset，未声明先 UTF-8 再 GBK
fn decode_body(content_type: Option<&str>, bytes: &[u8]) -> String {
    let charset = content_type.and_then(|ct| {
        ct.split(';').skip(1).find_map(|part| {
            let p = part.trim();
            p.strip_prefix("charset=")
                .map(|v| v.trim().trim_matches('"').to_ascii_lowercase())
        })
    });
    let encoding: &Encoding = match charset.as_deref() {
        Some("gbk") | Some("gb2312") | Some("gb18030") | Some("x-gbk") => GBK,
        Some("utf-8") | Some("utf8") => UTF_8,
        _ => {
            if std::str::from_utf8(bytes).is_ok() {
                UTF_8
            } else {
                GBK
            }
        }
    };
    if let Some(s) = encoding.decode_without_bom_handling_and_without_replacement(bytes) {
        return s.into_owned();
    }
    if encoding == UTF_8 {
        if let Some(s) = GBK.decode_without_bom_handling_and_without_replacement(bytes) {
            return s.into_owned();
        }
    }
    GBK.decode_without_bom_handling(bytes).0.into_owned()
}

/* ============================================================
 * 小工具
 * ============================================================ */

/// 按 TVBox `HtmlParser.joinUrl` 的语义解析相对地址
pub fn join_url(parent: &str, child: &str) -> String {
    let parent = parent.trim();
    let child = child.trim();
    if parent.is_empty() {
        return child.to_string();
    }
    if child.is_empty() {
        return parent.to_string();
    }
    // 绝对地址 / 协议相对地址不需要解析
    if child.starts_with("http://") || child.starts_with("https://") {
        return child.to_string();
    }
    if let Some(rest) = child.strip_prefix("//") {
        if let Some(pos) = parent.find("://") {
            return format!("{}://{rest}", &parent[..pos]);
        }
        return format!("https://{rest}");
    }
    match url::Url::parse(parent) {
        Ok(base) => base
            .join(child)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("{parent}{child}")),
        Err(_) => format!("{parent}{child}"),
    }
}

fn clean_base64(s: &str) -> String {
    let mut out: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    // Android Base64.DEFAULT 允许省略补位
    let rem = out.len() % 4;
    if rem != 0 {
        out.push_str(&"=".repeat(4 - rem));
    }
    out
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 超时文案：不足 1s 用毫秒显示（单测会把超时调小）
fn secs_label(d: Duration) -> String {
    if d.as_secs() > 0 {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn join_url_relative() {
        assert_eq!(
            join_url("https://a.com/x/y.html", "z.html"),
            "https://a.com/x/z.html"
        );
        assert_eq!(
            join_url("https://a.com/x/y.html", "/root.html"),
            "https://a.com/root.html"
        );
        assert_eq!(join_url("https://a.com/x/", "//b.com/z"), "https://b.com/z");
        assert_eq!(join_url("", "https://x/1"), "https://x/1");
    }

    #[test]
    fn parse_json_or_string_keeps_plain_text() {
        assert_eq!(parse_json_or_string("电影"), Json::String("电影".into()));
        assert_eq!(parse_json_or_string("{\"a\":1}"), json!({"a": 1}));
        assert_eq!(parse_json_or_string("123"), Json::String("123".into()));
    }

    #[test]
    fn base64_lenient() {
        assert_eq!(clean_base64("YQ"), "YQ==");
    }

    #[test]
    fn timeout_interrupts_busy_loop() {
        // 死循环必须被看门狗 + QuickJS 中断处理器打断，不能把进程拖死
        let sandbox = Sandbox::new(
            "function home(){ while(true){} }",
            "timeout-test",
            Rules::default(),
        )
        .with_timeout(Duration::from_millis(800));

        let start = std::time::Instant::now();
        let err = sandbox.call("home", &[]).expect_err("死循环应超时");
        let elapsed = start.elapsed();
        assert!(err.to_string().contains("超时"), "实际错误: {err}");
        assert!(
            elapsed < Duration::from_secs(5),
            "超时保护生效太慢: {elapsed:?}"
        );
    }

    #[test]
    fn missing_function_is_distinct_error() {
        let sandbox = Sandbox::new("function other(){}", "missing-test", Rules::default());
        match sandbox.call("home", &[]) {
            Err(SandboxError::Missing(name)) => assert_eq!(name, "home"),
            other => panic!("应返回 Missing(home)，实际: {other:?}"),
        }
    }

    #[test]
    fn script_exception_has_message() {
        let sandbox = Sandbox::new(
            "function home(){ throw new Error('boom') }",
            "error-test",
            Rules::default(),
        );
        let err = sandbox.call("home", &[]).expect_err("应抛错");
        assert!(err.to_string().contains("boom"), "实际: {err}");
    }
}

    #[test]
    fn proxy_globals_are_injected() {
        let sandbox = Sandbox::new(
            "function p(){ return { gp: typeof getProxy, j2: typeof js2Proxy, one: getProxy('https://a.example/b?c=1'), two: js2Proxy(false, false, {}, 'https://a.example/v.m3u8', '0', '') }; }",
            "proxy-globals-test",
            Rules::default(),
        );
        let value = sandbox.call("p", &[]).expect("proxy globals");
        assert_eq!(value["gp"], Json::String("function".into()));
        assert_eq!(value["j2"], Json::String("function".into()));
        let one = value["one"].as_str().unwrap_or("");
        assert!(one.contains("a.example"), "unexpected getProxy: {one}");
        assert!(value["two"].is_string(), "js2Proxy 应返回字符串");
    }

#[cfg(test)]
mod module_tests {
    use super::*;

    #[test]
    fn resolve_relative_spec() {
        let base = Some("https://example.com/js/entry.js");
        assert_eq!(
            resolve_module_spec(base, "./lib/util.js").as_deref(),
            Some("https://example.com/js/lib/util.js")
        );
        assert_eq!(
            resolve_module_spec(base, "../lib/a.js?x=1").as_deref(),
            Some("https://example.com/lib/a.js")
        );
        assert_eq!(
            resolve_module_spec(None, "https://x/y.js").as_deref(),
            Some("https://x/y.js")
        );
        assert_eq!(
            resolve_module_spec(None, "//cdn.example.com/a.js").as_deref(),
            Some("https://cdn.example.com/a.js")
        );
        assert_eq!(
            resolve_module_spec(base, "cat.js").as_deref(),
            Some("lib:cat.js")
        );
        assert_eq!(
            resolve_module_spec(base, "assets://js/lib/net.js").as_deref(),
            Some("lib:net.js")
        );
    }

    #[test]
    fn rewrite_named_default_and_side_effect_imports() {
        let code = "import { a, b as c } from './x.js';\nimport def from './y.js';\nimport * as ns from './z.js';\nimport './side.js';\nexport function home(){ return a + c + def + ns.v; }\nexport default { home };\n";
        let mut children = Vec::new();
        let out = rewrite_module_code(code, Some("https://e.com/p/entry.js"), &mut children);
        assert!(out.contains("__req(\"https://e.com/p/x.js\")"), "{out}");
        assert!(out.contains("var { a, b: c }"), "{out}");
        assert!(out.contains("__def && __def.default"), "{out}");
        assert!(out.contains("__req(\"https://e.com/p/z.js\")"), "{out}");
        assert!(out.contains("__req(\"https://e.com/p/side.js\")"), "{out}");
        assert!(!out.contains("import "), "{out}");
        assert!(out.contains("function home()"), "{out}");
        assert!(out.contains("module.exports.home = home;"), "{out}");
        assert!(out.contains("module.exports.default = "), "{out}");
        assert!(children.iter().any(|c| c == "https://e.com/p/x.js"));
        assert!(children.iter().any(|c| c == "https://e.com/p/side.js"));
    }

    #[test]
    fn rewrite_require_specifiers() {
        let code = "const u = require('./lib/mylib.js');\nconst v = require(\"./lib/other.js\");\nconst b = require('./utils.js');\nmodule.exports = { u: u, v: v, b: b };";
        let mut children = Vec::new();
        let out = rewrite_module_code(code, Some("https://e.com/js/entry.js"), &mut children);
        assert!(out.contains("__req(\"https://e.com/js/lib/mylib.js\")"), "{out}");
        assert!(out.contains("__req(\"https://e.com/js/lib/other.js\")"), "{out}");
        assert!(out.contains("__req(\"lib:utils.js\")"), "{out}");
        assert!(children.iter().any(|c| c == "https://e.com/js/lib/mylib.js"));
    }

    #[test]
    fn plain_script_skips_module_pipeline() {
        assert!(!has_module_syntax("function home(){ return {class: []}; }"));
        assert!(has_module_syntax("import { a } from './a.js';"));
        assert!(has_module_syntax("const a = require('./a.js');"));
    }
}
