//! 云盘聚合搜索命令（W14 P1）。
//!
//! 命令与 W14 §7.2 对齐：
//! - `cloud_search`：聚合搜索（内置 Telegram 内核 + 可选 PanSou 增强源）；
//! - `cloud_hot_searches`：首页热搜词（优先豆瓣首页磁盘缓存，失败回退内置词表）；
//! - `cloud_source_status` / `cloud_source_set` / `cloud_source_test` / `cloud_sources_update`：
//!   搜索源状态与配置（设置页「云盘搜索源」用）。
//!
//! 安全 / 隐私边界（W14 §11.2）：
//! - 搜索关键词只在进程内参与请求，不写日志；源错误日志只带源名，不带错误详情（URL 可能含关键词）；
//! - PanSou 服务地址 / token 存本机 `cloud-sources.json`（不是网盘凭据，不经过 DPAPI）；
//! - 返回值不含任何本机路径 / 凭据。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::State;

use vbox_core::cloudsearch::{
    builtin_channels, builtin_sources_json,
    kernel::SearchKernel,
    manifest::{channels_from_manifest, verify_sources_manifest, MAX_MANIFEST_BYTES},
    pansou::PanSouSource,
    tg::TgKernelSource,
    CloudSearchSource, SearchPage, SearchProvider, SearchQuery, SortBy, SourceKind, SourceStatus,
};

use crate::commands::AppState;

/// 内置内核的全局预算（W14 §11.2：全局 8s；单源超时由内核按源处理）。
const KERNEL_TIMEOUT: Duration = Duration::from_millis(8_000);

/// 首页热搜兜底词表（无豆瓣缓存时使用）。
const DEFAULT_HOT: [&str; 6] = ["三体", "流浪地球3", "沙丘2", "庆余年2", "4K合集", "纪录片"];

/// 搜索源配置文件（PanSou 为可选增强源，默认关闭）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudSourcesConfig {
    /// PanSou 兼容服务地址；空 / `None` 表示未启用。
    #[serde(default)]
    pansou_url: Option<String>,
    /// PanSou 服务 token（可选；`Authorization: Bearer`）。
    #[serde(default)]
    pansou_token: Option<String>,
    /// 远端频道清单地址（可选；配置后配合公钥启用签名清单热更新）。
    #[serde(default)]
    sources_url: Option<String>,
    /// 远端频道清单 RSA 公钥（PEM；与 `sources_url` 同时配置才启用）。
    #[serde(default)]
    sources_pubkey: Option<String>,
}

impl CloudSourcesConfig {
    fn normalized(mut self) -> Self {
        self.pansou_url = self
            .pansou_url
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .filter(|url| !url.is_empty());
        self.pansou_token = self
            .pansou_token
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        self.sources_url = self
            .sources_url
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .filter(|url| !url.is_empty());
        self.sources_pubkey = self
            .sources_pubkey
            .map(|pem| pem.trim().replace("\r\n", "\n"))
            .filter(|pem| !pem.is_empty());
        self
    }
}

/// 云盘搜索运行时状态：内核 + 源配置（配置变更时重建内核）。
pub struct CloudState {
    config_path: PathBuf,
    /// 远端频道清单缓存（验签通过后的原文；用于下次启动直接加载）。
    manifest_path: PathBuf,
    /// 豆瓣首页磁盘缓存（热搜词优先取这里，避免为了搜索页再打豆瓣接口）。
    douban_cache: PathBuf,
    inner: Mutex<CloudInner>,
}

struct CloudInner {
    config: CloudSourcesConfig,
    /// 当前实际使用的频道 handle（内置或远端签名清单）。
    channels: Vec<String>,
    /// 远端清单版本；`None` 表示正在使用内置清单。
    channels_version: Option<String>,
    kernel: SearchKernel,
}

impl CloudState {
    pub fn new(app_data_dir: &Path) -> Self {
        let config_path = app_data_dir.join("cloud-sources.json");
        let manifest_path = app_data_dir.join("cloud-sources-manifest.json");
        let douban_cache = app_data_dir.join("douban-home.json");
        let config = load_config(&config_path).unwrap_or_default().normalized();
        let (channels, channels_version) =
            load_remote_channels(&manifest_path, config.sources_pubkey.as_deref());
        let kernel = build_kernel(&config, &channels);
        Self {
            config_path,
            manifest_path,
            douban_cache,
            inner: Mutex::new(CloudInner {
                config,
                channels,
                channels_version,
                kernel,
            }),
        }
    }

