//! JVM 车道：用真 JRE 17 + 兼容层跑原始 jar。
//!
//! 位置：`sources/spider.rs` 五个 `*_dispatch` 的**开头**（JVM 主力，失败回退原生）。
//!
//! 进程模型（P2）：`vbox_jvm::Pool<HostProcess>` 按 `(jar 路径, ext hash)` 分片，
//! 每个分片是一个 `SpiderHost --serve` 常驻进程；宿主内部 24 线程池串行化到
//! 「同类+ext 最多 2 实例」。播放期用租约（`Lease`）把宿主钉住，避免取流时被回收。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::contract::{CatvodError, ErrorClass};
use super::loader::{JarStore, LoadedJar};
use super::{normalize, registry};
use crate::model::TvConfig;
use crate::model::{Category, Site, VideoDetail, VideoItem};
use vbox_jvm::{
    Call, HostOptions, HostProcess, JvmError, Lease, Pool, PoolConfig, PoolKey, PoolSpawnError,
    PoolStats, Runtime, Slot,
};

/// 同时存活的宿主上限（每个约 60–120 MB 常驻）。
const DEFAULT_MAX_HOSTS: usize = 8;
/// 空闲宿主回收阈值。
const DEFAULT_IDLE_SECS: u64 = 600;
/// 浏览类调用上限。
const BROWSE_TIMEOUT: Duration = Duration::from_secs(30);
/// 播放类调用上限（蜘蛛要抓取真实播放地址，慢站常见）。
const PLAY_TIMEOUT: Duration = Duration::from_secs(120);
/// 播放会话空闲回收阈值。
const SESSION_IDLE: Duration = Duration::from_secs(15 * 60);

/// JVM 车道的执行优先级。
///
/// - `Primary`（默认）：带 jar 的站点**先**走真 JRE；失败才回退已移植家族/脚本/分类。
/// - `Fallback`：只在其它路径都失败后才走 JVM（旧行为，保留用于排查与回滚）。
/// - `Off`：完全关闭（不加载 JRE，不做任何尝试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Primary,
    Fallback,
    Off,
}

/// 从 `VBOX_JVM_MODE` 读执行模式（`primary`/`fallback`/`off`，大小写不敏感）。
pub fn mode() -> Mode {
    match std::env::var("VBOX_JVM_MODE") {
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "fallback" => Mode::Fallback,
            "off" | "0" | "false" => Mode::Off,
            _ => Mode::Primary,
        },
        Err(_) => Mode::Primary,
    }
}

/// 少数 api 需要「原生优先」时的逃生门：`VBOX_JVM_NATIVE_FIRST=csp_A,csp_B`。
fn native_first(api: &str) -> bool {
    std::env::var("VBOX_JVM_NATIVE_FIRST")
        .map(|v| v.split(',').any(|item| item.trim() == api.trim()))
        .unwrap_or(false)
}

/// 该站点是否应该由 JVM 车道**先**执行（主力模式 + 带 jar + 未被显式排除）。
pub fn should_lead(site: &Site) -> bool {
    mode() == Mode::Primary && !native_first(site.api.trim()) && can_handle(site)
}

pub fn available() -> bool {
    Runtime::locate().is_some()
}

/// 该站点能否交给 JVM 车道：有 jar、能解析出入口类、且不是加固壳。
pub fn can_handle(site: &Site) -> bool {
    !site.jar.trim().is_empty() && class_of(site).is_some()
}

/// 站点入口类：优先取画像里的 `class`；画像缺失时按 TVBox 约定
/// `csp_Foo` → `com.github.catvod.spider.Foo` 兜底（只有 jar 已给出才会走到这里，
/// 因此类名猜错只会得到一条 `catvod.host.class_not_found`，比笼统 site_error 更有用）。
fn class_of(site: &Site) -> Option<String> {
    let api = site.api.trim();
    if let Some(profile) = registry::lookup(api) {
        if profile.guarded {
            // 加固壳默认不接管；启用 payload 供给（VBOX_JVM_SHELL=1）后才把 class 交出去，
            // 由 host_for 决定有没有可用载荷。
            if std::env::var("VBOX_JVM_SHELL").ok().as_deref() != Some("1") {
                return None;
            }
        }
        if let Some(class) = profile.class.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            return Some(class.to_string());
        }
    }
    let suffix = api.strip_prefix("csp_")?;
    if suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(format!("com.github.catvod.spider.{suffix}"))
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// 全局宿主池（按 `(jar 路径, ext hash)` 分片）。
fn host_pool() -> &'static Arc<Pool<HostProcess>> {
    static POOL: OnceLock<Arc<Pool<HostProcess>>> = OnceLock::new();
    POOL.get_or_init(|| {
        Pool::new(PoolConfig {
            max_hosts: env_usize("VBOX_JVM_MAX_HOSTS", DEFAULT_MAX_HOSTS),
            idle_ttl: Duration::from_secs(env_u64("VBOX_JVM_IDLE_SECS", DEFAULT_IDLE_SECS)),
            sweep_every: Duration::from_secs(30),
        })
    })
}

