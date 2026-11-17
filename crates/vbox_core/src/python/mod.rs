//! Python spider 运行时（CPython sidecar）
//!
//! TVBox 的 `.py` spider 是普通 Python 脚本（入口约定见 `host.py` 顶部注释），
//! 本模块用**系统 CPython 子进程**执行它们，不使用 pyo3 / Chaquopy：
//!
//! ```text
//! Rust ──stdin(JSON)──▶ python host.py ──stdout(JSON)──▶ Rust
//!          {script,ext,method,args}          {ok,result,error}
//! ```
//!
//! 设计要点：
//! - 每次接口调用启动一个独立进程（会话状态不跨调用保留），20s 超时后 kill；
//! - `host.py` 由 `include_str!` 嵌入，运行时写到临时目录（按内容 md5 命名，可复用）；
//! - 注入 `base` / `base.spider` / `base.localProxy` 模块，兼容
//!   `from base.spider import Spider` 的写法；
//! - 脚本来源支持 `http(s)://`（下载并缓存到临时目录 24h）、`file://`、`inline:`；
//!   也识别 `api = "py_XXX"` 且 `ext` 指向 `.py` 的配置；
//! - stdout 只解析协议 JSON，脚本 `print` 由 host.py 重定向到 stderr，不会污染协议。
//!
//! 运行环境要求（重要）：
//! - 需要本机安装 CPython 3.8+；解释器按 `VBOX_PYTHON`（绝对路径或“程序 参数”）、
//!   随包内置（`set_bundled`）、Windows 已知安装目录（`%LOCALAPPDATA%\Programs\Python\Python3*`、
//!   `%ProgramFiles%\Python3*`、py launcher）、`python`、`python3`、Windows 的 `py -3`
//!   顺序探测并缓存；
//! - Windows 的 `python.exe` / `python3.exe` 可能是 Microsoft Store 的应用执行别名占位
//!   （运行返回 9009），探测时会跳过这些假别名；
//! - 脚本按需依赖第三方库（`requests` / `bs4` / `lxml` / `pycryptodome` 等），
//!   **不随 VBox 打包**，缺失时脚本会报 `ModuleNotFoundError`；
//! - 这不是安全沙箱：脚本拥有当前用户的文件与网络权限，只应加载可信配置里的脚本。
//!
//! 明确不支持（返回明确错误，不静默）：
//! - `getDependence()` 自动下载依赖插件；
//! - `localProxy` / `getProxyUrl` 的本地 9978 代理（返回空串）；
//! - 需要 Android/Chaquopy 特有模块（`java`、`android`）的脚本。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

/// `host.py` 嵌入本二进制（改 host.py 后 cargo 会自动重新编译）
const HOST_PY: &str = include_str!("host.py");
/// 单次 Python 调用超时
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// Python 解释器探测超时（防止 Windows Store 别名 / 交互式启动卡住）
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// 子进程 stdout 上限（协议 JSON，超过视为异常）
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
/// 子进程 stderr 保留上限（错误信息用）
const MAX_STDERR_BYTES: usize = 64 * 1024;
/// 远程脚本缓存目录名（位于系统临时目录下）
const CACHE_DIR_NAME: &str = "vbox-py-spider";
/// 远程脚本缓存有效期
const SCRIPT_CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/* ============================================================
 * 站点识别
 * ============================================================ */

/// 站点是否应走 Python spider（供 `sources/spider.rs` 分派）
///
/// 判定顺序：`inline:` 脚本文本特征 → `py_` 前缀 → api/ ext 指向 `.py`。
pub fn is_python_site(site: &Site) -> bool {
    let api = site.api.trim();
    if let Some(code) = api.strip_prefix("inline:") {
        return inline_looks_python(code);
    }
    if api.starts_with("py_") {
        return true;
    }
    if api.to_ascii_lowercase().contains(".py") {
        return true;
    }
    ext_script_hint(site).is_some()
}

/// `inline:` 脚本形态下，用 Python 语法特征和 JS 区分（两者共用 inline: 前缀）
fn inline_looks_python(code: &str) -> bool {
    code.contains("base.spider")
        || code.contains("class Spider")
        || code.contains("def homeContent")
        || code.contains("def categoryContent")
        || code.contains("import base")
}

/// 站点 `ext` 里是否藏着脚本地址（`py_XXX` 站点的脚本来源）
fn ext_script_hint(site: &Site) -> Option<String> {
    if let Some(Json::String(s)) = site.ext.as_ref() {
        let t = s.trim();
        if looks_like_script_location(t) {
            return Some(t.to_string());
        }
    }
    if let Some(obj) = site.ext.as_ref().and_then(|v| v.as_object()) {
        for key in ["url", "api", "script", "spider", "path", "py"] {
            if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
                let t = s.trim();
                if looks_like_script_location(t) {
                    return Some(t.to_string());
                }
            }
        }
    }
    // ext 可能是被序列化过的字符串
    let text = site.ext_string();
    let t = text.trim();
    if t.ends_with(".py") || t.contains(".py?") {
        return Some(t.to_string());
    }
    None
}

fn looks_like_script_location(s: &str) -> bool {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    t.starts_with("inline:")
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("file://")
        || lower.ends_with(".py")
        || lower.contains(".py?")
}

/* ============================================================
 * 对外接口（签名对齐 sources/spider.rs：cfg 在前）
 * ============================================================ */

