// Tauri 命令层：前端通过 `invoke` 调用这里
//
// 订阅模型（对齐 TVBoxOS jun 版）：
// - 用户添加"订阅"：{ name, url }。
// - 订阅本身可能是"单配置"，也可能是"线路合集/多仓"（顶层只有 urls、没有 sites）。
// - 合集不合并：一次只激活一条线路；切换线路 = 重新加载该线路配置。
// - 启用状态持久化到 <app_data_dir>/subscriptions.json。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;
use vbox_core::config::{
    ConfigInspection, LoadOptions, LoadedConfig, inspect_config, load_config_with_options,
};
use vbox_core::error::Result as CoreResult;
use vbox_core::model::{Category, Parser, PlayResult, SearchHit, TvConfig, VideoDetail, VideoItem};

/* ---------------- 数据结构 ---------------- */

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Subscription {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddResult {
    pub subscriptions: Vec<Subscription>,
    pub enabled: Option<String>,
    pub loaded: Option<LoadedConfig>,
}

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
struct SubStore {
    #[serde(default)]
    subscriptions: Vec<Subscription>,
    /// 当前激活线路 URL
    #[serde(default)]
    enabled: Option<String>,
    /// 当前激活线路所属的订阅/合集入口 URL；单配置时等于 enabled
    #[serde(default)]
    source: Option<String>,
    /// 定时刷新间隔（分钟，0 = 关闭）
    #[serde(default)]
    refresh_minutes: u32,
}

/// 订阅相关设置（前端展示用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionPrefs {
    /// 定时刷新间隔（分钟，0 = 关闭）
    pub refresh_minutes: u32,
}

/// 订阅导入结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    /// 新增的订阅数
    pub imported: usize,
    /// 因重复/非法跳过的条目数
    pub skipped: usize,
    /// 合并后的订阅列表
    pub subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
struct PythonConfig {
    /// 空 = 自动探测（VBOX_PYTHON → python → python3 → py -3）
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
struct NodeConfig {
    /// 空 = 自动探测（VBOX_NODE → node → node.exe）
    #[serde(default)]
    path: Option<String>,
}

/* ---------------- 应用状态 ---------------- */

pub struct AppState {
    /// 当前激活线路的完整加载结果（含 lines / activeLine / config）
    pub loaded: Mutex<Option<LoadedConfig>>,
    /// 配置缓存目录
    pub cache_dir: PathBuf,
    /// 豆瓣首页磁盘缓存文件（<app_data_dir>/douban-home.json）
    pub douban_cache: PathBuf,
    /// 本地文件/代理服务基址（http://127.0.0.1:<port>）；启动服务后写入
    pub local_base: Mutex<Option<String>>,
    /// 默认解析器 name（前端可覆盖）
    pub default_parse: Mutex<Option<String>>,
    /// 正在进行的解析/抓包取消令牌（同一时刻只允许一个；前端点「取消」时置位）
    pub parse_cancel: Mutex<Option<(String, Arc<AtomicBool>)>>,
    /// Python 解释器配置文件（<app_data_dir>/python.json）
    pub python_config: PathBuf,
    /// Node 解释器配置文件（<app_data_dir>/node.json）
    pub node_config: PathBuf,
    /// 网盘登录会话（W13）：provider → LoginFlow 注册表 + 按 ticket_id 的会话表
    pub pan_login: vbox_core::pan::login::LoginHub,
    /// 网盘凭据存储（W13 D4）：DPAPI 加密的 credentials.json + 内存 overlay
    pub pan_credentials: Arc<crate::pan_credentials::PanCredentialStore>,
    /// 云盘聚合搜索（W14 P1）：内置 Telegram 内核 + 可选 PanSou 增强源
    pub cloud: Arc<crate::commands_cloud::CloudState>,
}

impl AppState {
    pub fn new(app_data_dir: PathBuf) -> Self {
        let cache_dir = app_data_dir.join("config-cache");
        let _ = fs::create_dir_all(&cache_dir);
        // 豆瓣首页磁盘缓存：启动时先读它渲染首屏，再后台增量刷新。
        let douban_cache = app_data_dir.join("douban-home.json");
        let python_config = app_data_dir.join("python.json");
        if let Ok(text) = fs::read_to_string(&python_config) {
            if let Ok(cfg) = serde_json::from_str::<PythonConfig>(&text) {
                vbox_core::python::set_preferred(cfg.path.as_deref());
            }
        }
        let node_config = app_data_dir.join("node.json");
        if let Ok(text) = fs::read_to_string(&node_config) {
            if let Ok(cfg) = serde_json::from_str::<NodeConfig>(&text) {
                vbox_core::node::set_preferred(cfg.path.as_deref());
            }
        }
        // W13：网盘登录 provider（115/夸克/迅雷走协议登录；阿里/UC/PikPak 走前端 WebView 兜底）。
        let pan_login = vbox_core::pan::login::default_hub().unwrap_or_else(|err| {
            eprintln!("[vbox] 网盘登录 provider 初始化失败（登录面板不可用）: {err}");
            vbox_core::pan::login::LoginHub::new()
        });
        // W13 D4：本机加密凭据存储；启动时把最近凭据恢复进内存 overlay，供既有网盘引擎直接复用。
        let pan_credentials = Arc::new(crate::pan_credentials::PanCredentialStore::new(
            &app_data_dir,
        ));
        let restored = pan_credentials.restore_overlay();
        if !restored.is_empty() {
            eprintln!("[vbox] 已恢复 {} 个网盘账号凭据", restored.len());
        }
        // W14 P1：云盘聚合搜索内核（内置 Telegram 频道源；PanSou 增强源按本机配置启用）。
        let cloud = Arc::new(crate::commands_cloud::CloudState::new(&app_data_dir));
        Self {
            loaded: Mutex::new(None),
            cache_dir,
            douban_cache,
            local_base: Mutex::new(None),
            default_parse: Mutex::new(None),
            parse_cancel: Mutex::new(None),
            python_config,
            node_config,
            pan_login,
            pan_credentials,
            cloud,
        }
    }