fn map_err(site: &Site, err: JvmError) -> CatvodError {
    // 缺类诊断：class_not_found 的类名记入全局清单，设置页/诊断命令汇总展示。
    if let JvmError::Spider { code, detail } = &err {
        vbox_jvm::note_missing_class(code, detail);
    }
    let class = match &err {
        JvmError::Spider { code, .. } => match code.as_str() {
            "class_not_found" => ErrorClass::JvmClassNotFound,
            "init_failed" => ErrorClass::JvmInitFailed,
            "method_missing" => ErrorClass::JvmMethodMissing,
            "bad_args" => ErrorClass::JvmBadArgs,
            "crashed" => ErrorClass::JvmCrashed,
            _ => ErrorClass::JvmSpiderThrew,
        },
        JvmError::DexConvert { .. } => ErrorClass::JvmDex2Jar,
        JvmError::MissingRuntime { .. } | JvmError::Io(_) | JvmError::BadManifest { .. } => {
            ErrorClass::JvmUnavailable
        }
        JvmError::Timeout { .. } | JvmError::Exit { .. } | JvmError::Protocol { .. } => {
            ErrorClass::JvmCrashed
        }
        JvmError::Json(_) => ErrorClass::Parse,
    };
    // 加固壳 / native 依赖的统一判定：只要错误链里出现 UnsatisfiedLinkError，
    // 就说明这条站点路径需要执行 native（ARM so、System.load、native 方法），
    // 纯 JVM 车道无法满足 —— 统一归到 catvod.host.shell_unsupported，
    // 而不是笼统的「蜘蛛抛异常」，用户才知道该换源或走原生。
    let detail = err.to_string();
    let class = if detail.contains("UnsatisfiedLinkError")
        || (class == ErrorClass::JvmInitFailed
            && (detail.contains("machine code=0x28") || detail.contains("machine code=0x34")))
    {
        ErrorClass::JvmShellUnsupported
    } else {
        class
    };
    // 兼容性遥测（L1）：记错误码 + 脱敏后的详情开头，用于统计「新壳卡在哪一步」；本机 spool，不上传。
    vbox_jvm::telemetry::note_failure(class.code(), &detail);
    CatvodError::new(class, site, detail)
}

/// jar 缓存根目录：`VBOX_JAR_CACHE` → 平台缓存目录 → 系统临时目录。
///
/// P2 接 Tauri 时改由 `app_data_dir()` 注入（计划 §6.6 缓存布局）。
pub fn default_jar_root() -> PathBuf {
    if let Ok(v) = std::env::var("VBOX_JAR_CACHE") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("VBox"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Caches/VBox"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")))
            .map(|p| p.join("vbox"))
    };
    base.unwrap_or_else(|| std::env::temp_dir().join("vbox"))
}

fn jar_store(_cfg: &TvConfig) -> JarStore {
    JarStore::new(default_jar_root().join("jars"))
}

fn load_jar(cfg: &TvConfig, site: &Site) -> Result<LoadedJar, CatvodError> {
    let store = jar_store(cfg);
    let client = crate::http::client_for(&cfg.http)
        .map_err(|e| CatvodError::new(ErrorClass::Network, site, e.to_string()))?;
    store
        .load(&site.jar, site, &client)
        .map_err(|e| CatvodError::new(ErrorClass::JarFetch, site, e.to_string()))
}

/// 浏览类调用的超时：站点 timeout（0 视为默认 15s）再加 5s 余量，上限 30s。
fn browse_timeout(site: &Site) -> Duration {
    let secs = if site.timeout == 0 { 15 } else { site.timeout.min(25) };
    Duration::from_secs(secs as u64 + 5).min(BROWSE_TIMEOUT)
}

/// 拉起一个新的常驻宿主并预热目标类。
/// 加固壳载荷：按 jar 的 sha256 在 `<dex 缓存>/payloads/<sha>/index.json` 里找。
fn payload_shims(jar: &Path, class: &str) -> Vec<PathBuf> {
    let cache = vbox_jvm::DexOptions::default_cache();
    let Ok(sha) = vbox_jvm::sha256_file(jar) else { return Vec::new() };
    vbox_jvm::resolve_payload(&cache, &sha, Some(class))
        .map(|p| p.paths)
        .unwrap_or_default()
}