/// 首页分类：`homeContent(true)` → `{class:[{type_id,type_name}]}`
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let value = invoke_json(cfg, site, "homeContent", json!([true]))?;
    let classes = value
        .get("class")
        .or_else(|| value.get("classes"))
        .and_then(|v| v.as_array())
        .cloned()
        .or_else(|| value.as_array().cloned())
        .ok_or_else(|| {
            CoreError::Config(format!(
                "站点「{}」的 Python homeContent() 未返回 class 数组: {}",
                site.name,
                truncate(&value.to_string(), 200)
            ))
        })?;
    Ok(classes
        .iter()
        .filter_map(|c| {
            let type_id = pick_str(c, &["type_id", "typeId", "id", "tid"])?;
            if type_id.trim().is_empty() {
                return None;
            }
            let type_name = pick_str(c, &["type_name", "typeName", "name"])
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| type_id.clone());
            Some(Category { type_id, type_name })
        })
        .collect())
}

/// 分类列表：`categoryContent(tid, str(pg), true, {})`
pub fn list_videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let value = invoke_json(
        cfg,
        site,
        "categoryContent",
        json!([cat, page.to_string(), true, {}]),
    )?;
    Ok(items_from(&value))
}

/// 详情：`detailContent([id])` → `{list:[{...vod_play_from/vod_play_url}]}`
pub fn get_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let value = invoke_json(cfg, site, "detailContent", json!([[id]]))?;
    let obj = first_video(&value).ok_or_else(|| {
        CoreError::Config(format!(
            "站点「{}」的 Python detailContent() 未返回影片: {}",
            site.name,
            truncate(&value.to_string(), 200)
        ))
    })?;
    Ok(VideoDetail {
        id: pick_str(obj, &["vod_id", "id"]).unwrap_or_else(|| id.to_string()),
        name: pick_str(obj, &["vod_name", "name", "title"]).unwrap_or_default(),
        cover: pick_str(obj, &["vod_pic", "vod_pic_thumb", "pic", "cover"]),
        desc: pick_str(obj, &["vod_content", "vod_blurb", "content", "desc"]),
        year: pick_str(obj, &["vod_year", "year"]),
        area: pick_str(obj, &["vod_area", "area"]),
        director: pick_str(obj, &["vod_director", "director"]),
        actor: pick_str(obj, &["vod_actor", "actor"]),
        episodes: episodes_of(obj),
    })
}

/// 搜索：`searchContent(keyword, quick)`；脚本没有实现时返回空数组（不报错）
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, quick: bool) -> Result<Vec<VideoItem>> {
    match invoke(cfg, site, "searchContent", json!([keyword, quick])) {
        Ok(value) => Ok(items_from(&value)),
        Err(InvokeError::Missing(_)) => Ok(Vec::new()),
        Err(e) => Err(to_core_error(site, e)),
    }
}

/// 播放：`playerContent(flag, id, vipFlags)` → 脚本返回的 JSON（`source.rs` 负责归一）
pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    flag: &str,
    id: &str,
    vip_flags: &[String],
) -> Result<Json> {
    invoke_json(cfg, site, "playerContent", json!([flag, id, vip_flags]))
}

/// 直播：`liveContent(url)` → 订阅文本（txt/m3u）或 JSON 文本
pub fn live_content(cfg: &TvConfig, site: &Site, url: &str) -> Result<String> {
    let value = invoke_json(cfg, site, "liveContent", json!([url]))?;
    Ok(match value {
        Json::String(s) => s,
        other => other.to_string(),
    })
}

/* ============================================================
 * 调用 sidecar
 * ============================================================ */

/// 调用失败的两类原因：脚本没实现方法 / 其它失败
#[derive(Debug)]
enum InvokeError {
    Missing(String),
    Failed(String),
}

fn invoke_json(cfg: &TvConfig, site: &Site, method: &str, args: Json) -> Result<Json> {
    invoke(cfg, site, method, args).map_err(|e| to_core_error(site, e))
}

fn to_core_error(site: &Site, e: InvokeError) -> CoreError {
    match e {
        InvokeError::Missing(method) => CoreError::Config(format!(
            "站点「{}」的 Python spider 未实现 {}() 方法（需要 `class Spider(Spider)` + homeContent/categoryContent 等）",
            site.name, method
        )),
        InvokeError::Failed(msg) => CoreError::Config(format!("站点「{}」: {}", site.name, msg)),
    }
}

fn invoke(
    cfg: &TvConfig,
    site: &Site,
    method: &str,
    args: Json,
) -> std::result::Result<Json, InvokeError> {
    let script = materialize_script(cfg, site).map_err(|e| InvokeError::Failed(e.to_string()))?;

    let outcome = invoke_with_script(site, &script, method, args);
    // inline 临时脚本用完即删；远程缓存 / 本地脚本保留
    if script.cleanup {
        let _ = std::fs::remove_file(&script.path);
    }
    outcome
}