    /// 豆瓣首页磁盘缓存路径（热搜词只读这里，不额外联网）。
    pub fn douban_cache_path(&self) -> &Path {
        &self.douban_cache
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CloudInner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn search(&self, query: &SearchQuery) -> Result<SearchPage, String> {
        let guard = self.lock();
        guard.kernel.search_checked(query).map_err(sanitize_error)
    }

    fn statuses(&self) -> Vec<SourceStatus> {
        let guard = self.lock();
        let channels = &guard.channels;
        let remote_version = guard.channels_version.clone();
        let mut statuses = Vec::with_capacity(2);
        let (version, detail) = match &remote_version {
            Some(version) => (
                version.clone(),
                format!("远端签名频道清单 {} 个（版本 {version}）", channels.len()),
            ),
            None => (
                builtin_version(),
                format!(
                    "内置 Telegram 频道 {} 个（{}）",
                    channels.len(),
                    if channels.is_empty() {
                        "清单为空"
                    } else {
                        "无需配置"
                    }
                ),
            ),
        };
        statuses.push(SourceStatus {
            kind: SourceKind::BuiltinKernel,
            ready: !channels.is_empty(),
            version,
            detail,
        });
        let config = &guard.config;
        let pansou_status = match pansou_source(config) {
            Ok(Some(source)) => source.health(),
            Ok(None) => SourceStatus {
                kind: SourceKind::PanSou,
                ready: false,
                version: env!("CARGO_PKG_VERSION").to_string(),
                detail: "未配置（可选增强源，默认关闭）".to_string(),
            },
            Err(err) => SourceStatus {
                kind: SourceKind::PanSou,
                ready: false,
                version: env!("CARGO_PKG_VERSION").to_string(),
                detail: err,
            },
        };
        statuses.push(pansou_status);
        statuses
    }

    fn set_source(
        &self,
        kind: &str,
        url: Option<String>,
        token: Option<String>,
    ) -> Result<Vec<SourceStatus>, String> {
        let kind = kind.trim().to_ascii_lowercase();
        if !matches!(kind.as_str(), "pansou" | "pan_sou") {
            return Err(format!("暂不支持配置搜索源 `{kind}`（当前仅支持 pansou）"));
        }
        let current = { self.lock().config.clone() };
        let config = CloudSourcesConfig {
            pansou_url: url,
            pansou_token: token,
            sources_url: current.sources_url,
            sources_pubkey: current.sources_pubkey,
        }
        .normalized();
        if let Some(url) = &config.pansou_url {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("PanSou 服务地址必须以 http:// 或 https:// 开头".to_string());
            }
        }
        save_config(&self.config_path, &config)?;
        let (channels, channels_version) =
            load_remote_channels(&self.manifest_path, config.sources_pubkey.as_deref());
        let kernel = build_kernel(&config, &channels);
        let mut guard = self.lock();
        guard.config = config;
        guard.channels = channels;
        guard.channels_version = channels_version;
        guard.kernel = kernel;
        drop(guard);
        Ok(self.statuses())
    }