fn spawn_host(jar: &Path, class: &str, ext: &str) -> Result<HostProcess, JvmError> {
    let rt = Runtime::locate().ok_or_else(|| JvmError::MissingRuntime {
        path: PathBuf::from("<resources>/jvm"),
    })?;
    let mut opts = HostOptions::new(vec![jar.to_path_buf()]);
    // 诊断/调优逃生门：VBOX_JVM_HEAP_MB 改堆上限；VBOX_JVM_ARGS 追加 JVM 参数（空格分隔）。
    if let Ok(v) = std::env::var("VBOX_JVM_HEAP_MB") {
        if let Ok(mb) = v.trim().parse::<u32>() {
            if (128..=2048).contains(&mb) {
                opts.heap_mb = mb;
            }
        }
    }
    if let Ok(v) = std::env::var("VBOX_JVM_ARGS") {
        opts.extra_jvm_args.extend(v.split_whitespace().map(|s| s.to_string()));
    }
    let shims = payload_shims(jar, class);
    if !shims.is_empty() {
        tracing::info!(jar = jar.display().to_string(), shims = shims.len(), "加固壳载荷已挂载");
        opts = opts.with_shims(shims);
    }
    let mut process = HostProcess::spawn(&rt, &opts)?;
    process.handshake(Duration::from_secs(20))?;
    process.warm(class, ext, Duration::from_secs(30))?;
    Ok(process)
}

/// 从池里取（或新建）该站点的宿主，并返回一个租约（Drop 时归还）。
fn host_for(
    cfg: &TvConfig,
    site: &Site,
    class: &str,
) -> Result<(Arc<Slot<HostProcess>>, Lease<HostProcess>, PoolKey), CatvodError> {
    let loaded = load_jar(cfg, site)?;
    if registry::lookup(site.api.trim()).map(|p| p.guarded).unwrap_or(false)
        && payload_shims(&loaded.path, class).is_empty()
    {
        return Err(CatvodError::new(
            ErrorClass::JvmShellUnsupported,
            site,
            format!(
                "加固壳载荷缺失：{}；{}",
                loaded.path.display(),
                vbox_jvm::unsupported_hint("<jar sha256>")
            ),
        ));
    }
    let ext = site.ext_string();
    let key = PoolKey::new(loaded.path.display().to_string(), &ext);
    let pool = host_pool();

    let mut spawn_err: Option<JvmError> = None;
    let slot = pool
        .get_or_spawn(key.clone(), || match spawn_host(&loaded.path, class, &ext) {
            Ok(process) => Ok(process),
            Err(err) => {
                spawn_err = Some(err);
                Err(())
            }
        })
        .map_err(|err| match err {
            PoolSpawnError::Full { max } => CatvodError::new(
                ErrorClass::JvmCrashed,
                site,
                format!("JVM 宿主池已满（上限 {max} 个，正在浏览/播放）"),
            ),
            PoolSpawnError::Spawn(()) => map_err(
                site,
                spawn_err.take().unwrap_or_else(|| JvmError::Protocol {
                    line: "spawn host".to_string(),
                }),
            ),
        })?;

    let lease = pool.lease(&slot);
    Ok((slot, lease, key))
}

/// 一次调用（含崩溃重试一次）。
fn invoke(cfg: &TvConfig, site: &Site, call: &Call, play: bool) -> Result<Value, CatvodError> {
    let class = class_of(site)
        .ok_or_else(|| CatvodError::new(ErrorClass::JvmClassNotFound, site, "画像里没有 class"))?;
    let timeout = if play { PLAY_TIMEOUT } else { browse_timeout(site) };

    let mut last: Option<JvmError> = None;
    for attempt in 0..2 {
        let (slot, lease, key) = host_for(cfg, site, &class)?;
        let outcome = slot.with(|process| process.call(call, timeout));
        match outcome {
            Ok(reply) => {
                host_pool().record_call(reply.ok);
                // 加固壳首次解密成功后：把 Java 侧导出的内层载荷自动安装成 payload，
                // 并淘汰这只「还在走原生桥」的宿主 —— 下一次请求会以 shim 快路径重建。
                if !play && slot.with(|process| process.harvest_payload()).is_some() {
                    drop(lease);
                    host_pool().evict(&key);
                    return reply.data_json().map_err(|e| map_err(site, e));
                }
                if std::env::var("VBOX_JVM_TRACE").is_ok() {
                    let head: String = reply.data.chars().take(300).collect();
                    eprintln!(
                        "[jvm] {} {} ok={} pp={:?} data[{}]={}",
                        site.key,
                        call.method,
                        reply.ok,
                        reply.pp,
                        reply.data.len(),
                        head
                    );
                }
                return reply.data_json().map_err(|e| map_err(site, e));
            }
            Err(err) => {
                host_pool().record_call(false);
                let retryable = matches!(
                    err,
                    JvmError::Timeout { .. } | JvmError::Exit { .. } | JvmError::Protocol { .. }
                );
                // 先归还租约再淘汰，避免把正在使用的宿主拔掉。
                drop(lease);
                if attempt == 0 && retryable {
                    host_pool().evict(&key);
                    last = Some(err);
                    continue;
                }
                return Err(map_err(site, err));
            }
        }
    }
    Err(map_err(
        site,
        last.unwrap_or_else(|| JvmError::Protocol {
            line: "retry exhausted".to_string(),
        }),
    ))
}