fn invoke_with_script(
    site: &Site,
    script: &ScriptFile,
    method: &str,
    args: Json,
) -> std::result::Result<Json, InvokeError> {
    let python = python_command().map_err(|e| InvokeError::Failed(e.to_string()))?;
    let host = host_py_path().map_err(|e| InvokeError::Failed(e.to_string()))?;

    let request = json!({
        "script": script.path.to_string_lossy(),
        "ext": site.ext_string(),
        "method": method,
        "args": args,
    });

    let reply = run_sidecar(
        &python,
        &host,
        &script.path,
        &request,
        method,
        &script.label,
    )
    .map_err(|e| InvokeError::Failed(e.to_string()))?;

    if reply.ok {
        // 字符串优先按 JSON 解析（同 spider/js.rs 的归一策略）
        Ok(match reply.result {
            Json::String(s) => parse_json_or_string(&s),
            other => other,
        })
    } else if reply.missing {
        Err(InvokeError::Missing(method.to_string()))
    } else {
        let mut detail = String::new();
        if !reply.trace.trim().is_empty() {
            detail = reply.trace.clone();
        } else if !reply.stderr.trim().is_empty() {
            detail = reply.stderr.clone();
        }
        let suffix = if detail.trim().is_empty() {
            String::new()
        } else {
            format!("（{}）", truncate(&detail, 1500))
        };
        Err(InvokeError::Failed(format!(
            "Python spider {method}() 执行失败: {}{}",
            if reply.error.trim().is_empty() {
                "未知错误"
            } else {
                reply.error.trim()
            },
            suffix
        )))
    }
}

/* ============================================================
 * Python 解释器探测
 * ============================================================ */

#[derive(Debug, Clone)]
struct PythonCmd {
    program: String,
    prefix: Vec<String>,
}

impl PythonCmd {
    fn label(&self) -> String {
        if self.prefix.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.prefix.join(" "))
        }
    }

    /// `VBOX_PYTHON` 可能是绝对路径，也可能是 `py -3` 这种带参数的写法
    fn from_spec(spec: &str) -> Self {
        let spec = spec.trim();
        if Path::new(spec).is_file() {
            return Self {
                program: spec.to_string(),
                prefix: Vec::new(),
            };
        }
        let mut parts = spec.split_whitespace();
        Self {
            program: parts.next().unwrap_or("python").to_string(),
            prefix: parts.map(|p| p.to_string()).collect(),
        }
    }
}

fn python_command() -> Result<PythonCmd> {
    let cache = PYTHON_CACHE.get_or_init(|| StdMutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(detect_python());
    }
    match guard.as_ref().unwrap() {
        Ok(cmd) => Ok(cmd.clone()),
        Err(e) => Err(CoreError::Config(e.clone())),
    }
}

static PYTHON_PREFERRED: OnceLock<StdMutex<Option<String>>> = OnceLock::new();
/// 随应用集成的解释器（安装包内置；优先级低于用户设置和环境变量）
static PYTHON_BUNDLED: OnceLock<StdMutex<Option<String>>> = OnceLock::new();
static PYTHON_CACHE: OnceLock<StdMutex<Option<std::result::Result<PythonCmd, String>>>> =
    OnceLock::new();