    fn load_options(&self, force_refresh: bool) -> LoadOptions {
        let local_base = self.local_base.lock().ok().and_then(|g| g.clone());
        LoadOptions {
            use_cache: true,
            cache_dir: Some(self.cache_dir.clone()),
            local_base,
            force_refresh,
            ..LoadOptions::default()
        }
    }
}

/* ---------------- 工具 ---------------- */

fn wrap<T: Serialize>(r: CoreResult<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

fn subs_path(app: &tauri::AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let _ = fs::create_dir_all(&dir);
    dir.join("subscriptions.json")
}

fn read_store(app: &tauri::AppHandle) -> SubStore {
    let p = subs_path(app);
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 原子写 subscriptions.json：先写临时文件再 rename，避免崩溃/断电留下半截 JSON。
fn write_store(app: &tauri::AppHandle, store: &SubStore) -> Result<(), String> {
    let p = subs_path(app);
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let data = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, &data).map_err(|e| format!("写入订阅临时文件失败: {e}"))?;
    match fs::rename(&tmp, &p) {
        Ok(()) => Ok(()),
        Err(first) => {
            // Windows 上目标被短暂占用时 rename 可能失败：删掉目标再试一次
            if fs::remove_file(&p).is_ok() && fs::rename(&tmp, &p).is_ok() {
                return Ok(());
            }
            let _ = fs::remove_file(&tmp);
            Err(format!("保存订阅文件失败: {first}"))
        }
    }
}

/// 校验订阅名称/链接（双必填）。
fn validate_subscription(name: &str, url: &str) -> Result<(String, String), String> {
    let name = name.trim().to_string();
    let url = url.trim().to_string();
    if name.is_empty() || url.is_empty() {
        return Err("订阅名称和链接均为必填".to_string());
    }
    Ok((name, url))
}

/// 取出当前配置的**快照**（克隆）后立刻释放锁。
///
/// 网络请求可能持续数秒：绝不能在持有 `loaded` 锁的期间做 IO，否则并发的
/// resolve / 详情加载会互相排队（主线程不再被阻塞，但请求会白白等待）。
/// 站点展示名（用于源健康记录）。
fn site_label(cfg: &TvConfig, site: &str) -> String {
    cfg.sites
        .iter()
        .find(|item| item.key == site)
        .map(|item| item.name.clone())
        .unwrap_or_default()
}

/// 记录站点请求结果：成功清零连续失败；失败累计并保存错误文本。
fn record_site_health<T>(
    app: &tauri::AppHandle,
    site: &str,
    label: &str,
    result: &Result<T, String>,
) {
    let Some(store) = app.try_state::<crate::source_health::SourceHealthStore>() else {
        return;
    };
    match result {
        Ok(_) => {
            let _ = store.record("site", site, label, true, None);
        }
        Err(err) => {
            let _ = store.record("site", site, label, false, Some(err));
        }
    }
}

/// 记录线路加载结果（key = 线路 URL）。
fn record_line_health(app: &tauri::AppHandle, key: &str, ok: bool, error: Option<&str>) {
    let Some(store) = app.try_state::<crate::source_health::SourceHealthStore>() else {
        return;
    };
    let _ = store.record("line", key, "", ok, error);
}

fn config_snapshot(state: &tauri::State<'_, AppState>) -> Result<TvConfig, String> {
    let guard = state
        .loaded
        .lock()
        .map_err(|_| "配置状态锁损坏".to_string())?;
    let loaded = guard
        .as_ref()
        .ok_or_else(|| "尚未启用任何订阅地址，请先在设置页选择一个地址".to_string())?;
    Ok(loaded.config.clone())
}

/// 当前激活线路 URL 的快照（取不到时返回空串）。
///
/// 分类缓存按「线路 + 站点」分区：切线路后不会误用上一条线路的分类标签。
fn active_line_snapshot(state: &tauri::State<'_, AppState>) -> String {
    state
        .loaded
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|loaded| loaded.active_line.clone()))
        .unwrap_or_default()
}

fn with_config<T, F>(state: &tauri::State<AppState>, f: F) -> Result<T, String>
where
    T: Serialize,
    F: FnOnce(&TvConfig) -> CoreResult<T>,
{
    let cfg = config_snapshot(state)?;
    wrap(f(&cfg))
}

/// 在**阻塞线程池**里执行重活（网络 / 子进程 / 大配置读取）。
///
/// 为什么不能只用 `#[tauri::command(async)]`（同步函数 + Tauri 异步执行）：
/// 它只是把同步函数放进 tokio 任务，也就是跑在 tokio **worker 线程的异步上下文**里；
/// 而核心层 HTTP 用的是 `reqwest::blocking`，在异步上下文里销毁它的内部 runtime 会 panic
/// （`Cannot drop a runtime in a context where blocking is not allowed`），
/// 命令 promise 永不返回 —— 表现为页面一直转圈/加载不出任何数据。
/// `spawn_blocking` 是真正的阻塞线程（没有 async 上下文），才是安全位置。
async fn run_blocking<T, F>(task: &'static str, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(f).await {
        Ok(result) => result,
        Err(e) => Err(format!("{task}执行失败: {e}")),
    }
}

/* ---------------- 订阅管理 ---------------- */

#[tauri::command]
pub fn get_subscriptions(app: tauri::AppHandle) -> Vec<Subscription> {
    read_store(&app).subscriptions
}

#[tauri::command]
pub fn get_enabled(app: tauri::AppHandle) -> Option<String> {
    read_store(&app).enabled
}

#[tauri::command]
pub fn get_loaded(state: tauri::State<AppState>) -> Option<LoadedConfig> {
    state.loaded.lock().ok().and_then(|g| g.clone())
}