// ---- 播放会话：宿主必须活到播放器停止，靠租约钉住 ----

struct Session {
    _lease: Lease<HostProcess>,
    touched: Instant,
    /// 宿主自报的本地代理端口（`-Dtvbox.proxy.port` 生效时才有）。
    pp: Option<u16>,
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn session_key(site: &Site, id: &str) -> String {
    format!("{}#{}", site.key, id)
}

/// 停掉一个播放会话（切源/停止时调用）。
pub fn release_session(site: &Site, id: &str) -> bool {
    sessions()
        .lock()
        .map(|mut map| map.remove(&session_key(site, id)).is_some())
        .unwrap_or(false)
}

/// 停掉某站点的全部会话。
pub fn release_site_sessions(site_key: &str) -> usize {
    let Ok(mut map) = sessions().lock() else { return 0 };
    let prefix = format!("{site_key}#");
    let keys: Vec<String> = map.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
    for key in &keys {
        map.remove(key);
    }
    keys.len()
}

/// 回收空闲超过 `max_idle` 的会话。
pub fn reap_sessions(max_idle: Duration) -> usize {
    let Ok(mut map) = sessions().lock() else { return 0 };
    let now = Instant::now();
    let keys: Vec<String> = map
        .iter()
        .filter(|(_, s)| now.duration_since(s.touched) >= max_idle)
        .map(|(k, _)| k.clone())
        .collect();
    for key in &keys {
        map.remove(key);
    }
    keys.len()
}

/// 当前播放会话数（诊断）。
pub fn session_count() -> usize {
    sessions().lock().map(|m| m.len()).unwrap_or(0)
}

// ---- 五个入口：签名与 `sources/spider.rs` 的 `*_dispatch` 对齐 ----

pub fn list_categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>, CatvodError> {
    let class = class_of(site).unwrap_or_default();
    let ext = site.ext_string();
    let call = Call::home_content(class, &ext);
    let value = invoke(cfg, site, &call, false)?;
    let cats = normalize::categories(&value);
    if std::env::var("VBOX_JVM_TRACE").is_ok() {
        eprintln!("[jvm] categories(len={})", cats.len());
    }
    Ok(cats)
}

pub fn list_videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>, CatvodError> {
    let class = class_of(site).unwrap_or_default();
    let ext = site.ext_string();
    let call = Call::category_content(class, &ext, cat, &page.to_string(), "");
    let value = invoke(cfg, site, &call, false)?;
    Ok(normalize::videos(&value))
}

pub fn get_detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail, CatvodError> {
    let class = class_of(site).unwrap_or_default();
    let ext = site.ext_string();
    let ids = vec![id.to_string()];
    let call = Call::detail_content(class, &ext, &ids);
    let value = invoke(cfg, site, &call, false)?;
    normalize::detail(&value).map_err(|e| CatvodError::new(ErrorClass::Parse, site, e.to_string()))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str, quick: bool) -> Result<Vec<VideoItem>, CatvodError> {
    let class = class_of(site).unwrap_or_default();
    let ext = site.ext_string();
    let call = Call::search_content(class, &ext, keyword, if quick { "1" } else { "" });
    let value = invoke(cfg, site, &call, false)?;
    Ok(normalize::videos(&value))
}

/// 播放：返回原始 catvod JSON，并把宿主用租约钉住（播放器直连宿主的本地代理端口）。
pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    flag: &str,
    id: &str,
    vip_flags: &[String],
) -> Result<Value, CatvodError> {
    let class = class_of(site).unwrap_or_default();
    let ext = site.ext_string();
    let call = Call::player_content(class, &ext, flag, id, vip_flags);

    // 换源/重播：先把该站点的旧会话释放掉，免得宿主被历史会话钉住。
    release_site_sessions(&site.key);
    reap_sessions(SESSION_IDLE);

    // 播放要拿住租约，不能像浏览那样用完就还。
    let (slot, lease, _key) = host_for(cfg, site, &class_of(site).unwrap_or_default())?;
    let reply = slot
        .with(|process| process.call(&call, PLAY_TIMEOUT))
        .map_err(|e| {
            host_pool().record_call(false);
            map_err(site, e)
        })?;
    host_pool().record_call(reply.ok);
    let pp = reply.pp;
    let value = reply.data_json().map_err(|e| map_err(site, e))?;
    if let Ok(mut map) = sessions().lock() {
        map.insert(
            session_key(site, id),
            Session {
                _lease: lease,
                touched: Instant::now(),
                pp,
            },
        );
    }
    Ok(value)
}

