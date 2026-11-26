//! Node/drpy2 sidecar：把 drpy 规则引擎放到 Node 里跑，Rust 只负责桥接。
//!
//! 目录布局（临时缓存）：
//! ```text
//! <tmp>/vbox-drpy/<hash>/<generation>/
//!   package.json      {"type":"module"}
//!   engine.js         drpy2.min.js
//!   drpy-core-lite.min.js
//!   rule.mjs          ext 规则（自动补 export default rule）
//!   host.cjs          include_str! 的 Node 宿主（内置 cheerio）
//! ```
//! 协议：stdin `{core,engine,rule,method,args}` → stdout `{ok,result}` / `{ok:false,error}`。
//! Node 由用户安装（设置页可指定路径）；不是安全沙箱，只加载可信配置。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value as Json};
use wait_timeout::ChildExt;

use crate::error::{CoreError, Result};
use crate::http;

const HOST_JS: &str = include_str!("host.cjs");
const NODE_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_OUTPUT: usize = 32 * 1024 * 1024;
/// engine / rule 及被 import 依赖的文本上限；超过即显式失败，防止无界读入内存。
const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;

/// drpy 运行根目录覆盖（桌面端设置成应用数据目录下的 `drpy/`）。
static DRPY_ROOT: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

/// 覆盖 drpy 运行缓存根目录；`None` 恢复默认（`VBOX_DRPY_TMP` → `<VBOX_DATA_DIR>/drpy`
/// → 系统临时目录）。桌面端在沙箱/只读 HOME 环境下必须覆盖，否则临时目录不可写。
pub fn set_drpy_root(path: Option<PathBuf>) {
    if let Ok(mut slot) = DRPY_ROOT.get_or_init(|| Mutex::new(None)).lock() {
        *slot = path;
    }
}