/// 用户/设置页指定的解释器；None 表示自动探测。
fn preferred_python() -> Option<String> {
    PYTHON_PREFERRED
        .get_or_init(|| StdMutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 随应用集成的解释器（发布包内置目录）。
fn bundled_python() -> Option<String> {
    PYTHON_BUNDLED
        .get_or_init(|| StdMutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 设置随应用集成的解释器（None = 没有内置），并清空探测缓存。
///
/// 与 [`set_preferred`] 分开：设置页「自动探测」清空的是用户指定路径，
/// 不会把安装包内置的解释器一起丢掉。
pub fn set_bundled(spec: Option<&str>) {
    let spec = spec
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    *PYTHON_BUNDLED
        .get_or_init(|| StdMutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = spec;
    python_reset_cache();
}

/// 设置解释器（空字符串/None = 恢复自动探测），并清空探测缓存。
pub fn set_preferred(spec: Option<&str>) {
    let spec = spec
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    *PYTHON_PREFERRED
        .get_or_init(|| StdMutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = spec;
    python_reset_cache();
}

/// 清空解释器探测缓存（设置改变后调用）。
pub fn python_reset_cache() {
    if let Some(cache) = PYTHON_CACHE.get() {
        *cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// 本机是否有可用的 Python 解释器（探测结果进程内缓存）
pub fn python_available() -> bool {
    python_command().is_ok()
}

/// 探测到的解释器描述（供 CLI / 前端展示；无 Python 时为 None）
pub fn python_label() -> Option<String> {
    python_command().ok().map(|c| c.label())
}

/// Python 依赖检查结果（供设置页展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonDependency {
    pub name: String,
    pub module: String,
    pub available: bool,
}

/// Python 运行时状态（供 Tauri `python_status` 命令）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonStatus {
    pub available: bool,
    pub label: Option<String>,
    pub version: Option<String>,
    pub preferred: Option<String>,
    /// 随应用集成的解释器路径（发布包内置时存在）
    pub bundled: Option<String>,
    pub error: Option<String>,
    pub dependencies: Vec<PythonDependency>,
}

/// 返回当前 Python 状态与常用依赖（requests/bs4/lxml/Crypto/pyquery）。
pub fn python_status() -> PythonStatus {
    let preferred = preferred_python();
    match python_command() {
        Ok(cmd) => {
            let version = probe_python(&cmd).ok();
            PythonStatus {
                available: true,
                label: Some(cmd.label()),
                version,
                preferred,
                bundled: bundled_python(),
                error: None,
                dependencies: check_dependencies(&cmd),
            }
        }
        Err(e) => PythonStatus {
            available: false,
            label: None,
            version: None,
            preferred,
            bundled: bundled_python(),
            error: Some(e.to_string()),
            dependencies: Vec::new(),
        },
    }
}

fn check_dependencies(cmd: &PythonCmd) -> Vec<PythonDependency> {
    const SCRIPT: &str = r#"
import importlib.util, json
mods = [("requests","requests"),("bs4","bs4"),("lxml","lxml"),("pycryptodome","Crypto"),("pyquery","pyquery")]
print(json.dumps([{"name": n, "module": m, "available": importlib.util.find_spec(m) is not None} for n, m in mods], ensure_ascii=False))
"#;
    let output = Command::new(&cmd.program)
        .args(&cmd.prefix)
        .args(["-c", SCRIPT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<Vec<PythonDependency>>(text.trim()).unwrap_or_default()
}

fn detect_python() -> std::result::Result<PythonCmd, String> {
    let mut candidates: Vec<PythonCmd> = Vec::new();
    if let Some(value) = preferred_python() {
        candidates.push(PythonCmd::from_spec(&value));
    }
    if let Ok(value) = std::env::var("VBOX_PYTHON") {
        let value = value.trim().to_string();
        if !value.is_empty() {
            candidates.push(PythonCmd::from_spec(&value));
        }
    }
    if let Some(value) = bundled_python() {
        candidates.push(PythonCmd::from_spec(&value));
    }

    // Windows：先扫已知安装目录 / py launcher，再回退 PATH 命令。
    // 原因：winget / 官方安装器装完会写用户 PATH，但**已在运行的 VBox 进程环境变量不会更新**，
    // 直接 `Command::new("python")` 会命中旧 PATH 里的 Microsoft Store 假别名（退出码 9009）。
    #[cfg(windows)]
    {
        for exe in windows_python_exe_candidates() {
            candidates.push(PythonCmd {
                program: exe.to_string_lossy().to_string(),
                prefix: Vec::new(),
            });
        }
        for launcher in windows_py_launcher_candidates() {
            candidates.push(PythonCmd {
                program: launcher.to_string_lossy().to_string(),
                prefix: vec!["-3".to_string()],
            });
        }
    }

    candidates.push(PythonCmd {
        program: "python".to_string(),
        prefix: Vec::new(),
    });
    candidates.push(PythonCmd {
        program: "python3".to_string(),
        prefix: Vec::new(),
    });
    if cfg!(windows) {
        // Windows 官方安装器自带的 py launcher；`python` 往往是 Store 别名占位
        candidates.push(PythonCmd {
            program: "py".to_string(),
            prefix: vec!["-3".to_string()],
        });
    }

    let mut tried: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for candidate in &candidates {
        if !seen.contains(&candidate.label()) {
            seen.push(candidate.label());
        } else {
            continue;
        }
        // Windows：PATH 里的 python/python3 可能是 Store 应用执行别名占位（9009），
        // 解析成真实路径后直接跳过，避免把“假解释器”当成探测失败原因。
        #[cfg(windows)]
        if !candidate.program.contains('\\') && !candidate.program.contains('/') {
            match resolve_on_path(&candidate.program) {
                Some(path) if is_store_alias(&path.to_string_lossy()) => {
                    tried.push(format!(
                        "{}: Microsoft Store 应用执行别名（占位，已跳过）",
                        candidate.label()
                    ));
                    continue;
                }
                Some(path) => {
                    let resolved = PythonCmd {
                        program: path.to_string_lossy().to_string(),
                        prefix: candidate.prefix.clone(),
                    };
                    match probe_python(&resolved) {
                        Ok(version) => {
                            tracing::info!(
                                target: "vbox_core::python",
                                "Python spider 使用解释器: {}（Python {}）",
                                resolved.label(),
                                version
                            );
                            return Ok(resolved);
                        }
                        Err(err) => {
                            tried.push(format!("{}: {}", resolved.label(), err));
                            continue;
                        }
                    }
                }
                None => {
                    tried.push(format!("{}: 未在 PATH 中找到", candidate.label()));
                    continue;
                }
            }
        }
        match probe_python(candidate) {
            Ok(version) => {
                tracing::info!(
                    target: "vbox_core::python",
                    "Python spider 使用解释器: {}（Python {}）",
                    candidate.label(),
                    version
                );
                return Ok(candidate.clone());
            }
            Err(err) => tried.push(format!("{}: {}", candidate.label(), err)),
        }
    }

    Err(format!(
        "未找到可用的 Python 解释器（Python spider 需要本机 CPython 3.8+；可用环境变量 VBOX_PYTHON 指定解释器路径，或在设置页手动填写）。已尝试: {}",
        tried.join("；")
    ))
}

/// 在 PATH 中解析可执行文件（Windows；不做扩展名猜测，只找 .exe / 原样名）。
#[cfg(windows)]
fn resolve_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for suffix in [".exe", ""] {
            let candidate = dir.join(format!("{name}{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Microsoft Store 的 `python.exe` / `python3.exe` 是应用执行别名占位。
#[cfg(windows)]
fn is_store_alias(program: &str) -> bool {
    let normalized = program.replace('/', "\\").to_ascii_lowercase();
    normalized.contains("\\microsoft\\windowsapps\\")
}

/// `Python314` → `(3, 14)`；`Python39` → `(3, 9)`；无法解析返回 None。
#[cfg(windows)]
fn parse_python_dir_version(name: &str) -> Option<(u32, u32)> {
    let digits = name.strip_prefix("Python")?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let major = (digits.as_bytes()[0] - b'0') as u32;
    let minor = digits[1..].parse::<u32>().ok()?;
    (major >= 2).then_some((major, minor))
}

/// Windows 已知安装目录里的 `python.exe`（winget / 官方安装器），按版本从高到低。
#[cfg(windows)]
fn windows_python_exe_candidates() -> Vec<PathBuf> {
    let mut found: Vec<(u32, u32, PathBuf)> = Vec::new();
    let mut scan = |root: PathBuf, prefix: &str| {
        let Ok(entries) = std::fs::read_dir(&root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry
                .file_type()
                .map(|kind| kind.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.starts_with(prefix) {
                continue;
            }
            let Some((major, minor)) = parse_python_dir_version(name) else {
                continue;
            };
            let exe = path.join("python.exe");
            if exe.is_file() {
                found.push((major, minor, exe));
            }
        }
    };

    // winget 默认的用户级安装位置
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        scan(
            PathBuf::from(local).join("Programs").join("Python"),
            "Python3",
        );
    }
    // 系统级安装
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(key) {
            scan(PathBuf::from(root), "Python3");
        }
    }

    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = Vec::new();
    for (_, _, exe) in found {
        if !out.contains(&exe) {
            out.push(exe);
        }
    }
    out
}

/// Windows 的 py launcher（`py -3`）,用户级与系统级位置。
#[cfg(windows)]
fn windows_py_launcher_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let launcher = PathBuf::from(local)
            .join("Programs")
            .join("Python")
            .join("Launcher")
            .join("py.exe");
        if launcher.is_file() {
            out.push(launcher);
        }
    }
    let system = PathBuf::from(r"C:\Windows\py.exe");
    if system.is_file() {
        out.push(system);
    }
    out
}

fn probe_python(cmd: &PythonCmd) -> std::result::Result<String, String> {
    let mut command = Command::new(&cmd.program);
    command
        .args(&cmd.prefix)
        .args(["-c", "import sys;print(sys.version.split()[0])"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("启动失败: {e}"))?;

    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let stderr = read_tail(&mut child, false);
                    return Err(format!("退出码 {:?} {}", status.code(), stderr.trim()));
                }
                let stdout = read_tail(&mut child, true);
                let version = stdout.trim().to_string();
                if version.is_empty() {
                    return Err("无版本输出".to_string());
                }
                return Ok(version);
            }
            Ok(None) => {}
            Err(e) => return Err(format!("等待进程失败: {e}")),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("探测超时".to_string());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 进程已退出后读取残留输出（probe 用；输出很小）
fn read_tail(child: &mut Child, stdout: bool) -> String {
    let mut buf = String::new();
    if stdout {
        if let Some(pipe) = child.stdout.take() {
            let _ = pipe.take(4096).read_to_string(&mut buf);
        }
    } else if let Some(pipe) = child.stderr.take() {
        let _ = pipe.take(4096).read_to_string(&mut buf);
    }
    buf
}

/* ============================================================
 * 脚本来源解析 / 缓存
 * ============================================================ */

struct ScriptFile {
    path: PathBuf,
    /// inline 临时脚本用完即删
    cleanup: bool,
    /// 错误信息里展示的来源
    label: String,
}

fn materialize_script(cfg: &TvConfig, site: &Site) -> Result<ScriptFile> {
    let api = site.api.trim();
    if let Some(code) = api.strip_prefix("inline:") {
        return write_temp_script(code);
    }
    if api.starts_with("http://") || api.starts_with("https://") {
        return download_cached(cfg, site, api);
    }
    if api.starts_with("file://") {
        return Ok(file_script(parse_file_url(api)?));
    }
    if api.starts_with("py_") {
        if let Some(source) = ext_script_hint(site) {
            return materialize_source(cfg, site, &source);
        }
        return Err(CoreError::Config(format!(
            "站点「{}」使用 api=\"{}\" 但没有提供 .py 脚本地址；请在 ext 中给出 http(s)/file:///inline: 脚本",
            site.name, api
        )));
    }
    if api.to_ascii_lowercase().contains(".py") {
        // 配置加载器会把 ./py/x.py 修正成绝对 URL；直接构造 Site 时也允许本地路径
        let path = Path::new(api);
        if path.is_file() {
            return Ok(file_script(path.to_path_buf()));
        }
        if let Some(source) = ext_script_hint(site) {
            return materialize_source(cfg, site, &source);
        }
        return Err(CoreError::Config(format!(
            "站点「{}」的 Python 脚本地址无效（需要 http(s):// / file:// / inline:）: {}",
            site.name, api
        )));
    }
    if let Some(source) = ext_script_hint(site) {
        return materialize_source(cfg, site, &source);
    }
    Err(CoreError::Config(format!(
        "站点「{}」不是可执行的 Python spider（api={}）",
        site.name, api
    )))
}

fn materialize_source(cfg: &TvConfig, site: &Site, source: &str) -> Result<ScriptFile> {
    let source = source.trim();
    if let Some(code) = source.strip_prefix("inline:") {
        return write_temp_script(code);
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        return download_cached(cfg, site, source);
    }
    if source.starts_with("file://") {
        return Ok(file_script(parse_file_url(source)?));
    }
    let path = Path::new(source);
    if path.is_file() {
        return Ok(file_script(path.to_path_buf()));
    }
    Err(CoreError::Config(format!(
        "无法识别的 Python 脚本地址: {source}"
    )))
}

fn parse_file_url(url: &str) -> Result<PathBuf> {
    let parsed = url::Url::parse(url)
        .map_err(|e| CoreError::Config(format!("非法 file:// 脚本地址 {url}: {e}")))?;
    parsed
        .to_file_path()
        .map_err(|_| CoreError::Config(format!("无法解析本地脚本路径: {url}")))
}

fn file_script(path: PathBuf) -> ScriptFile {
    ScriptFile {
        label: format!("file://{}", path.display()),
        path,
        cleanup: false,
    }
}

fn write_temp_script(code: &str) -> Result<ScriptFile> {
    let dir = temp_dir()?;
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("tmp_{}_{}_{}.py", std::process::id(), nanos, seq));
    std::fs::write(&path, code.as_bytes())?;
    Ok(ScriptFile {
        path,
        cleanup: true,
        label: format!("inline({} bytes)", code.len()),
    })
}

fn temp_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(CACHE_DIR_NAME);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 下载远程脚本到临时缓存目录（24h 内复用）
fn download_cached(cfg: &TvConfig, site: &Site, url: &str) -> Result<ScriptFile> {
    let dir = temp_dir()?;
    let path = dir.join(format!("{:x}.py", md5::compute(url.as_bytes())));

    if is_fresh_cache(&path) {
        return Ok(ScriptFile {
            path,
            cleanup: false,
            label: url.to_string(),
        });
    }

    let client = http::client_for(&cfg.http)?;
    let text = http::get_text(&client, url, &site_headers(site), cfg.http.retry)?;
    if invalid_python_script(&text) {
        return Err(CoreError::Config(format!(
            "Python spider 下载到的不是脚本（可能 404/HTML）: {url}"
        )));
    }

    // 原子写：先写唯一临时文件再 rename，避免并发调用读到半个脚本
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".download_{}_{}.tmp", std::process::id(), seq));
    std::fs::write(&tmp, text.as_bytes())?;
    if std::fs::rename(&tmp, &path).is_err() {
        // Windows 上 rename 不覆盖已存在文件
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp, &path)?;
    }

    Ok(ScriptFile {
        path,
        cleanup: false,
        label: url.to_string(),
    })
}

fn is_fresh_cache(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() == 0 {
        return false;
    }
    match meta.modified().ok().and_then(|t| t.elapsed().ok()) {
        Some(age) => age < SCRIPT_CACHE_TTL,
        // 取不到 mtime 时按“命中”处理，避免反复下载
        None => true,
    }
}

/// 明显不是 Python 脚本的响应（HTML 错误页 / 404 文本）
fn invalid_python_script(text: &str) -> bool {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.is_empty() {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    lower.starts_with('<')
        || lower.starts_with("{\"code\":404")
        || lower.starts_with("404")
        || lower.starts_with("not found")
}

fn site_headers(site: &Site) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if let Some(obj) = site.ext_object() {
        for key in ["referer", "cookie", "user-agent", "User-Agent"] {
            if let Some(v) = obj.get(key).and_then(|v| v.as_str()) {
                if !v.trim().is_empty() {
                    out.insert(key.to_string(), v.to_string());
                }
            }
        }
    }
    out
}

/// 把 host.py 写到临时目录（按内容 md5 命名；并发写用 rename 兜底）
fn host_py_path() -> Result<PathBuf> {
    let dir = temp_dir()?;
    let path = dir.join(format!("host_{:x}.py", md5::compute(HOST_PY.as_bytes())));
    if path.is_file() {
        return Ok(path);
    }
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".host_{}_{}.tmp", std::process::id(), seq));
    std::fs::write(&tmp, HOST_PY.as_bytes())?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(path),
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            if path.is_file() {
                Ok(path)
            } else {
                Err(CoreError::Config(format!(
                    "写入 Python sidecar 失败: {}",
                    path.display()
                )))
            }
        }
    }
}