    /// 单独保存远端频道清单配置（URL / 公钥），不影响 PanSou 配置。
    ///
    /// `None` 保留原值；`Some("")` 清空。
    fn set_sources_config(
        &self,
        sources_url: Option<String>,
        sources_pubkey: Option<String>,
    ) -> Result<Vec<SourceStatus>, String> {
        let current = { self.lock().config.clone() };
        let config = CloudSourcesConfig {
            pansou_url: current.pansou_url,
            pansou_token: current.pansou_token,
            sources_url: sources_url.or(current.sources_url),
            sources_pubkey: sources_pubkey.or(current.sources_pubkey),
        }
        .normalized();
        if let Some(url) = &config.sources_url {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("频道清单地址必须以 http:// 或 https:// 开头".to_string());
            }
        }
        save_config(&self.config_path, &config)?;
        let (channels, channels_version) =
            load_remote_channels(&self.manifest_path, config.sources_pubkey.as_deref());
        let kernel = build_kernel(&config, &channels);
        let mut guard = self.lock();
        guard.config = config;
        guard.channels = channels;
        guard.channels_version = channels_version;
        guard.kernel = kernel;
        drop(guard);
        Ok(self.statuses())
    }

    /// 拉取并验签远端频道清单，成功后写缓存并重建内核（W14 §11 P3）。
    fn update_sources(&self, url: Option<String>) -> Result<String, String> {
        let config = { self.lock().config.clone() };
        let target = url
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
            .or_else(|| config.sources_url.clone())
            .ok_or_else(|| {
                "未配置频道清单地址：请在设置 → 云盘搜索中填写清单 URL 与公钥".to_string()
            })?;
        if !(target.starts_with("http://") || target.starts_with("https://")) {
            return Err("频道清单地址必须以 http:// 或 https:// 开头".to_string());
        }
        let public_key = config
            .sources_pubkey
            .clone()
            .filter(|pem| !pem.trim().is_empty())
            .ok_or_else(|| {
                "未配置频道清单公钥：请在设置 → 云盘搜索中粘贴 RSA 公钥（PEM）".to_string()
            })?;

        let raw = fetch_manifest(&target)?;
        let manifest = verify_sources_manifest(&raw, &public_key)
            .map_err(|err| format!("频道清单校验失败：{err}"))?;
        let channels = channels_from_manifest(&manifest);
        if channels.is_empty() {
            return Err("远端频道清单没有启用频道".to_string());
        }
        save_manifest(&self.manifest_path, &raw)?;
        let kernel = build_kernel(&config, &channels);
        let version = manifest.version.clone();
        let count = channels.len();
        {
            let mut guard = self.lock();
            guard.channels = channels;
            guard.channels_version = Some(version.clone());
            guard.kernel = kernel;
        }
        Ok(format!(
            "频道清单已更新：{count} 个启用频道（版本 {version}）"
        ))
    }

    /// 连通性自检：PanSou 已配置时真的发一次最小搜索；未配置时只报告内置源状态。
    fn test_source(&self) -> Result<String, String> {
        let guard = self.lock();
        let config = guard.config.clone();
        drop(guard);
        let channels = { self.lock().channels.clone() };
        match pansou_source(&config)? {
            None => Ok(format!(
                "内置内核：{} 个 Telegram 频道（未配置增强源）",
                channels.len()
            )),
            Some(source) => {
                let query = SearchQuery {
                    keyword: "测试".to_string(),
                    page: 1,
                    page_size: 1,
                    providers: Vec::new(),
                    sort: SortBy::default(),
                };
                match source.search(&query) {
                    Ok(page) => Ok(format!(
                        "PanSou 可用：返回 {} 条候选（内置频道 {} 个）",
                        page.items.len(),
                        channels.len()
                    )),
                    Err(error) => Err(format!("PanSou 连接失败：{}", sanitize_error(error))),
                }
            }
        }
    }
}

fn load_config(path: &Path) -> Option<CloudSourcesConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_config(path: &Path, config: &CloudSourcesConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建配置目录失败：{err}"))?;
    }
    let text =
        serde_json::to_string_pretty(config).map_err(|err| format!("序列化配置失败：{err}"))?;
    std::fs::write(path, text).map_err(|err| format!("写入搜索源配置失败：{err}"))
}

fn builtin_version() -> String {
    serde_json::from_str::<serde_json::Value>(builtin_sources_json())
        .ok()
        .and_then(|value| {
            value
                .get("version")
                .and_then(|version| version.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn pansou_source(config: &CloudSourcesConfig) -> Result<Option<PanSouSource>, String> {
    let Some(url) = config.pansou_url.as_deref() else {
        return Ok(None);
    };
    PanSouSource::new(url, config.pansou_token.clone())
        .map(Some)
        .map_err(|err| format!("PanSou 源初始化失败：{err}"))
}

/// 读取远端清单缓存并验签；未配置公钥 / 缓存缺失 / 校验失败时回退内置清单。
fn load_remote_channels(path: &Path, public_key: Option<&str>) -> (Vec<String>, Option<String>) {
    let builtin = builtin_channels();
    let Some(public_key) = public_key.filter(|pem| !pem.trim().is_empty()) else {
        return (builtin, None);
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (builtin, None);
    };
    match verify_sources_manifest(&raw, public_key) {
        Ok(manifest) => {
            let channels = channels_from_manifest(&manifest);
            if channels.is_empty() {
                eprintln!("[vbox] 远端频道清单没有启用频道，回退内置清单");
                return (builtin, None);
            }
            (channels, Some(manifest.version))
        }
        Err(err) => {
            // 错误信息不含 URL / 清单内容，仅类别 + 可读原因。
            eprintln!("[vbox] 远端频道清单校验失败，回退内置清单：{err}");
            (builtin, None)
        }
    }
}

/// 抓取远端清单原文（12s 超时、256KiB 上限；失败信息不回显 URL）。
fn fetch_manifest(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(concat!(
            "VBox/",
            env!("CARGO_PKG_VERSION"),
            " cloudsearch-manifest"
        ))
        .build()
        .map_err(|_| "创建频道清单请求失败".to_string())?;
    let response = client
        .get(url)
        .send()
        .map_err(|_| "频道清单请求失败（请检查网络后重试）".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "频道清单请求失败（HTTP {}）",
            response.status().as_u16()
        ));
    }
    if let Some(length) = response.content_length() {
        if length as usize > MAX_MANIFEST_BYTES {
            return Err(format!(
                "远端频道清单过大（上限 {MAX_MANIFEST_BYTES} 字节）"
            ));
        }
    }
    let mut buffer = Vec::new();
    response
        .take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| "频道清单读取失败".to_string())?;
    if buffer.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "远端频道清单过大（上限 {MAX_MANIFEST_BYTES} 字节）"
        ));
    }
    String::from_utf8(buffer).map_err(|_| "频道清单不是合法 UTF-8".to_string())
}