/// 编辑订阅名称/链接；同步更新启用状态里对旧 URL 的引用。
#[tauri::command]
pub fn update_subscription(
    app: tauri::AppHandle,
    old_url: String,
    name: String,
    url: String,
) -> Result<Vec<Subscription>, String> {
    let (name, url) = validate_subscription(&name, &url)?;
    let old_url = old_url.trim().to_string();
    let mut store = read_store(&app);
    let index = store
        .subscriptions
        .iter()
        .position(|sub| sub.url == old_url)
        .ok_or_else(|| "订阅不存在（可能已被删除）".to_string())?;
    if store
        .subscriptions
        .iter()
        .enumerate()
        .any(|(i, sub)| i != index && sub.url == url)
    {
        return Err("已存在相同链接的订阅".to_string());
    }
    store.subscriptions[index] = Subscription {
        name,
        url: url.clone(),
    };
    if store.enabled.as_deref() == Some(old_url.as_str()) {
        store.enabled = Some(url.clone());
    }
    if store.source.as_deref() == Some(old_url.as_str()) {
        store.source = Some(url);
    }
    write_store(&app, &store)?;
    Ok(store.subscriptions)
}

/// 导出订阅到 JSON 文件（原生保存对话框；取消返回 None）。
#[tauri::command]
pub async fn export_subscriptions(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let store = read_store(&app);
    let payload = serde_json::json!({
        "version": 1,
        "subscriptions": store.subscriptions,
    });
    let data =
        serde_json::to_string_pretty(&payload).map_err(|e| format!("订阅序列化失败: {e}"))?;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title("导出订阅")
            .set_file_name("vbox-subscriptions.json")
            .add_filter("JSON", &["json"])
            .save_file()
    })
    .await
    .map_err(|e| format!("打开保存对话框失败: {e}"))?;
    let Some(path) = picked else {
        return Ok(None);
    };
    fs::write(&path, data).map_err(|e| format!("写入订阅文件失败: {e}"))?;
    Ok(Some(path.to_string_lossy().to_string()))
}

/// 从 JSON 文件导入订阅（按 URL 去重，已存在的跳过；取消返回 None）。
#[tauri::command]
pub async fn import_subscriptions(app: tauri::AppHandle) -> Result<Option<ImportResult>, String> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("导入订阅")
            .add_filter("JSON", &["json"])
            .pick_file()
    })
    .await
    .map_err(|e| format!("打开文件对话框失败: {e}"))?;
    let Some(path) = picked else {
        return Ok(None);
    };
    let text = fs::read_to_string(&path).map_err(|e| format!("读取订阅文件失败: {e}"))?;
    #[derive(Deserialize)]
    struct ImportFile {
        #[serde(default)]
        subscriptions: Vec<Subscription>,
    }
    let parsed: ImportFile = serde_json::from_str(&text)
        .map_err(|e| format!("订阅文件解析失败（需为 subscriptions 数组格式）: {e}"))?;

    let mut store = read_store(&app);
    let mut imported = 0usize;
    let mut skipped = 0usize;
    for item in parsed.subscriptions {
        match validate_subscription(&item.name, &item.url) {
            Ok((name, url)) => {
                if store.subscriptions.iter().any(|sub| sub.url == url) {
                    skipped += 1;
                } else {
                    store.subscriptions.push(Subscription { name, url });
                    imported += 1;
                }
            }
            Err(_) => skipped += 1,
        }
    }
    write_store(&app, &store)?;
    Ok(Some(ImportResult {
        imported,
        skipped,
        subscriptions: store.subscriptions,
    }))
}

/// 读取订阅相关设置（当前只有定时刷新间隔）。
#[tauri::command]
pub fn subscription_prefs(app: tauri::AppHandle) -> SubscriptionPrefs {
    SubscriptionPrefs {
        refresh_minutes: read_store(&app).refresh_minutes,
    }
}

/// 设置定时刷新间隔（分钟，0 = 关闭；上限 24 小时）。
#[tauri::command]
pub fn set_subscription_refresh(
    app: tauri::AppHandle,
    minutes: u32,
) -> Result<SubscriptionPrefs, String> {
    let mut store = read_store(&app);
    store.refresh_minutes = minutes.min(24 * 60);
    write_store(&app, &store)?;
    Ok(SubscriptionPrefs {
        refresh_minutes: store.refresh_minutes,
    })
}

#[tauri::command]
pub async fn add_subscription(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    name: String,
    url: String,
) -> Result<AddResult, String> {
    let name = name.trim().to_string();
    let url = url.trim().to_string();
    if name.is_empty() || url.is_empty() {
        return Err("订阅名称和链接均为必填".into());
    }

    let mut store = read_store(&app);
    if !store.subscriptions.iter().any(|s| s.url == url) {
        store.subscriptions.push(Subscription {
            name,
            url: url.clone(),
        });
    }

    let mut loaded = None;
    if store.enabled.is_none() {
        // 首个订阅：加载第一条线路（单配置即自身；合集取 urls[0]）
        let opts = state.load_options(false);
        let target = url.clone();
        if let Ok(result) = run_blocking("订阅配置加载", move || {
            wrap(load_config_with_options(&target, &opts))
        })
        .await
        {
            store.enabled = Some(result.active_line.clone());
            store.source = Some(result.source_url.clone());
            *state
                .loaded
                .lock()
                .map_err(|_| "配置状态锁损坏".to_string())? = Some(result.clone());
            loaded = Some(result);
        }
    }

    write_store(&app, &store)?;
    Ok(AddResult {
        subscriptions: store.subscriptions.clone(),
        enabled: store.enabled.clone(),
        loaded,
    })
}

#[tauri::command]
pub async fn remove_subscription(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    url: String,
) -> Result<Vec<Subscription>, String> {
    let mut store = read_store(&app);
    store.subscriptions.retain(|s| s.url != url);

    if let Some(enabled) = store.enabled.clone() {
        let mut clear = enabled == url || store.source.as_deref() == Some(url.as_str());
        if !clear {
            // 若 enabled 是该订阅线路合集里的某条线路，也一并清理
            let opts = state.load_options(false);
            let target = url.clone();
            if let Ok(inspect) = run_blocking("订阅地址读取", move || {
                wrap(inspect_config(&target, &opts))
            })
            .await
            {
                if inspect.lines.iter().any(|l| l.url == enabled) {
                    clear = true;
                }
            }
        }
        if clear {
            store.enabled = None;
            store.source = None;
            *state
                .loaded
                .lock()
                .map_err(|_| "配置状态锁损坏".to_string())? = None;
        }
    }

    write_store(&app, &store)?;
    Ok(store.subscriptions)
}