/* ============================================================
 * 子进程执行 / 协议解析
 * ============================================================ */

#[derive(Debug, Default)]
struct HostReply {
    ok: bool,
    missing: bool,
    result: Json,
    error: String,
    trace: String,
    stderr: String,
}

fn run_sidecar(
    python: &PythonCmd,
    host: &Path,
    script: &Path,
    request: &Json,
    method: &str,
    script_label: &str,
) -> Result<HostReply> {
    let workdir = script.parent().unwrap_or_else(|| Path::new("."));

    let mut command = Command::new(&python.program);
    command
        .args(&python.prefix)
        .arg(host)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Python 默认按 locale 编码管道；统一 UTF-8，中文路径/内容不乱码
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1");

    let mut child = command
        .spawn()
        .map_err(|e| CoreError::Config(format!("启动 Python 失败（{}）: {e}", python.label())))?;

    // 写入请求后关闭 stdin（host.py 以 EOF 结束读取）
    if let Some(mut stdin) = child.stdin.take() {
        let payload = serde_json::to_vec(request)
            .map_err(|e| CoreError::Config(format!("序列化 Python sidecar 请求失败: {e}")))?;
        let _ = stdin.write_all(&payload);
        let _ = stdin.flush();
    }

    let stdout_reader = child
        .stdout
        .take()
        .map(|pipe| spawn_reader(pipe, MAX_OUTPUT_BYTES));
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| spawn_reader(pipe, MAX_STDERR_BYTES));

    let deadline = Instant::now() + CALL_TIMEOUT;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CoreError::Config(format!("等待 Python 进程失败: {e}")));
            }
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let (stdout, stdout_truncated) = join_reader(stdout_reader);
    let (stderr, _) = join_reader(stderr_reader);
    let stderr_text = String::from_utf8_lossy(&stderr).trim().to_string();

    if timed_out {
        return Err(CoreError::Config(format!(
            "Python spider 执行超时（>{}s），已终止进程（{method}，脚本 {script_label}）{}{}",
            CALL_TIMEOUT.as_secs(),
            if stderr_text.is_empty() {
                ""
            } else {
                "；stderr: "
            },
            truncate(&stderr_text, 600)
        )));
    }
    if stdout_truncated {
        return Err(CoreError::Config(format!(
            "Python spider stdout 超过 {}MB 上限，已拒绝解析（{method}）",
            MAX_OUTPUT_BYTES / (1024 * 1024)
        )));
    }

    parse_reply(&stdout, &stderr_text, method)
}