fn save_manifest(path: &Path, raw: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建清单缓存目录失败：{err}"))?;
    }
    std::fs::write(path, raw).map_err(|err| format!("写入频道清单缓存失败：{err}"))
}

fn build_kernel(config: &CloudSourcesConfig, channels: &[String]) -> SearchKernel {
    let mut kernel = SearchKernel::default().with_timeout(KERNEL_TIMEOUT);
    match TgKernelSource::with_default_client(channels.to_vec()) {
        Ok(source) => {
            kernel.add_source(Arc::new(source));
        }
        Err(err) => eprintln!("[vbox] 云盘内置搜索源初始化失败：{err}"),
    }
    match pansou_source(config) {
        Ok(Some(source)) => {
            kernel.add_source(Arc::new(source));
        }
        Ok(None) => {}
        Err(err) => eprintln!("[vbox] 云盘增强搜索源初始化失败：{err}"),
    }
    kernel
}

/// 错误信息脱敏：底层 HTTP 错误常带完整 URL（可能含关键词），这里只保留错误类别。
fn sanitize_error(error: vbox_core::error::CoreError) -> String {
    use vbox_core::error::CoreError;
    match error {
        CoreError::Config(message) => format!("云盘搜索配置错误：{message}"),
        CoreError::Http(_) => "云盘搜索网络请求失败（请检查网络或搜索源配置）".to_string(),
        CoreError::Json { .. } => "云盘搜索响应解析失败".to_string(),
        CoreError::Io(_) => "云盘搜索本地读写失败".to_string(),
        other => format!("云盘搜索失败：{other}"),
    }
}

/* ---------------- Tauri commands ---------------- */

/// 聚合搜索（内置内核 + 可选增强源）。
#[tauri::command]
pub fn cloud_search(
    state: State<'_, AppState>,
    keyword: String,
    page: Option<u32>,
    page_size: Option<u32>,
    providers: Option<Vec<String>>,
    sort: Option<String>,
) -> Result<SearchPage, String> {
    let providers = providers
        .unwrap_or_default()
        .into_iter()
        .map(|provider| SearchProvider::from_str_lossy(&provider))
        .collect();
    let query = SearchQuery {
        keyword,
        page: page.unwrap_or(1),
        page_size: page_size.unwrap_or(20),
        providers,
        sort: match sort.as_deref() {
            Some("time") => SortBy::Time,
            _ => SortBy::Relevance,
        },
    };
    let query = query.normalized().map_err(sanitize_error)?;
    state.cloud.search(&query)
}

/// 首页热搜词：优先豆瓣首页磁盘缓存（零额外网络），失败回退内置词表。
#[tauri::command]
pub fn cloud_hot_searches(state: State<'_, AppState>) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    if let Some(home) = vbox_core::douban::cached_home(&state.cloud.douban_cache_path()) {
        for item in home.carousel.iter().take(8) {
            let title = item.title.trim();
            if !title.is_empty() && !words.iter().any(|existing| existing == title) {
                words.push(title.to_string());
            }
        }
    }
    for fallback in DEFAULT_HOT {
        if words.len() >= 8 {
            break;
        }
        if !words.iter().any(|existing| existing == fallback) {
            words.push(fallback.to_string());
        }
    }
    words.truncate(8);
    words
}

/// 当前搜索源状态（内置内核 + PanSou）。
#[tauri::command]
pub fn cloud_source_status(state: State<'_, AppState>) -> Vec<SourceStatus> {
    state.cloud.statuses()
}