/// 播放会话的代理端口（端口自报，供播放器地址改写/诊断）。
pub fn session_proxy_port(site: &Site, id: &str) -> Option<u16> {
    sessions().lock().ok()?.get(&session_key(site, id))?.pp
}

/// 当前池统计（诊断面板）。
pub fn stats() -> PoolStats {
    host_pool().stats()
}

/// 当前存活宿主数（调试/日志用）。
pub fn live_hosts() -> usize {
    stats().hosts
}

/// 关闭所有宿主与会话（应用退出时调用）。
pub fn shutdown_all() {
    if let Ok(mut map) = sessions().lock() {
        map.clear();
    }
    host_pool().shutdown_all();
}

/// 诊断：当前 jar 是否已缓存、宿主是否存活（`vbox_cli jvm-status` 用）。
pub fn describe(cfg: &TvConfig, site: &Site) -> Value {
    let class = class_of(site).unwrap_or_default();
    let mut out = serde_json::json!({
        "available": available(),
        "can_handle": can_handle(site),
        "class": class,
        "jar": site.jar,
        "mode": format!("{:?}", mode()),
        "pool": {
            "hosts": live_hosts(),
            "sessions": session_count(),
        },
    });
    if can_handle(site) {
        if let Ok(loaded) = load_jar(cfg, site) {
            let key = PoolKey::new(loaded.path.display().to_string(), &site.ext_string());
            out["cached"] = Value::Bool(true);
            out["path"] = Value::String(loaded.path.display().to_string());
            out["key"] = Value::String(format!("{}#{}", key.jar, key.ext_hash));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局清单是进程级单例，两个断言必须放在同一个测试里，否则并行测试会互相污染。
    #[test]
    fn diag_records_only_class_not_found() {
        vbox_jvm::diag::reset();
        let site = Site::default();

        let missing = JvmError::Spider {
            code: "class_not_found".to_string(),
            detail: "java.lang.ClassNotFoundException: android.util.SomeMissingThing".to_string(),
        };
        let mapped = map_err(&site, missing);
        assert_eq!(mapped.class, ErrorClass::JvmClassNotFound);

        let other = JvmError::Spider {
            code: "spider_threw".to_string(),
            detail: "java.lang.ClassNotFoundException: ignored.Cls".to_string(),
        };
        let _ = map_err(&site, other);

        let rows = vbox_jvm::missing_classes();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].class, "android.util.SomeMissingThing");
        assert_eq!(rows[0].hits, 1);
        vbox_jvm::diag::reset();
    }

    /// 加固壳的 shim 失败（native 方法缺实现、System.load 解不了加密 so）统一归 shell_unsupported。
    #[test]
    fn unsatisfied_link_maps_to_shell_unsupported() {
        let site = Site::default();
        let cases = [
            (
                "spider_threw",
                "java.lang.UnsatisfiedLinkError: 'byte[] com.github.catvod.spider.MyCrypto.extDe(java.lang.String)'",
            ),
            (
                "init_failed",
                "java.lang.ExceptionInInitializerError: java.lang.UnsatisfiedLinkError: ...\\.aowuc: Can't load this .dll (machine code=0x34)",
            ),
        ];
        for (code, detail) in cases {
            let err = JvmError::Spider {
                code: code.to_string(),
                detail: detail.to_string(),
            };
            let mapped = map_err(&site, err);
            assert_eq!(
                mapped.class,
                ErrorClass::JvmShellUnsupported,
                "detail={detail}"
            );
        }
    }
}