fn spawn_reader<R>(mut pipe: R, cap: usize) -> std::thread::JoinHandle<(Vec<u8>, bool)>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut out: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut buf = [0u8; 8192];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if out.len() < cap {
                        let take = (cap - out.len()).min(n);
                        out.extend_from_slice(&buf[..take]);
                    } else {
                        // 继续排空管道，避免子进程写阻塞；内容丢弃
                        truncated = true;
                    }
                }
                Err(_) => break,
            }
        }
        (out, truncated)
    })
}

fn join_reader(handle: Option<std::thread::JoinHandle<(Vec<u8>, bool)>>) -> (Vec<u8>, bool) {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

fn parse_reply(stdout: &[u8], stderr_text: &str, method: &str) -> Result<HostReply> {
    let text = String::from_utf8_lossy(stdout);
    let mut reply_value: Option<Json> = None;
    for line in text.lines().rev() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Json>(line) {
            if value.get("ok").is_some() {
                reply_value = Some(value);
                break;
            }
        }
    }

    let Some(value) = reply_value else {
        // 协议都没有：多半是解释器/脚本在导入阶段直接崩溃
        return Err(CoreError::Config(format!(
            "Python sidecar 未返回有效协议 JSON（{method}）。stdout: {}；stderr: {}",
            if text.trim().is_empty() {
                "<空>".to_string()
            } else {
                truncate(text.trim(), 600)
            },
            if stderr_text.is_empty() {
                "<空>".to_string()
            } else {
                truncate(stderr_text, 1200)
            }
        )));
    };

    Ok(HostReply {
        ok: value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        missing: value
            .get("missing")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        result: value.get("result").cloned().unwrap_or(Json::Null),
        error: value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        trace: value
            .get("trace")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        stderr: stderr_text.to_string(),
    })
}