/// 配置搜索源（仅 PanSou；`url` 为空表示关闭）。
#[tauri::command]
pub fn cloud_source_set(
    state: State<'_, AppState>,
    kind: String,
    url: Option<String>,
    token: Option<String>,
) -> Result<Vec<SourceStatus>, String> {
    state.cloud.set_source(&kind, url, token)
}

/// 保存远端频道清单配置（列表 URL + RSA 公钥 PEM），不影响 PanSou 设置。
#[tauri::command]
pub fn cloud_sources_config_set(
    state: State<'_, AppState>,
    sources_url: Option<String>,
    sources_pubkey: Option<String>,
) -> Result<Vec<SourceStatus>, String> {
    state.cloud.set_sources_config(sources_url, sources_pubkey)
}

/// 搜索源自检（PanSou 已配置时发一次最小搜索）。
#[tauri::command]
pub fn cloud_source_test(state: State<'_, AppState>) -> Result<String, String> {
    state.cloud.test_source()
}

/// 远端频道清单热更新（W14 §11 P3）：抓取 → RSA 验签 → 缓存 → 重建内核。
///
/// `url` 为空时使用设置页保存的清单地址；必须已配置 RSA 公钥（PEM）。
#[tauri::command]
pub fn cloud_sources_update(
    state: State<'_, AppState>,
    url: Option<String>,
) -> Result<String, String> {
    state.cloud.update_sources(url)
}

/* ---------------- 只读云盘管理（W14 P2 / §9） ---------------- */

/// 列云盘目录（`parent_id` 为空 = 根目录）。
///
/// 凭据按 `account_id` 从本机加密存储读取，只在进程内交给 [`DriveClient`]；
/// 返回值只有目录项，不含 cookie/token。
#[tauri::command]
pub fn drive_list(
    state: State<'_, AppState>,
    account_id: String,
    parent_id: Option<String>,
) -> Result<Vec<vbox_core::drive::DriveEntry>, String> {
    let client = drive_client(&state, &account_id)?;
    let parent = parent_id.as_deref().filter(|p| !p.trim().is_empty());
    vbox_core::drive::DriveRead::list(&client, parent).map_err(sanitize_drive_error)
}

/// 取云盘文件下载直链（交给前端播放器/下载中心；不含 cookie/token）。
#[tauri::command]
pub fn drive_download(
    state: State<'_, AppState>,
    account_id: String,
    entry_id: String,
) -> Result<String, String> {
    let client = drive_client(&state, &account_id)?;
    vbox_core::drive::DriveRead::download_url(&client, &entry_id).map_err(sanitize_drive_error)
}

/// 云盘搜索结果 → 播放直链（W14 P3：分享展开 + W13 登录凭据）。
///
/// 使用云盘首页「账号管理」中当前活跃的登录凭据（overlay）；未登录时返回可操作错误，
/// 前端据此提示去登录并回退「打开原页」。返回值只含直链与防盗链请求头，不含凭据。
#[tauri::command]
pub fn cloud_resolve_play(
    share_url: String,
    password: Option<String>,
) -> Result<vbox_core::cloudplay::ResolvedPlay, String> {
    vbox_core::cloudplay::resolve_play(&share_url, password.as_deref())
        .map_err(sanitize_drive_error)
}

/// 按 `account_id` 解析凭据并构造只读云盘客户端。
fn drive_client(
    state: &AppState,
    account_id: &str,
) -> Result<vbox_core::drive::DriveClient, String> {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        return Err("缺少 account_id".to_string());
    }
    let cred = state
        .pan_credentials
        .credential(account_id)?
        .ok_or_else(|| {
            format!("未找到账号 {account_id}：请在云盘首页右上角「账号管理」重新登录")
        })?;
    vbox_core::drive::DriveClient::from_credential(&cred).map_err(sanitize_drive_error)
}