/// 展开订阅：返回单仓/多仓（线路合集）与地址列表
#[tauri::command]
pub async fn get_addresses(
    state: tauri::State<'_, AppState>,
    name: String,
    url: String,
) -> Result<ConfigInspection, String> {
    let opts = state.load_options(false);
    let mut inspection = run_blocking("订阅地址展开", move || {
        wrap(inspect_config(&url, &opts))
    })
    .await?;
    if inspection.kind == vbox_core::config::ConfigKind::Single {
        // 单仓展示订阅名称作为地址名
        if let Some(line) = inspection.lines.first_mut() {
            line.name = Some(name);
        }
    }
    Ok(inspection)
}

/// 启用某条地址。
///
/// - 单配置：`source_url` 省略，`url` 即配置地址。
/// - 线路合集：前端应传 `source_url`=合集入口、`line_url`=要激活的线路；
///   只传 `url` 也兼容（会当作单配置加载，再存为自身）。
#[tauri::command]
pub async fn set_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    url: String,
    force_refresh: Option<bool>,
    source_url: Option<String>,
    line_url: Option<String>,
) -> Result<LoadedConfig, String> {
    let requested_line = url.clone();
    let source = source_url
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| url.clone());
    let mut opts = state.load_options(force_refresh.unwrap_or(false));
    opts.preferred_line = line_url.or_else(|| {
        if source != url {
            Some(url.clone())
        } else {
            None
        }
    });
    let loaded = run_blocking("配置加载", move || {
        wrap(load_config_with_options(&source, &opts))
    })
    .await;
    let result = match loaded {
        Ok(value) => {
            record_line_health(&app, &value.active_line, true, None);
            value
        }
        Err(err) => {
            record_line_health(&app, &requested_line, false, Some(&err));
            return Err(err);
        }
    };

    let mut store = read_store(&app);
    store.enabled = Some(result.active_line.clone());
    store.source = Some(result.source_url.clone());
    write_store(&app, &store)?;

    *state
        .loaded
        .lock()
        .map_err(|_| "配置状态锁损坏".to_string())? = Some(result.clone());
    Ok(result)
}

/// 恢复上次启用的线路（来源 + 线路 URL），供应用启动时使用
#[tauri::command]
pub async fn restore_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    force_refresh: Option<bool>,
) -> Result<LoadedConfig, String> {
    let store = read_store(&app);
    let enabled = store
        .enabled
        .ok_or_else(|| "尚未启用任何地址".to_string())?;
    let source = store.source.unwrap_or_else(|| enabled.clone());
    let mut opts = state.load_options(force_refresh.unwrap_or(false));
    opts.preferred_line = Some(enabled.clone());
    let loaded = run_blocking("配置恢复", move || {
        wrap(load_config_with_options(&source, &opts))
    })
    .await;
    let result = match loaded {
        Ok(value) => {
            record_line_health(&app, &value.active_line, true, None);
            value
        }
        Err(err) => {
            record_line_health(&app, &enabled, false, Some(&err));
            return Err(err);
        }
    };

    let mut store = read_store(&app);
    store.enabled = Some(result.active_line.clone());
    store.source = Some(result.source_url.clone());
    write_store(&app, &store)?;

    *state
        .loaded
        .lock()
        .map_err(|_| "配置状态锁损坏".to_string())? = Some(result.clone());
    Ok(result)
}

/// 重新加载当前启用线路
#[tauri::command]
pub async fn reload_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    force_refresh: Option<bool>,
) -> Result<LoadedConfig, String> {
    restore_enabled(app, state, force_refresh).await
}

#[tauri::command]
pub async fn load_config(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<LoadedConfig, String> {
    let opts = state.load_options(false);
    run_blocking("配置加载", move || {
        wrap(load_config_with_options(&path, &opts))
    })
    .await
}

/* ---------------- 读取型命令 ---------------- */

#[tauri::command]
pub async fn list_categories(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    cache: tauri::State<'_, crate::vod_cache::VodCacheStore>,
    site: String,
    force: Option<bool>,
) -> Result<Vec<Category>, String> {
    let line = active_line_snapshot(&state);
    // 30 分钟内命中缓存：直接返回，不联网、也不记源健康（避免「假失败」）。
    if force != Some(true) {
        if let Some(items) = cache.categories_fresh(&line, &site) {
            return Ok(items);
        }
    }

    let cfg = config_snapshot(&state)?;
    let label = site_label(&cfg, &site);
    let site_key = site.clone();
    let result = run_blocking("分类加载", move || {
        wrap(vbox_core::list_categories(&cfg, &site))
    })
    .await;

    // 缓存写回放在 await 之后：tauri::State 不能 move 进阻塞线程。
    if let Ok(items) = &result {
        if let Err(err) = cache.save_categories(&line, &site_key, items) {
            eprintln!("[vbox] 分类缓存写入失败(line={line}, site={site_key}): {err}");
        }
    }
    record_site_health(&app, &site_key, &label, &result);
    result
}

/// 分类标签的磁盘缓存（首屏直接渲染，不联网、不记源健康）。
///
/// 注意：Tauri 2 不允许「带引用参数（State）的 async 命令」返回非 `Result`，
/// 而这里只是读一次小文件，直接同步返回 `Option` 更简单也更贴合首屏语义。
#[tauri::command]
pub fn list_categories_cached(
    state: tauri::State<'_, AppState>,
    cache: tauri::State<'_, crate::vod_cache::VodCacheStore>,
    site: String,
) -> Option<Vec<Category>> {
    let line = active_line_snapshot(&state);
    cache.categories(&line, &site)
}

#[tauri::command]
pub async fn list_videos(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    site: String,
    category: String,
    page: u32,
) -> Result<Vec<VideoItem>, String> {
    let cfg = config_snapshot(&state)?;
    let label = site_label(&cfg, &site);
    let site_key = site.clone();
    let result = run_blocking("列表加载", move || {
        wrap(vbox_core::list_videos(&cfg, &site, &category, page))
    })
    .await;
    record_site_health(&app, &site_key, &label, &result);
    result
}

#[tauri::command]
pub async fn get_detail(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    site: String,
    id: String,
) -> Result<VideoDetail, String> {
    let cfg = config_snapshot(&state)?;
    let label = site_label(&cfg, &site);
    let site_key = site.clone();
    let result = run_blocking("详情加载", move || {
        wrap(vbox_core::get_detail(&cfg, &site, &id))
    })
    .await;
    record_site_health(&app, &site_key, &label, &result);
    result
}

/// 搜索分页：一次全网搜索的结果在进程内保留 5 分钟，翻页不重复发起请求。
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(300);

struct SearchCache {
    key: String,
    at: Instant,
    hits: Vec<SearchHit>,
}

static SEARCH_CACHE: Mutex<Option<SearchCache>> = Mutex::new(None);

/// 分页搜索返回：`total` 是本次检索的总命中数，`hasMore` 便于前端停止加载。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub items: Vec<SearchHit>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub has_more: bool,
}