/* ============================================================
 * JSON 返回解析（与 spider/js.rs 保持一致的宽松策略）
 * ============================================================ */

/// 字符串优先当 JSON 解析；明显不是 JSON 的文本原样保留
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

/// 视图列表：`{list:[...]}` / 根数组 / `{data:{list:[...]}}`
fn items_from(value: &Json) -> Vec<VideoItem> {
    let list = value
        .get("list")
        .and_then(|v| v.as_array())
        .or_else(|| {
            value
                .get("data")
                .and_then(|d| d.get("list"))
                .and_then(|v| v.as_array())
        })
        .or_else(|| value.as_array());
    let Some(list) = list else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|v| {
            let id = pick_str(v, &["vod_id", "id", "video_id"])?;
            if id.trim().is_empty() {
                return None;
            }
            Some(VideoItem {
                id,
                name: pick_str(v, &["vod_name", "name", "title"]).unwrap_or_default(),
                cover: pick_str(v, &["vod_pic", "pic", "cover"]),
                remark: pick_str(v, &["vod_remarks", "remark", "remarks", "note"]),
            })
        })
        .collect()
}

fn first_video(value: &Json) -> Option<&Json> {
    if let Some(list) = value.get("list").and_then(|v| v.as_array()) {
        return list.first();
    }
    if let Some(list) = value
        .get("data")
        .and_then(|d| d.get("list"))
        .and_then(|v| v.as_array())
    {
        return list.first();
    }
    if value.is_object() && pick_str(value, &["vod_id", "id", "vod_name", "name"]).is_some() {
        return Some(value);
    }
    None
}

/// 详情里的多线路选集：优先 `vod_play_from` + `vod_play_url`，其次自定义 `episodes`
fn episodes_of(obj: &Json) -> Vec<EpisodeGroup> {
    let play_from = pick_str(obj, &["vod_play_from", "play_from"]).unwrap_or_default();
    let play_url = pick_str(obj, &["vod_play_url", "play_url"]).unwrap_or_default();
    if !play_url.trim().is_empty() {
        return parse_play_url(&play_from, &play_url);
    }
    parse_episodes_field(obj)
}