/// 云盘错误脱敏：Config 已含可读指引；HTTP/JSON 不回显底层 URL。
fn sanitize_drive_error(error: vbox_core::error::CoreError) -> String {
    use vbox_core::error::CoreError;
    match error {
        CoreError::Config(message) => message,
        CoreError::Http(_) => "云盘请求失败（请检查网络后重试）".to_string(),
        CoreError::Json { .. } => "云盘响应解析失败".to_string(),
        CoreError::Api { code, msg } => format!("云盘接口返回错误：{msg}（code={code}）"),
        CoreError::Io(_) => "云盘本地读写失败".to_string(),
        other => format!("云盘操作失败：{other}"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use vbox_core::error::CoreError;

    fn state_dir(tag: &str) -> PathBuf {
        let dir = std::env::current_dir()
            .expect("cwd")
            .join("target")
            .join(format!("cloud-state-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn default_statuses_have_builtin_and_pansou() {
        let state = CloudState::new(&state_dir("status"));
        let statuses = state.statuses();
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].kind, SourceKind::BuiltinKernel);
        assert!(statuses[0].ready, "内置频道清单应可用");
        assert_eq!(statuses[1].kind, SourceKind::PanSou);
        assert!(!statuses[1].ready, "默认不配置 PanSou");
    }

    #[test]
    fn source_config_round_trip_and_disable() {
        let dir = state_dir("config");
        let state = CloudState::new(&dir);
        let statuses = state
            .set_source(
                "pansou",
                Some("  http://127.0.0.1:8888/ ".to_string()),
                Some(" token-x ".to_string()),
            )
            .expect("set pansou");
        assert!(statuses[1].ready);
        let saved = std::fs::read_to_string(dir.join("cloud-sources.json")).expect("saved config");
        assert!(saved.contains("http://127.0.0.1:8888"));
        assert!(saved.contains("token-x"));

        // 关闭增强源后配置应清空。
        let statuses = state.set_source("pansou", None, None).expect("disable");
        assert!(!statuses[1].ready);
        let saved = std::fs::read_to_string(dir.join("cloud-sources.json")).expect("saved config");
        assert!(!saved.contains("token-x"));
    }

    #[test]
    fn source_set_rejects_unknown_kind_and_bad_url() {
        let state = CloudState::new(&state_dir("reject"));
        assert!(state.set_source("pd", None, None).is_err());
        assert!(state
            .set_source("pansou", Some("ftp://x".to_string()), None)
            .is_err());
    }

    #[test]
    fn sources_update_requires_url_and_public_key() {
        let state = CloudState::new(&state_dir("sources"));
        let err = state.update_sources(None).expect_err("缺少 URL 应失败");
        assert!(err.contains("未配置频道清单地址"), "{err}");
        // 只配置 URL、没有公钥时，也应给出明确指引（且不访问网络）。
        assert!(state
            .set_sources_config(Some("http://127.0.0.1:9/manifest.json".to_string()), None)
            .is_ok());
        let err = state.update_sources(None).expect_err("缺少公钥应失败");
        assert!(err.contains("未配置频道清单公钥"), "{err}");
        // 非 http(s) 地址直接拒绝。
        let err = state
            .update_sources(Some("ftp://example.com/m.json".to_string()))
            .expect_err("非 http 地址应失败");
        assert!(err.contains("http://"), "{err}");
    }

    #[test]
    fn sources_settings_are_persisted_and_cleared() {
        let dir = state_dir("sources-config");
        let state = CloudState::new(&dir);
        // 先保存一份 PanSou 配置，确认写频道清单配置不会把它清掉。
        state
            .set_source("pansou", Some("http://127.0.0.1:8888".to_string()), None)
            .expect("set pansou");
        state
            .set_sources_config(
                Some("https://example.com/sources.json".to_string()),
                Some("-----BEGIN PUBLIC KEY-----\nxx\n-----END PUBLIC KEY-----".to_string()),
            )
            .expect("save sources settings");
        let saved = std::fs::read_to_string(dir.join("cloud-sources.json")).expect("config");
        assert!(saved.contains("https://example.com/sources.json"));
        assert!(saved.contains("sourcesPubkey"));
        assert!(saved.contains("127.0.0.1:8888"), "PanSou 配置应被保留");
        // 清空：传空字符串应删除对应字段。
        state
            .set_sources_config(Some(String::new()), Some(String::new()))
            .expect("clear sources settings");
        let saved = std::fs::read_to_string(dir.join("cloud-sources.json")).expect("config");
        assert!(!saved.contains("example.com"));
        assert!(
            saved.contains("127.0.0.1:8888"),
            "清空清单配置不应影响 PanSou"
        );
    }

    #[test]
    fn sanitize_error_does_not_echo_url() {
        let message = sanitize_error(CoreError::Config("空关键词".to_string()));
        assert!(message.contains("空关键词"));
        // HTTP 错误只返回固定文案（底层 URL 可能含关键词）。
        let error = reqwest::blocking::get("http://127.0.0.1:9/none?kw=secret").unwrap_err();
        let message = sanitize_error(CoreError::Http(error));
        assert!(!message.contains("secret"));
        assert!(message.contains("网络请求失败"));
    }
}