/// 同步执行一次检索（`all` 跨源并发，否则只搜 `site`）。
fn collect_search(
    cfg: &TvConfig,
    site: Option<&str>,
    keyword: &str,
    all: bool,
    threads: usize,
) -> Result<Vec<SearchHit>, String> {
    if all {
        return Ok(vbox_core::search::search_all_with_threads(
            cfg, keyword, 60, 8000, threads,
        ));
    }
    let Some(site_key) = site.filter(|s| !s.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let site_obj = cfg
        .sites
        .iter()
        .find(|s| s.key == site_key)
        .ok_or_else(|| format!("站点不存在: {site_key}"))?;
    let items = wrap(vbox_core::search::search_site(
        cfg, site_obj, keyword, false,
    ))?;
    Ok(items
        .into_iter()
        .map(|item| SearchHit {
            site_key: site_obj.key.clone(),
            site_name: site_obj.name.clone(),
            item,
        })
        .collect())
}

/// Panyq：搜索前用隐藏 WebView 捕获页面运行时生成的 `sign`。
///
/// 该站的 `sign` 是页面脚本现算的（服务端渲染只会返回 Next.js 错误边界，
/// 前端 chunk 里也没有生成点），所以只能让页面自己跑一遍、再拦截它发出的
/// `/api/search?sign=` 请求。捕获失败时静默跳过，由 core 层回落
/// `catvod.host.unsupported` 并给出可操作提示（ext 里写 sign / sign_command）。
async fn prime_panyq_sign(
    app: &tauri::AppHandle,
    cfg: &vbox_core::model::TvConfig,
    site: Option<&str>,
    keyword: &str,
) {
    use vbox_core::spider::catvod::families::panyq;
    let Some(site_key) = site.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    let Some(site_obj) = cfg.sites.iter().find(|item| item.key == site_key) else {
        return;
    };
    if !panyq::host_capture_needed(site_obj) {
        return;
    }
    let Ok(page_url) = panyq::host_page_url(site_obj, keyword, 1) else {
        return;
    };
    if let Some(sign_url) =
        crate::capture::capture_api_url_async(app, &page_url, "/api/search?sign=", 8_000).await
    {
        panyq::put_sign(&site_obj.key, &sign_url);
    }
}

/// 取一页并返回 `(本页, 是否还有下一页)`；`page` 从 1 开始。
fn slice_page<T: Clone>(items: &[T], page: usize, page_size: usize) -> (Vec<T>, bool) {
    let start = (page - 1).saturating_mul(page_size);
    if start >= items.len() {
        return (Vec::new(), false);
    }
    let end = (start + page_size).min(items.len());
    (items[start..end].to_vec(), end < items.len())
}

/// 搜索：scope = "current" | "all"
///
/// `all`（搜索页默认）走跨源并发搜索，线程数取「设置 → 搜索 → 搜索线程」
/// （`<app_data_dir>/search.json`）；`current` 只搜单个站点（搜索页的站点刷新）。
#[tauri::command]
pub async fn search(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    prefs: tauri::State<'_, crate::search_prefs::SearchPrefsStore>,
    site: Option<String>,
    keyword: String,
    scope: Option<String>,
) -> Result<Vec<SearchHit>, String> {
    let keyword = keyword.trim().to_string();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let all = scope.as_deref() == Some("all");
    let threads = prefs.load().threads as usize;
    let cfg = config_snapshot(&state)?;
    prime_panyq_sign(&app, &cfg, site.as_deref(), &keyword).await;

    run_blocking("搜索", move || {
        collect_search(&cfg, site.as_deref(), &keyword, all, threads)
    })
    .await
}

/// 分页搜索：供搜索页「加载更多」使用。
///
/// 首次请求执行一次完整检索并缓存 5 分钟，后续翻页直接切片，不重复打站点；
/// `page` 从 1 开始，`page_size` 夹在 1..=200。
#[tauri::command]
pub async fn search_page(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    prefs: tauri::State<'_, crate::search_prefs::SearchPrefsStore>,
    site: Option<String>,
    keyword: String,
    scope: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<SearchPage, String> {
    let keyword = keyword.trim().to_string();
    let page = page.unwrap_or(1).max(1) as usize;
    let page_size = page_size.unwrap_or(60).clamp(1, 200) as usize;
    let empty = SearchPage {
        items: Vec::new(),
        total: 0,
        page,
        page_size,
        has_more: false,
    };
    if keyword.is_empty() {
        return Ok(empty);
    }

    let all = scope.as_deref() == Some("all");
    let threads = prefs.load().threads as usize;
    let cfg = config_snapshot(&state)?;
    let cache_key = format!(
        "{}|{}|{}|{}",
        if all { "all" } else { "current" },
        site.as_deref().unwrap_or(""),
        keyword,
        threads
    );

    // 命中未过期缓存则直接切片；过期/不同关键词则重新检索。
    let cached = SEARCH_CACHE.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .filter(|entry| entry.key == cache_key && entry.at.elapsed() < SEARCH_CACHE_TTL)
            .map(|entry| entry.hits.clone())
    });

    let hits = match cached {
        Some(hits) => hits,
        None => {
            prime_panyq_sign(&app, &cfg, site.as_deref(), &keyword).await;
            let fetched = run_blocking("搜索", move || {
                collect_search(&cfg, site.as_deref(), &keyword, all, threads)
            })
            .await?;
            if let Ok(mut guard) = SEARCH_CACHE.lock() {
                *guard = Some(SearchCache {
                    key: cache_key,
                    at: Instant::now(),
                    hits: fetched.clone(),
                });
            }
            fetched
        }
    };

    let total = hits.len();
    let (items, has_more) = slice_page(&hits, page, page_size);
    Ok(SearchPage {
        items,
        total,
        page,
        page_size,
        has_more,
    })
}