/// 解析 TVBox 选集字段：`$$$` 分线路，线路内 `#` 分集、每集 `名称$地址`
fn parse_play_url(play_from: &str, play_url: &str) -> Vec<EpisodeGroup> {
    let bodies: Vec<&str> = play_url.split("$$$").collect();
    let names: Vec<&str> = play_from.split("$$$").collect();

    bodies
        .iter()
        .enumerate()
        .map(|(i, body)| {
            let group = names
                .get(i)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    if bodies.len() == 1 {
                        "默认".to_string()
                    } else {
                        format!("线路{}", i + 1)
                    }
                });
            let items: Vec<Episode> = body
                .split('#')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .enumerate()
                .map(|(idx, raw)| match raw.find('$') {
                    Some(pos) => {
                        let title = raw[..pos].trim();
                        let url = raw[pos + 1..].trim();
                        Episode {
                            title: if title.is_empty() {
                                format!("第{}集", idx + 1)
                            } else {
                                title.to_string()
                            },
                            url: url.to_string(),
                        }
                    }
                    None => Episode {
                        title: format!("第{}集", idx + 1),
                        url: raw.to_string(),
                    },
                })
                .collect();
            EpisodeGroup { group, items }
        })
        .filter(|g| !g.items.is_empty())
        .collect()
}

/// 兼容非标准脚本：`episodes: [{group|name, items|urls: [{title|name, url}]}]`
fn parse_episodes_field(obj: &Json) -> Vec<EpisodeGroup> {
    let Some(groups) = obj.get("episodes").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    groups
        .iter()
        .map(|g| {
            let group =
                pick_str(g, &["group", "name", "flag"]).unwrap_or_else(|| "默认".to_string());
            let list = g
                .get("items")
                .or_else(|| g.get("urls"))
                .or_else(|| g.get("episodes"))
                .and_then(|v| v.as_array());
            let items = list
                .map(|list| {
                    list.iter()
                        .enumerate()
                        .filter_map(|(idx, item)| {
                            let url = pick_str(item, &["url", "play_url", "link"])?;
                            let title = pick_str(item, &["title", "name"])
                                .unwrap_or_else(|| format!("第{}集", idx + 1));
                            Some(Episode { title, url })
                        })
                        .collect()
                })
                .unwrap_or_default();
            EpisodeGroup { group, items }
        })
        .filter(|g| !g.items.is_empty())
        .collect()
}

/// 取第一个存在的字符串字段（数字/布尔也转成字符串）
fn pick_str(obj: &Json, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            match v {
                Json::String(s) => return Some(s.clone()),
                Json::Number(n) => return Some(n.to_string()),
                Json::Bool(b) => return Some(b.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/* ============================================================
 * 纯函数单测（不依赖本机 Python）
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 安装目录名 → 版本解析（Python314 → 3.14，Python39 → 3.9）。
    #[cfg(windows)]
    #[test]
    fn windows_python_dir_version_parsing() {
        assert_eq!(parse_python_dir_version("Python314"), Some((3, 14)));
        assert_eq!(parse_python_dir_version("Python39"), Some((3, 9)));
        assert_eq!(parse_python_dir_version("Python27"), Some((2, 7)));
        assert_eq!(parse_python_dir_version("Python3"), None);
        assert_eq!(parse_python_dir_version("PythonX"), None);
        assert_eq!(parse_python_dir_version("Python"), None);
    }

    /// Microsoft Store 的应用执行别名占位应被识别并跳过。
    #[cfg(windows)]
    #[test]
    fn windows_store_alias_detection() {
        assert!(is_store_alias(
            r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\python.exe"
        ));
        assert!(is_store_alias(
            "c:/users/me/appdata/local/microsoft/windowsapps/python3.exe"
        ));
        assert!(!is_store_alias(
            r"C:\Users\me\AppData\Local\Programs\Python\Python314\python.exe"
        ));
    }

    fn site_with(api: &str, ext: Option<Json>) -> Site {
        Site {
            key: "t".into(),
            name: "测试".into(),
            kind: 3,
            api: api.into(),
            ext,
            ..Default::default()
        }
    }

    #[test]
    fn detect_python_sites() {
        assert!(is_python_site(&site_with("http://x/a.py", None)));
        assert!(is_python_site(&site_with("./py/x.py", None)));
        assert!(is_python_site(&site_with("py_BLZB", None)));
        assert!(is_python_site(&site_with(
            "py_BLZB",
            Some(Json::String("http://x/libvio.py".into()))
        )));
        assert!(is_python_site(&site_with(
            "py_BLZB",
            Some(json!({"url": "https://x/a.py"}))
        )));
        assert!(is_python_site(&site_with(
            "inline:from base.spider import Spider\nclass Spider(Spider):\n    pass",
            None
        )));
        assert!(!is_python_site(&site_with("csp_XYQHiker", None)));
        assert!(!is_python_site(&site_with("https://x/a.js", None)));
        assert!(!is_python_site(&site_with(
            "inline:function home(){return {}}",
            None
        )));
    }

    #[test]
    fn parse_play_url_tvbox_style() {
        let groups = parse_play_url(
            "线路A$$$线路B",
            "第1集$http://a/1.m3u8#第2集$http://a/2.m3u8$$$第1集$http://b/1.mp4",
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group, "线路A");
        assert_eq!(groups[0].items[1].url, "http://a/2.m3u8");
        assert_eq!(groups[1].items[0].url, "http://b/1.mp4");
    }

    #[test]
    fn json_normalization() {
        assert_eq!(parse_json_or_string("{\"a\":1}"), json!({"a": 1}));
        assert_eq!(
            parse_json_or_string("直播,#genre#"),
            Json::String("直播,#genre#".into())
        );
        assert_eq!(parse_json_or_string("123"), Json::String("123".into()));
    }

    #[test]
    fn invalid_script_detection() {
        assert!(invalid_python_script("<html>404</html>"));
        assert!(invalid_python_script("{\"code\":404}"));
        assert!(!invalid_python_script("from base.spider import Spider"));
    }
}