fn drpy_root() -> PathBuf {
    if let Ok(slot) = DRPY_ROOT.get_or_init(|| Mutex::new(None)).lock() {
        if let Some(path) = slot.clone() {
            return path;
        }
    }
    if let Ok(custom) = std::env::var("VBOX_DRPY_TMP") {
        let custom = custom.trim();
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    if let Ok(data_dir) = std::env::var("VBOX_DATA_DIR") {
        let data_dir = data_dir.trim();
        if !data_dir.is_empty() {
            return PathBuf::from(data_dir).join("drpy");
        }
    }
    std::env::temp_dir().join("vbox-drpy")
}

#[derive(Debug, Clone)]
struct NodeCmd {
    program: String,
}

impl NodeCmd {
    fn label(&self) -> String {
        self.program.clone()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    pub available: bool,
    pub label: Option<String>,
    pub version: Option<String>,
    pub preferred: Option<String>,
    /// 随应用集成的 Node 路径（发布包内置时存在）
    pub bundled: Option<String>,
    pub error: Option<String>,
}

static PREFERRED: OnceLock<Mutex<Option<String>>> = OnceLock::new();
/// 随应用集成的 Node（安装包内置；优先级低于用户设置和环境变量）
static BUNDLED: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static CACHE: OnceLock<Mutex<Option<std::result::Result<NodeCmd, String>>>> = OnceLock::new();

fn preferred() -> Option<String> {
    PREFERRED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn bundled() -> Option<String> {
    BUNDLED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub fn set_preferred(spec: Option<&str>) {
    let spec = spec
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    *PREFERRED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = spec;
    reset_cache();
}

pub fn reset_cache() {
    if let Some(cache) = CACHE.get() {
        *cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// 设置随应用集成的 Node（None = 没有内置），并清空探测缓存。
///
/// 与 [`set_preferred`] 分开：设置页「自动探测」清空的是用户指定路径，
/// 不会把安装包内置的 Node 一起丢掉。
pub fn set_bundled(spec: Option<&str>) {
    let spec = spec
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    *BUNDLED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = spec;
    reset_cache();
}

fn node_command() -> Result<NodeCmd> {
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(detect_node());
    }
    match guard.as_ref().unwrap() {
        Ok(cmd) => Ok(cmd.clone()),
        Err(e) => Err(CoreError::Config(e.clone())),
    }
}

fn detect_node() -> std::result::Result<NodeCmd, String> {
    let mut candidates = Vec::new();
    if let Some(spec) = preferred() {
        candidates.push(NodeCmd { program: spec });
    }
    if let Ok(spec) = std::env::var("VBOX_NODE") {
        if !spec.trim().is_empty() {
            candidates.push(NodeCmd {
                program: spec.trim().to_string(),
            });
        }
    }
    if let Some(spec) = bundled() {
        candidates.push(NodeCmd { program: spec });
    }
    candidates.push(NodeCmd {
        program: "node".to_string(),
    });
    if cfg!(windows) {
        candidates.push(NodeCmd {
            program: "node.exe".to_string(),
        });
    }
    let mut tried = Vec::new();
    for cmd in candidates {
        match probe_node(&cmd) {
            Ok(version) => {
                tracing::info!(target: "vbox_core::node", "drpy Node 运行时: {}（{}）", cmd.label(), version);
                return Ok(cmd);
            }
            Err(e) => tried.push(format!("{}: {e}", cmd.label())),
        }
    }
    Err(format!(
        "未找到可用的 Node.js（drpy 需要 Node 18+；可用设置页或 VBOX_NODE 指定路径）。已尝试: {}",
        tried.join("；")
    ))
}

fn probe_node(cmd: &NodeCmd) -> std::result::Result<String, String> {
    let output = Command::new(&cmd.program)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .map_err(|e| format!("启动失败: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn node_status() -> NodeStatus {
    match node_command() {
        Ok(cmd) => NodeStatus {
            available: true,
            label: Some(cmd.label()),
            version: probe_node(&cmd).ok(),
            preferred: preferred(),
            bundled: bundled(),
            error: None,
        },
        Err(e) => NodeStatus {
            available: false,
            label: None,
            version: None,
            preferred: preferred(),
            bundled: bundled(),
            error: Some(e.to_string()),
        },
    }
}

/// 识别 drpy 站点：api 含 drpy，或 ext 是 drpy 规则脚本。
pub fn is_drpy_site(api: &str, ext: &str) -> bool {
    let api = api.to_ascii_lowercase();
    if api.contains("drpy") {
        return true;
    }
    let ext = ext.to_ascii_lowercase();
    ext.contains("drpy2.min.js") || ext.contains("/drpy.js") || ext.ends_with("drpy.js")
}

/// 调用 drpy 方法。args 为 JSON 数组，返回值已从字符串 JSON 归一化。
pub fn call(
    cfg: &crate::model::TvConfig,
    api: &str,
    ext: &str,
    method: &str,
    args: &[Json],
) -> Result<Json> {
    let cmd = node_command()?;
    let (dir, core_path) = prepare_dir(cfg, api, ext)?;
    // 部分 drpy2 引擎是自包含的（不依赖 drpy-core-lite），此时 core 传空，宿主会跳过导入。
    let core_value = core_path
        .as_ref()
        .map(|path| json!(path.to_string_lossy()))
        .unwrap_or(Json::Null);
    let request = json!({
        "core": core_value,
        "engine": dir.join("engine.js"),
        "rule": dir.join("rule.mjs"),
        "method": method,
        "args": args,
    });
    let (stdout, stderr) = run_node(&cmd, &dir.join("host.cjs"), &request.to_string())?;
    parse_reply(&stdout, method, &stderr)
}

fn parse_reply(output: &str, method: &str, stderr: &str) -> Result<Json> {
    // 协议行取「最后一个可解析且带布尔 ok 的 JSON 对象」：drpy 引擎/依赖常把
    // 日志以 console.log 打到 stdout，协议行之后仍可能跟 JSON 噪音；只看
    // 最后一个以 '{' 开头的行会误选日志，把成功调用报成协议解析失败。
    for line in output.lines().rev() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Json>(trimmed) else {
            continue;
        };
        let Some(ok) = value.get("ok").and_then(Json::as_bool) else {
            continue;
        };
        if ok {
            return Ok(normalize_result(
                value.get("result").cloned().unwrap_or(Json::Null),
            ));
        }
        return Err(CoreError::Config(format!(
            "drpy {method} 执行失败: {}",
            value
                .get("error")
                .and_then(Json::as_str)
                .unwrap_or("未知错误")
        )));
    }
    let mut detail = format!("stdout 尾部: {}", tail(output));
    if !stderr.trim().is_empty() {
        detail.push_str(&format!("；stderr 尾部: {}", tail(stderr)));
    }
    Err(CoreError::Config(format!(
        "drpy {method} 没有返回协议行（stdout {} 字节）: {detail}",
        output.len()
    )))
}

fn normalize_result(value: Json) -> Json {
    if let Json::String(text) = &value {
        let trimmed = text.trim();
        if (trimmed.starts_with('{') || trimmed.starts_with('['))
            && serde_json::from_str::<Json>(trimmed).is_ok()
        {
            if let Ok(parsed) = serde_json::from_str::<Json>(trimmed) {
                return parsed;
            }
        }
    }
    value
}

fn tail(text: &str) -> String {
    let text = text.trim();
    let chars: String = text
        .chars()
        .rev()
        .take(600)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    chars
}

fn run_node(cmd: &NodeCmd, host: &Path, request: &str) -> Result<(String, String)> {
    let mut child = Command::new(&cmd.program)
        .arg(host)
        .current_dir(host.parent().unwrap_or(Path::new(".")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CoreError::Config(format!("启动 Node 失败: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(request.as_bytes());
    }
    let out_reader = read_pipe(child.stdout.take());
    let err_reader = read_pipe(child.stderr.take());
    let status = match child
        .wait_timeout(NODE_TIMEOUT)
        .map_err(|e| CoreError::Config(format!("等待 Node 失败: {e}")))?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CoreError::Config(format!(
                "drpy Node 执行超时（>{}s）",
                NODE_TIMEOUT.as_secs()
            )));
        }
    };
    let out = out_reader.join().unwrap_or_default();
    let stderr_bytes = err_reader.join().unwrap_or_default();
    if out.truncated {
        return Err(CoreError::Config(format!(
            "drpy Node 输出超过 {} 字节上限，已丢弃后续输出",
            MAX_OUTPUT
        )));
    }
    let stdout = String::from_utf8_lossy(&out.bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes.bytes).into_owned();
    if !status.success() && !stdout.contains("\"ok\":true") {
        return Err(CoreError::Config(format!(
            "drpy Node 退出失败（{}）: {}",
            status,
            tail(&stderr)
        )));
    }
    Ok((stdout, stderr))
}

#[derive(Default)]
struct PipeOut {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_pipe<R: std::io::Read + Send + 'static>(
    pipe: Option<R>,
) -> std::thread::JoinHandle<PipeOut> {
    read_pipe_capped(pipe, MAX_OUTPUT)
}

/// 读取子进程输出，最多保留 `max` 字节。
///
/// 超限后继续把管道读空（避免子进程写满管道阻塞），但不再保存；返回 `truncated`
/// 让调用方明确报错，而不是把半截输出当成完整结果。
fn read_pipe_capped<R: std::io::Read + Send + 'static>(
    pipe: Option<R>,
    max: usize,
) -> std::thread::JoinHandle<PipeOut> {
    std::thread::spawn(move || {
        let mut out = PipeOut::default();
        if let Some(mut pipe) = pipe {
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let room = max.saturating_sub(out.bytes.len());
                        if n <= room {
                            out.bytes.extend_from_slice(&buf[..n]);
                        } else {
                            out.bytes.extend_from_slice(&buf[..room]);
                            out.truncated = true;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        out
    })
}


fn staging_dir(base: &Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    base.join(format!(".staging-{}-{}", std::process::id(), seq))
}

/// 把已写完整的暂存目录原子发布为正式目录。
///
/// 目录级 rename 让读者要么看到旧一代目录、要么看到新一代完整目录，不会读到
/// engine.js 已换而 rule.mjs 还是旧的这种「跨文件撕裂」。若目标代已存在（并发
/// prepare 同一 api/ext），直接复用并清掉本线程暂存目录。
fn publish_dir(staging: &Path, final_dir: &Path) -> Result<PathBuf> {
    match std::fs::rename(staging, final_dir) {
        Ok(()) => Ok(final_dir.to_path_buf()),
        Err(err) => {
            let _ = std::fs::remove_dir_all(staging);
            if final_dir.join("host.cjs").is_file()
                && final_dir.join("engine.js").is_file()
                && final_dir.join("rule.mjs").is_file()
            {
                Ok(final_dir.to_path_buf())
            } else {
                Err(CoreError::Io(err))
            }
        }
    }
}

/// 计算目录内所有文件的排序后内容摘要（相对路径 + 长度 + 字节）。
///
/// 只用于生成不可变「代」目录名：内容相同则摘要相同，任一文件变化都会换名字。
fn content_digest(dir: &Path) -> Result<String> {
    fn collect(base: &Path, cur: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
        for entry in std::fs::read_dir(cur).map_err(CoreError::Io)? {
            let entry = entry.map_err(CoreError::Io)?;
            let path = entry.path();
            if path.is_dir() {
                collect(base, &path, out)?;
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = std::fs::read(&path).map_err(CoreError::Io)?;
                out.push((rel, bytes));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    collect(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut ctx = md5::Context::new();
    for (rel, bytes) in files {
        ctx.consume(rel.as_bytes());
        ctx.consume([0u8]);
        ctx.consume((bytes.len() as u64).to_le_bytes());
        ctx.consume(&bytes);
    }
    Ok(format!("{:x}", ctx.compute()))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 代目录名形如 `<epoch_ms>-<content_digest>`；返回 (时间戳, 摘要)。
fn generation_stamp(name: &str) -> Option<(u128, &str)> {
    let (ts, digest) = name.split_once('-')?;
    if ts.is_empty() || !ts.chars().all(|c| c.is_ascii_digit()) || digest.len() != 32 {
        return None;
    }
    Some((ts.parse().ok()?, digest))
}

fn generation_complete(path: &Path) -> bool {
    path.join("host.cjs").is_file()
        && path.join("engine.js").is_file()
        && path.join("rule.mjs").is_file()
}

/// 查找内容摘要相同、且写完整的既有代，返回时间戳最新的一个。
fn find_complete_generation(base: &Path, digest: &str) -> Option<PathBuf> {
    let mut best: Option<(u128, PathBuf)> = None;
    for entry in std::fs::read_dir(base).ok()? {
        let path = entry.ok()?.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let Some((ts, found)) = generation_stamp(&name) else {
            continue;
        };
        if found != digest || !generation_complete(&path) {
            continue;
        }
        if best.as_ref().is_none_or(|(best_ts, _)| ts > *best_ts) {
            best = Some((ts, path));
        }
    }
    best.map(|(_, path)| path)
}

/// 只保留最近 `keep` 代，并删除超过 `ttl_ms` 的更老代（尽力而为）。
///
/// 保留窗口用于避免删掉仍可能被并发调用读取的目录；删除失败（例如 Windows
/// 上文件仍被占用）直接忽略，下一轮再清。
fn prune_generations(base: &Path, now_ms: u128, keep: usize, ttl_ms: u128) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    let mut gens: Vec<(u128, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if let Some((ts, _)) = generation_stamp(&name) {
            gens.push((ts, path));
        }
    }
    gens.sort_by(|a, b| b.0.cmp(&a.0));
    for (ts, path) in gens.into_iter().skip(keep) {
        if now_ms.saturating_sub(ts) >= ttl_ms {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

fn prepare_dir(
    cfg: &crate::model::TvConfig,
    api: &str,
    ext: &str,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let base = drpy_root().join(format!(
        "{:x}",
        md5::compute(format!("{api}|{ext}").as_bytes())
    ));
    std::fs::create_dir_all(&base).map_err(CoreError::Io)?;

    let engine_text = fetch_text(cfg, api, "drpy engine")?;
    let rule_text = fetch_text(cfg, ext, "drpy rule")?;
    let staging = staging_dir(&base);

    let built = (|| -> Result<Option<String>> {
        std::fs::create_dir_all(&staging).map_err(CoreError::Io)?;
        std::fs::write(staging.join("host.cjs"), HOST_JS).map_err(CoreError::Io)?;
        std::fs::write(staging.join("package.json"), "{\"type\":\"module\"}\n")
            .map_err(CoreError::Io)?;

        // 把 engine 的模块依赖（绝对 URL / 相对路径）下载到本地并改写 import，Node 才能直接加载。
        let mut visited: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let (engine_local, mut core_name) =
            localize_module(&staging, cfg, api, &engine_text, &mut visited, 0)?;
        std::fs::write(staging.join("engine.js"), engine_local).map_err(CoreError::Io)?;

        // engine 里引用了 core 但没被当作可下载依赖识别时，再按固定文件名尝试一次。
        if core_name.is_none() && engine_text.contains("drpy-core-lite") {
            for name in ["drpy-core-lite.min.js", "drpy-core-lite.js"] {
                let core_url = resolve_sibling(api, &format!("./{name}"));
                if let Ok(text) = fetch_text(cfg, &core_url, "drpy core") {
                    std::fs::write(staging.join(name), text).map_err(CoreError::Io)?;
                    core_name = Some(name.to_string());
                    break;
                }
            }
            if core_name.is_none() {
                return Err(CoreError::Config(
                    "drpy 依赖 drpy-core-lite.min.js 未找到；请确认 api 指向网站上的 drpy2.min.js 且同目录有 core"
                        .into(),
                ));
            }
        }

        // 规则脚本也可能 import 远端模块，同样下载并改写
        let (rule_local, rule_core) =
            localize_module(&staging, cfg, ext, &rule_text, &mut visited, 0)?;
        if core_name.is_none() {
            core_name = rule_core;
        }
        std::fs::write(staging.join("rule.mjs"), normalize_rule(&rule_local))
            .map_err(CoreError::Io)?;
        Ok(core_name)
    })();

    let core_name = match built {
        Ok(name) => name,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };
    // 按暂存目录的完整内容（含 engine 依赖、core、rule 重写结果）算内容摘要：
    // 任一被 import 的模块变化都会生成新目录，不会复用旧依赖。
    let digest = content_digest(&staging)?;
    if let Some(existing) = find_complete_generation(&base, &digest) {
        let _ = std::fs::remove_dir_all(&staging);
        let core_path = core_name
            .map(|name| existing.join(name))
            .filter(|path| path.is_file());
        return Ok((existing, core_path));
    }
    let now = now_ms();
    let final_dir = base.join(format!("{now}-{digest}"));
    let dir = publish_dir(&staging, &final_dir)?;
    // 保留最近 8 代，且只清理超过 10 分钟的代，避免与并发调用抢目录。
    prune_generations(&base, now, 8, 10 * 60 * 1000);
    let core_path = core_name
        .map(|name| dir.join(name))
        .filter(|path| path.is_file());
    Ok((dir, core_path))
}

/// 匹配静态 import / 动态 import / require 的模块说明符。
fn import_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r#"(from\s*|import\s*\(\s*|import\s*|require\s*\(\s*)(["'])([^"']+)(["'])"#,
        )
        .expect("drpy import 正则")
    })
}

/// 递归下载模块依赖并改写为本地相对路径；返回改写后的文本与 core 的本地文件名。
fn localize_module(
    dir: &Path,
    cfg: &crate::model::TvConfig,
    base: &str,
    text: &str,
    visited: &mut std::collections::HashMap<String, String>,
    depth: usize,
) -> Result<(String, Option<String>)> {
    const MAX_DEPTH: usize = 8;
    if depth > MAX_DEPTH {
        return Err(CoreError::Config("drpy 依赖层级过深（>8）".into()));
    }

    let mut mapping: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut core_name: Option<String> = None;

    for caps in import_re().captures_iter(text) {
        let spec = caps[3].to_string();
        let is_url = spec.starts_with("http://") || spec.starts_with("https://");
        let is_relative = spec.starts_with("./") || spec.starts_with("../");
        // drpy2 引擎把同目录依赖写成裸文件名（如 `import cheerio from"cheerio.min.js"`），
        // Node 的 ESM 解析器会把它当作 npm 包名而报 ERR_MODULE_NOT_FOUND。
        // 这类「带 js/mjs/cjs 扩展名的裸说明符」应按引擎所在 URL 的同级文件解析。
        let bare_module = !is_url
            && !is_relative
            && {
                let clean = spec.split(['?', '#']).next().unwrap_or(&spec);
                matches!(
                    Path::new(clean).extension().and_then(|e| e.to_str()),
                    Some(ext) if matches!(ext.to_ascii_lowercase().as_str(), "js" | "mjs" | "cjs")
                )
            };
        if !is_url && !is_relative && !bare_module {
            continue; // 真正的裸包名（如 cheerio、node:fs）交给 Node 自身解析
        }
        let target = if is_url {
            spec.clone()
        } else if is_relative {
            resolve_sibling(base, &spec)
        } else {
            resolve_sibling(base, &format!("./{spec}"))
        };
        if target == base {
            continue;
        }
        // 只下载 JS 模块；图片/文本等资源不做本地化
        let path_part = target.split(['?', '#']).next().unwrap_or(&target);
        let is_js_module = match Path::new(path_part).extension().and_then(|e| e.to_str()) {
            Some(ext) => matches!(ext.to_ascii_lowercase().as_str(), "js" | "mjs" | "cjs"),
            None => true,
        };
        if !is_js_module {
            continue;
        }
        let name = if target.contains("drpy-core-lite") {
            core_name = Some("drpy-core-lite.min.js".to_string());
            "drpy-core-lite.min.js".to_string()
        } else if let Some(existing) = visited.get(&target) {
            existing.clone()
        } else {
            let used: std::collections::HashSet<String> = visited.values().cloned().collect();
            local_name(&target, &used)
        };

        if !visited.contains_key(&target) {
            visited.insert(target.clone(), name.clone());
            let dep_text = fetch_text(cfg, &target, "drpy dependency")?;
            let (dep_local, dep_core) =
                localize_module(dir, cfg, &target, &dep_text, visited, depth + 1)?;
            if core_name.is_none() {
                core_name = dep_core;
            }
            std::fs::write(dir.join(&name), dep_local).map_err(CoreError::Io)?;
        }
        mapping.insert(spec, name);
    }

    Ok((rewrite_specifiers(text, &mapping), core_name))
}

/// 依据「说明符 → 本地文件名」映射改写每个 import/require 的路径。
fn rewrite_specifiers(text: &str, mapping: &std::collections::HashMap<String, String>) -> String {
    if mapping.is_empty() {
        return text.to_string();
    }
    import_re()
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let prefix = &caps[1];
            let quote1 = &caps[2];
            let spec = &caps[3];
            let quote2 = &caps[4];
            match mapping.get(spec) {
                Some(local) => format!("{prefix}{quote1}./{local}{quote2}"),
                None => caps[0].to_string(),
            }
        })
        .into_owned()
}

/// 从依赖 URL 推导本地文件名：安全文件名直接使用，否则回退为 URL 哈希。
fn local_name(url: &str, used: &std::collections::HashSet<String>) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let base = path.rsplit('/').next().unwrap_or("");
    let safe = !base.is_empty()
        && base.len() <= 80
        && base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    let candidate = if safe {
        base.to_string()
    } else {
        let hash = format!("{:x}", md5::compute(url));
        format!("dep_{}.js", &hash[..8])
    };
    if used.contains(&candidate) {
        let hash = format!("{:x}", md5::compute(url));
        format!("dep_{}.js", &hash[..8])
    } else {
        candidate
    }
}

fn resolve_sibling(base: &str, relative: &str) -> String {
    if let Ok(url) = url::Url::parse(base) {
        if let Ok(resolved) = url.join(relative) {
            return resolved.to_string();
        }
    }
    let path = Path::new(base);
    match path.parent() {
        Some(parent) => parent
            .join(relative.trim_start_matches("./"))
            .to_string_lossy()
            .into_owned(),
        None => relative.to_string(),
    }
}

fn normalize_rule(text: &str) -> String {
    let text = text.trim_start_matches('\u{feff}');
    if text.contains("export default")
        || text.contains("module.exports")
        || text.contains("export {")
    {
        return text.to_string();
    }
    format!("{text}\n\nexport default (typeof rule !== 'undefined' ? rule : {{}});\n")
}

fn fetch_text(cfg: &crate::model::TvConfig, address: &str, what: &str) -> Result<String> {
    let address = address.trim();
    if address.is_empty() {
        return Err(CoreError::Config(format!("{what} 地址为空")));
    }
    if let Some(path) = address.strip_prefix("inline:") {
        return Ok(path.to_string());
    }
    if address.starts_with("file://") {
        let parsed = url::Url::parse(address)
            .map_err(|e| CoreError::Config(format!("{what} file:// 地址非法: {e}")))?;
        let path = parsed
            .to_file_path()
            .map_err(|_| CoreError::Config(format!("{what} file:// 路径非法: {address}")))?;
        return read_local_source(&path, address, what);
    }
    if address.starts_with("http://") || address.starts_with("https://") {
        let client = http::client_for(&cfg.http)?;
        return http::get_text_limited(
            &client,
            address,
            &std::collections::HashMap::new(),
            cfg.http.retry,
            Some(MAX_SOURCE_BYTES),
        );
    }
    // 裸本地路径
    read_local_source(std::path::Path::new(address), address, what)
}

fn read_local_source(path: &std::path::Path, address: &str, what: &str) -> Result<String> {
    let too_large = || {
        CoreError::Config(format!(
            "{what} 超过 {MAX_SOURCE_BYTES} 字节上限（{address}）"
        ))
    };
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_SOURCE_BYTES {
            return Err(too_large());
        }
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| CoreError::Config(format!("{what} 读取失败（{address}）: {e}")))?;
    // 预检与读取之间文件可能被替换，读后按实际长度再判一次。
    if text.len() as u64 > MAX_SOURCE_BYTES {
        return Err(too_large());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_imports_to_local_paths() {
        let mut mapping = std::collections::HashMap::new();
        mapping.insert("./a.js".to_string(), "a.js".to_string());
        mapping.insert("https://cdn.example.com/lib/b.js".to_string(), "dep_12345678.js".to_string());
        let text = r#"import x from "./a.js";import "https://cdn.example.com/lib/b.js";const c = require('./c.js');"#;
        let out = rewrite_specifiers(text, &mapping);
        assert!(out.contains(r#"from "./a.js""#));
        assert!(out.contains(r#"import "./dep_12345678.js""#));
        assert!(out.contains(r#"require('./c.js')"#), "未映射的保留: {out}");
    }

    #[test]
    fn local_name_falls_back_to_hash_for_unsafe_names() {
        let used = std::collections::HashSet::new();
        assert_eq!(local_name("https://x.com/lib/cheerio.min.js", &used), "cheerio.min.js");
        let chinese = local_name("https://x.com/lib/模板.js", &used);
        assert!(chinese.starts_with("dep_") && chinese.ends_with(".js"), "{chinese}");
        let mut used2 = std::collections::HashSet::new();
        used2.insert("cheerio.min.js".to_string());
        assert!(local_name("https://y.com/cheerio.min.js", &used2).starts_with("dep_"));
    }

    #[test]
    fn parse_reply_ignores_json_logs_after_protocol_line() {
        let out = "{\"ok\":true,\"result\":{\"class\":[]}}\n{\"level\":\"info\",\"msg\":\"done\"}\n";
        let v = parse_reply(out, "home", "").unwrap();
        assert_eq!(v.get("class").and_then(Json::as_array).map(Vec::len), Some(0));
    }

    #[test]
    fn parse_reply_skips_malformed_json_noise() {
        let out = "{\"ok\":true,\"result\":{\"list\":[{\"vod_id\":1}]}}\n{not-json\n";
        let v = parse_reply(out, "category", "").unwrap();
        assert_eq!(v["list"][0]["vod_id"], 1);
    }

    #[test]
    fn parse_reply_surfaces_engine_error_detail() {
        let err = parse_reply("{\"ok\":false,\"error\":\"boom\"}", "detail", "").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("drpy detail 执行失败"), "{text}");
        assert!(text.contains("boom"), "{text}");
    }

    #[test]
    fn parse_reply_normalizes_stringified_result() {
        let v = parse_reply("{\"ok\":true,\"result\":\"{\\\"list\\\":[]}\"}", "search", "").unwrap();
        assert!(v.get("list").and_then(Json::as_array).is_some());
    }

    #[test]
    fn resolves_sibling_against_base_url() {
        assert_eq!(
            resolve_sibling("https://a.com/x/drpy2.min.js", "./drpy-core-lite.min.js"),
            "https://a.com/x/drpy-core-lite.min.js"
        );
        assert_eq!(
            resolve_sibling("https://a.com/x/drpy2.min.js", "lib/cheerio.min.js"),
            "https://a.com/x/lib/cheerio.min.js"
        );
    }
    fn temp_test_dir(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "vbox-node-{name}-{}-{stamp}",
            std::process::id()
        ))
    }

    fn write_complete_generation(dir: &Path, host: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("host.cjs"), host).unwrap();
        std::fs::write(dir.join("engine.js"), "// engine").unwrap();
        std::fs::write(dir.join("rule.mjs"), "export default {}").unwrap();
    }

    #[test]
    fn publish_dir_moves_complete_staging_into_place() {
        let base = temp_test_dir("publish-move");
        let staging = base.join("stage");
        let final_dir = base.join("gen");
        write_complete_generation(&staging, "fresh");
        let got = publish_dir(&staging, &final_dir).unwrap();
        assert_eq!(got, final_dir);
        assert_eq!(
            std::fs::read_to_string(final_dir.join("host.cjs")).unwrap(),
            "fresh"
        );
        assert!(!staging.exists(), "暂存目录应已被 rename 掉");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn publish_dir_reuses_complete_generation_and_drops_staging() {
        let base = temp_test_dir("publish-reuse");
        let staging = base.join("stage");
        let final_dir = base.join("gen");
        write_complete_generation(&final_dir, "existing");
        write_complete_generation(&staging, "stale");
        let got = publish_dir(&staging, &final_dir).unwrap();
        assert_eq!(got, final_dir);
        assert_eq!(
            std::fs::read_to_string(final_dir.join("host.cjs")).unwrap(),
            "existing",
            "并发发布时读者只应看到先到的那一代"
        );
        assert!(!staging.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn publish_dir_refuses_incomplete_existing_generation() {
        let base = temp_test_dir("publish-broken");
        let staging = base.join("stage");
        let final_dir = base.join("gen");
        std::fs::create_dir_all(&final_dir).unwrap();
        std::fs::write(final_dir.join("host.cjs"), "partial").unwrap();
        write_complete_generation(&staging, "fresh");
        let err = publish_dir(&staging, &final_dir).unwrap_err();
        assert!(matches!(err, CoreError::Io(_)), "半成品目标目录必须报错");
        assert!(!staging.exists());
        assert!(!final_dir.join("engine.js").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn read_pipe_capped_keeps_prefix_and_flags_truncation() {
        let reader = std::io::Cursor::new(vec![b'a'; 10]);
        let out = read_pipe_capped(Some(reader), 4).join().unwrap();
        assert_eq!(out.bytes.len(), 4);
        assert_eq!(out.bytes, b"aaaa");
        assert!(out.truncated, "超过上限必须显式标记，不能静默截断");
    }

    #[test]
    fn read_pipe_capped_under_limit_is_not_truncated() {
        let reader = std::io::Cursor::new(b"hello".to_vec());
        let out = read_pipe_capped(Some(reader), 16).join().unwrap();
        assert_eq!(out.bytes, b"hello");
        assert!(!out.truncated);
    }

    #[test]
    fn content_digest_ignores_creation_order() {
        let base = temp_test_dir("digest-order");
        let a = base.join("a");
        let b = base.join("b");
        std::fs::create_dir_all(a.join("lib")).unwrap();
        std::fs::create_dir_all(b.join("lib")).unwrap();
        std::fs::write(a.join("engine.js"), "engine").unwrap();
        std::fs::write(b.join("lib/dep.js"), "dep").unwrap();
        std::fs::write(a.join("lib/dep.js"), "dep").unwrap();
        std::fs::write(b.join("engine.js"), "engine").unwrap();
        assert_eq!(content_digest(&a).unwrap(), content_digest(&b).unwrap());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn content_digest_changes_when_dependency_bytes_change() {
        let base = temp_test_dir("digest-bytes");
        let a = base.join("a");
        let b = base.join("b");
        for dir in [&a, &b] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("engine.js"), "same engine").unwrap();
        }
        std::fs::write(a.join("dep.js"), "v1").unwrap();
        std::fs::write(b.join("dep.js"), "v2").unwrap();
        assert_ne!(content_digest(&a).unwrap(), content_digest(&b).unwrap());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn content_digest_changes_when_relative_path_changes() {
        let base = temp_test_dir("digest-path");
        let a = base.join("a");
        let b = base.join("b");
        for dir in [&a, &b] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("engine.js"), "engine").unwrap();
        }
        std::fs::write(a.join("dep.js"), "dep").unwrap();
        std::fs::create_dir_all(b.join("lib")).unwrap();
        std::fs::write(b.join("lib/dep.js"), "dep").unwrap();
        assert_ne!(content_digest(&a).unwrap(), content_digest(&b).unwrap());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn parse_reply_reports_stderr_when_protocol_missing() {
        let err = parse_reply("noise\n", "detail", "SyntaxError: boom").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("stderr 尾部"), "{text}");
        assert!(text.contains("SyntaxError: boom"), "{text}");
        assert!(text.contains("stdout 6 字节"), "{text}");
    }

    #[test]
    fn fetch_text_local_failure_names_resource_and_address() {
        let cfg = crate::model::TvConfig::default();
        let err = fetch_text(&cfg, "file:///Z:/no/such/drpy-rule.js", "drpy rule").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("drpy rule 读取失败"), "{text}");
        assert!(text.contains("drpy-rule.js"), "{text}");
        let err = fetch_text(&cfg, "Z:/no/such/engine.js", "drpy engine").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("drpy engine 读取失败"), "{text}");
        assert!(text.contains("Z:/no/such/engine.js"), "{text}");
    }

    #[test]
    fn generation_stamp_requires_timestamp_and_digest() {
        let digest = "a".repeat(32);
        assert_eq!(
            generation_stamp(&format!("12345-{digest}")),
            Some((12345, digest.as_str()))
        );
        assert!(generation_stamp("12345-nothex").is_none());
        assert!(generation_stamp("no-stamp").is_none());
        assert!(generation_stamp(&format!("x-{digest}")).is_none());
    }

    #[test]
    fn find_complete_generation_prefers_newest_matching_digest() {
        let base = temp_test_dir("find-generation");
        let digest = "b".repeat(32);
        let other = "c".repeat(32);
        write_complete_generation(&base.join(format!("100-{digest}")), "old");
        write_complete_generation(&base.join(format!("200-{digest}")), "new");
        write_complete_generation(&base.join(format!("300-{other}")), "other");
        // 半成品即使时间戳更新也不能被复用
        std::fs::create_dir_all(base.join(format!("400-{digest}"))).unwrap();
        let found = find_complete_generation(&base, &digest).unwrap();
        assert_eq!(found, base.join(format!("200-{digest}")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prune_generations_keeps_newest_and_removes_old() {
        let base = temp_test_dir("prune-keep");
        let digest = "d".repeat(32);
        for ts in [1_000u128, 2_000, 3_000] {
            write_complete_generation(&base.join(format!("{ts}-{digest}")), "x");
        }
        prune_generations(&base, 100_000, 2, 1_000);
        assert!(!base.join(format!("1000-{digest}")).exists());
        assert!(base.join(format!("2000-{digest}")).exists());
        assert!(base.join(format!("3000-{digest}")).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prune_generations_respects_ttl_for_recent_dirs() {
        let base = temp_test_dir("prune-ttl");
        let digest = "e".repeat(32);
        for ts in [95_000u128, 96_000, 99_000] {
            write_complete_generation(&base.join(format!("{ts}-{digest}")), "x");
        }
        // 即使超出 keep，只要尚未超过 TTL 就不删（可能仍被并发调用使用）
        prune_generations(&base, 100_000, 1, 10_000);
        for ts in [95_000u128, 96_000, 99_000] {
            assert!(base.join(format!("{ts}-{digest}")).exists(), "{ts}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn oversized_local_source_is_rejected_with_limit() {
        let dir = temp_test_dir("oversized-local");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("engine.js");
        // 超过 8 MiB 上限 1 字节；内容为合法 UTF-8。
        std::fs::write(&path, "a".repeat(MAX_SOURCE_BYTES as usize + 1)).unwrap();
        let cfg = crate::model::TvConfig::default();
        let address = format!("file:///{}", path.to_string_lossy().replace('\\', "/"));
        let err = fetch_text(&cfg, &address, "drpy engine").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("drpy engine 超过"), "{text}");
        assert!(text.contains("上限"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_http_source_is_rejected_with_limit() {
        use std::io::{Read, Write};
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            _ => panic!("需要 IP 监听地址"),
        };
        let handle = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            let body = vec![b'a'; MAX_SOURCE_BYTES as usize + 1];
            let response = tiny_http::Response::from_data(body);
            let _ = request.respond(response);
        });
        let mut cfg = crate::model::TvConfig::default();
        cfg.http.retry = 0;
        let url = format!("http://127.0.0.1:{port}/engine.js");
        let err = fetch_text(&cfg, &url, "drpy engine").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("超过"), "{text}");
        assert!(text.contains("上限"), "{text}");
        // 服务端只应收到一次请求；超限时客户端在读完 limit+1 后立即返回。
        handle.join().unwrap();
    }

}