/// 播放解析：站点 + flag + 剧集 id/URL -> 真实直链（或需要嗅探的播放页）
///
/// 关键点（曾经是「点解析没反应 + APP 卡住」的根因）：
/// - 解析链内部是**阻塞 HTTP**（最长 20s），不能让它在 Tauri 主线程跑（同步命令会跑在主
///   线程并阻塞窗口消息循环，WebView 无法重绘）。这里改成 async 命令 +
///   `spawn_blocking`，主线程全程可用，进度事件才能即时渲染。
/// - `request_id` 由前端生成，用于把 `parse-progress` 事件过滤到本次解析；
///   同时登记取消令牌，供 `cancel_parse` 置位。
#[tauri::command]
pub async fn resolve(
    app: tauri::AppHandle,
    site: String,
    flag: Option<String>,
    url: Option<String>,
    id: Option<String>,
    parse_name: Option<String>,
    request_id: Option<String>,
) -> Result<PlayResult, String> {
    let (cfg, parse_name, default_parse, site_key, site_name) = {
        let cfg = config_snapshot(&app.state::<AppState>())?;
        let site_key = site.clone();
        let site_name = site_label(&cfg, &site);
        let default_parse = app
            .state::<AppState>()
            .default_parse
            .lock()
            .ok()
            .and_then(|g| g.clone());
        // 注意：只有用户**显式**选择解析器时才走解析链；default_parse 仅作为
        // «需要解析链时» 的偏好，不能当成显式选择，否则会破坏 TVBox 默认的直嗅路径。
        let _ = &default_parse;
        (cfg, parse_name, default_parse, site_key, site_name)
    };

    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(rid) = request_id.as_deref() {
        if let Ok(mut slot) = app.state::<AppState>().parse_cancel.lock() {
            *slot = Some((rid.to_string(), Arc::clone(&cancel)));
        }
    }
    let sink = crate::progress::sink(&app, request_id.clone(), Arc::clone(&cancel));

    let parsed = tauri::async_runtime::spawn_blocking(move || {
        vbox_core::source::get_play_with_progress(
            &cfg,
            &site,
            flag.as_deref().unwrap_or(""),
            // TVBox 的 playerContent(flag, id) 里的 id 实际是"剧集 URL"（详情页选集项的 url），
            // 不是视频详情 id；因此优先用 url，只有前端没传时再退回 id。
            url.as_deref().or(id.as_deref()).unwrap_or(""),
            url.as_deref().unwrap_or(""),
            parse_name.as_deref(),
            default_parse.as_deref(),
            &sink,
        )
    })
    .await
    .map_err(|e| format!("解析任务执行失败: {e}"))?;

    // 清理取消令牌（只清理属于自己的那条，避免误清后一次解析）
    if let Some(rid) = request_id.as_deref() {
        if let Ok(mut slot) = app.state::<AppState>().parse_cancel.lock() {
            if slot.as_ref().map(|(id, _)| id == rid).unwrap_or(false) {
                *slot = None;
            }
        }
    }

    let result = parsed.map_err(|e| e.to_string());
    record_site_health(&app, &site_key, &site_name, &result);
    result
}

/// 取消正在进行的解析 / 抓包（尽力而为）。
///
/// 阻塞中的 HTTP 请求无法中断，取消只在「解析器之间 / 抓包轮询间」生效：
/// 返回 true 表示匹配到了本次请求并已置取消位。
#[tauri::command]
pub fn cancel_parse(state: tauri::State<'_, AppState>, request_id: String) -> bool {
    let Ok(guard) = state.parse_cancel.lock() else {
        return false;
    };
    match guard.as_ref() {
        Some((id, token)) if id == &request_id => {
            token.store(true, Ordering::SeqCst);
            true
        }
        _ => false,
    }
}

#[tauri::command]
pub fn list_parses(state: tauri::State<AppState>) -> Result<Vec<Parser>, String> {
    with_config(&state, |cfg| Ok(cfg.parses.clone()))
}

#[tauri::command]
pub fn set_default_parse(state: tauri::State<AppState>, name: String) -> Result<(), String> {
    *state
        .default_parse
        .lock()
        .map_err(|_| "解析器状态锁损坏".to_string())? = Some(name);
    Ok(())
}

/* ---------------- 抓包 / 调试 ---------------- */

#[tauri::command]
pub fn network_log(limit: Option<usize>) -> Vec<vbox_core::capture::NetworkEvent> {
    vbox_core::capture::recent(limit.unwrap_or(200))
}

#[tauri::command]
pub fn network_clear() {
    vbox_core::capture::clear();
}

/* ---------------- 本地媒体（Range 流式，供 mpv seek） ---------------- */

/// 把本地绝对路径转换成 `http://127.0.0.1:{port}/file/...`。
///
/// 走本地 HTTP 而不是直接传路径，是为了让 mpv 通过 Range 按需读取：
/// moov 在文件尾部的 MP4 也能快速起播/拖动，不必整文件加载。
#[tauri::command]
pub fn local_media_url(state: tauri::State<'_, AppState>, path: String) -> Result<String, String> {
    let base = state
        .local_base
        .lock()
        .ok()
        .and_then(|base| base.clone())
        .ok_or_else(|| "本地服务未启动".to_string())?;
    if path.trim().is_empty() {
        return Err("路径为空".into());
    }
    let encoded = percent_encoding::utf8_percent_encode(&path, percent_encoding::NON_ALPHANUMERIC);
    Ok(format!("{base}/file/{encoded}"))
}

/* ---------------- 点播首页（豆瓣） ---------------- */

/// 豆瓣首页数据：8 个集合 + 10 条轮播（带详情）。
///
/// 抓取是阻塞 HTTP（reqwest::blocking），必须放在 `spawn_blocking` 线程里；
/// 直接在 async 上下文跑会 panic（Cannot drop a runtime in a context where
/// blocking is not allowed）。
///
/// 现在带磁盘缓存：`force = false` 只增量刷新过期集合（未过期集合不联网），
/// 抓取失败的集合保留旧数据；缓存文件在 `<app_data_dir>/douban-home.json`。
#[tauri::command]
pub async fn douban_home(
    state: tauri::State<'_, AppState>,
    force: Option<bool>,
) -> Result<vbox_core::douban::DoubanHome, String> {
    // 先把本地服务基址与磁盘缓存路径取出来（不跨 await 持锁）。
    let image_proxy = state
        .local_base
        .lock()
        .ok()
        .and_then(|base| base.clone())
        .map(|base| format!("{base}/img?url="));
    let cache_path = state.douban_cache.clone();

    let mut home = run_blocking("豆瓣首页加载", move || {
        vbox_core::douban::home_with_cache(Some(cache_path.as_path()), force.unwrap_or(false))
    })
    .await?;

    home.image_proxy = image_proxy;
    Ok(home)
}

/// 豆瓣首页磁盘缓存：启动首屏直接渲染（不联网、不阻塞 UI）。
///
/// 同 [`list_categories_cached`]：Tauri 2 的 async 命令带引用参数时必须返回
/// `Result`，这里只需读一个小 JSON，直接同步返回 `Option` 即可。
#[tauri::command]
pub fn douban_home_cached(
    state: tauri::State<'_, AppState>,
) -> Option<vbox_core::douban::DoubanHome> {
    // 先取本地服务基址（不跨 await 持锁），再读缓存文件。
    let image_proxy = state
        .local_base
        .lock()
        .ok()
        .and_then(|base| base.clone())
        .map(|base| format!("{base}/img?url="));

    let mut home = vbox_core::douban::cached_home(state.douban_cache.as_path())?;
    home.image_proxy = image_proxy;
    Some(home)
}

/// 媒体地址嗅探（WebView 抓包）；具体实现见 src-tauri/src/capture.rs
///
/// 抓包的 300ms 轮询是阻塞代码，capture 内部已丢给 `spawn_blocking`；
/// 这里同样登记取消令牌并转发 `parse-progress` 事件（sniff 阶段）。
#[tauri::command]
pub async fn sniff(
    app: tauri::AppHandle,
    url: String,
    headers: Option<HashMap<String, String>>,
    click: Option<String>,
    timeout_ms: Option<u64>,
    request_id: Option<String>,
) -> Result<PlayResult, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(rid) = request_id.as_deref() {
        if let Ok(mut slot) = app.state::<AppState>().parse_cancel.lock() {
            *slot = Some((rid.to_string(), Arc::clone(&cancel)));
        }
    }
    let sink = crate::progress::sink(&app, request_id.clone(), Arc::clone(&cancel));

    let result = crate::capture::sniff_async(
        &app,
        &url,
        headers.unwrap_or_default(),
        click,
        timeout_ms.unwrap_or(20_000),
        &sink,
    )
    .await;

    if let Some(rid) = request_id.as_deref() {
        if let Ok(mut slot) = app.state::<AppState>().parse_cancel.lock() {
            if slot.as_ref().map(|(id, _)| id == rid).unwrap_or(false) {
                *slot = None;
            }
        }
    }

    result
}

/* ---------------- 直播 ---------------- */

#[tauri::command]
pub fn live_sources(
    state: tauri::State<AppState>,
) -> Result<Vec<vbox_core::model::LiveSource>, String> {
    with_config(&state, |cfg| Ok(cfg.lives.clone()))
}

/// 加载某个 lives 组的频道（type0 文本订阅 / type3 js）
#[tauri::command]
pub async fn live_groups(
    state: tauri::State<'_, AppState>,
    index: usize,
) -> Result<Vec<vbox_core::live::LiveGroup>, String> {
    let cfg = config_snapshot(&state)?;
    run_blocking("直播分组加载", move || {
        let live = cfg
            .lives
            .get(index)
            .ok_or_else(|| format!("lives 索引越界: {index}"))?;
        // 配置内嵌分组形态：{ "group": "...", "channels": [...] }（TVBox 常见的 lives 写法）
        let result = if !live.channels.is_empty() {
            let group = live
                .group
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| live.name.clone());
            let arr = serde_json::json!([{ "group": group, "channels": live.channels }]);
            vbox_core::live::parse_txt_or_m3u(&arr.to_string(), "config:lives")
        } else {
            vbox_core::live::load_live_source(&cfg, live)
        };
        result.map_err(|e| e.to_string())
    })
    .await
}

/// 拉取并解析某个频道的 EPG（URL 模板支持 {name}/{date}）
#[tauri::command]
pub async fn live_epg(
    epg_url: String,
    channel: String,
    date: String,
) -> Result<Vec<vbox_core::epg::EpgProgram>, String> {
    if epg_url.trim().is_empty() {
        return Ok(Vec::new());
    }
    run_blocking("EPG 加载", move || {
        let url = vbox_core::epg::render_epg_url(&epg_url, &channel, &date);
        vbox_core::epg::fetch_epg(&url).map_err(|e| e.to_string())
    })
    .await
}

/* ---------------- Python 运行时设置 ---------------- */

/// 当前 Python 解释器/依赖状态（设置页展示）
#[tauri::command]
pub async fn python_status() -> vbox_core::python::PythonStatus {
    // 探测会起 `python --version` 与依赖 import 子进程，必须离开主线程
    match run_blocking("Python 状态探测", || {
        Ok(vbox_core::python::python_status())
    })
    .await
    {
        Ok(status) => status,
        Err(_) => vbox_core::python::python_status(),
    }
}

/// 保存 Python 解释器（空字符串/None = 自动探测）并立即重新探测
#[tauri::command]
pub async fn set_python_path(
    state: tauri::State<'_, AppState>,
    path: Option<String>,
) -> Result<vbox_core::python::PythonStatus, String> {
    let path = path.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let config_path = state.python_config.clone();
    run_blocking("Python 路径保存", move || {
        let cfg = PythonConfig { path: path.clone() };
        let text = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
        fs::write(&config_path, text).map_err(|e| e.to_string())?;
        vbox_core::python::set_preferred(path.as_deref());
        Ok(vbox_core::python::python_status())
    })
    .await
}

/// 清空探测缓存并重新检测（不修改保存的路径）
#[tauri::command]
pub async fn refresh_python_status() -> vbox_core::python::PythonStatus {
    match run_blocking("Python 状态刷新", || {
        vbox_core::python::python_reset_cache();
        Ok(vbox_core::python::python_status())
    })
    .await
    {
        Ok(status) => status,
        Err(_) => vbox_core::python::python_status(),
    }
}

/// 当前 Node.js 运行时状态（drpy 使用）
#[tauri::command]
pub async fn node_status() -> vbox_core::node::NodeStatus {
    match run_blocking("Node 状态探测", || Ok(vbox_core::node::node_status())).await {
        Ok(status) => status,
        Err(_) => vbox_core::node::node_status(),
    }
}

/// 保存 Node.js 路径（空字符串/None = 自动探测）并立即重新检测
#[tauri::command]
pub async fn set_node_path(
    state: tauri::State<'_, AppState>,
    path: Option<String>,
) -> Result<vbox_core::node::NodeStatus, String> {
    let path = path.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let config_path = state.node_config.clone();
    run_blocking("Node 路径保存", move || {
        let cfg = NodeConfig { path: path.clone() };
        let text = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
        fs::write(&config_path, text).map_err(|e| e.to_string())?;
        vbox_core::node::set_preferred(path.as_deref());
        Ok(vbox_core::node::node_status())
    })
    .await
}

/// 清空 Node 探测缓存并重新检测（不修改保存的路径）
#[tauri::command]
pub async fn refresh_node_status() -> vbox_core::node::NodeStatus {
    match run_blocking("Node 状态刷新", || {
        vbox_core::node::reset_cache();
        Ok(vbox_core::node::node_status())
    })
    .await
    {
        Ok(status) => status,
        Err(_) => vbox_core::node::node_status(),
    }
}

/* ---------------- 站点 api 支持画像（REA 产物，供站点选择器联动） ---------------- */

/// 单个 api 的 REA 画像摘要（前端站点选择器据此区分「原生实现 / 待移植 / 不支持」，
/// 避免只按 type/api 前缀猜测导致把已原生实现的源误标「后续支持」）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteApiSupport {
    pub api: String,
    /// 已有原生实现（规则引擎 / 原生解析家族）
    pub native: bool,
    /// rulepack / script / unported / unsupported
    pub strategy: String,
    pub class: Option<String>,
    pub jars: Vec<String>,
    pub difficulty: Option<String>,
    pub reason: Option<String>,
    pub blocked_reason: Option<String>,
    /// 阻塞原因的用户可读解释（待移植/不支持时展示）
    pub blocked_hint: Option<String>,
    /// 画像覆盖的站点数
    pub sites: u32,
}

#[tauri::command]
pub fn site_api_support() -> Vec<SiteApiSupport> {
    use vbox_core::spider::catvod::registry;
    let mut rows: Vec<SiteApiSupport> = registry::profiles()
        .apis
        .iter()
        .map(|(api, profile)| SiteApiSupport {
            api: api.clone(),
            native: registry::is_native_ready(api),
            strategy: profile.strategy.clone(),
            class: profile.class.clone(),
            jars: profile.jars.clone(),
            difficulty: profile.difficulty.clone(),
            reason: profile.reason.clone(),
            blocked_reason: profile.blocked_reason.clone(),
            blocked_hint: profile
                .blocked_reason
                .as_deref()
                .map(registry::blocked_reason_hint)
                .map(str::to_string),
            sites: profile.sites,
        })
        .collect();
    rows.sort_by(|a, b| a.api.cmp(&b.api));
    rows
}

/* ---------------- 播放器（libmpv，后续阶段） ---------------- */

#[tauri::command]
pub fn play(_config: String, _url: String) -> Result<String, String> {
    Ok("播放器将在 mpv 接入后可用".into())
}

#[tauri::command]
pub fn player_control(action: String) -> Result<String, String> {
    Ok(format!("收到播放控制指令: {}", action))
}

#[cfg(test)]
mod page_tests {
    use super::slice_page;

    #[test]
    fn slice_page_covers_boundaries() {
        let items: Vec<u32> = (0..50).collect();
        assert_eq!(
            slice_page(&items, 1, 24),
            ((0..24).collect::<Vec<_>>(), true)
        );
        assert_eq!(
            slice_page(&items, 3, 24),
            ((48..50).collect::<Vec<_>>(), false)
        );
        assert_eq!(slice_page(&items, 4, 24), (Vec::new(), false));
        assert_eq!(slice_page::<u32>(&[], 1, 24), (Vec::new(), false));
    }
}
