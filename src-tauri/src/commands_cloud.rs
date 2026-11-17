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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::State;

use vbox_core::cloudsearch::{
    CloudSearchSource, SearchPage, SearchProvider, SearchQuery, SortBy, SourceKind, SourceStatus,
    builtin_channels, builtin_sources_json, kernel::SearchKernel, pansou::PanSouSource,
    tg::TgKernelSource,
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
        self
    }
}

/// 云盘搜索运行时状态：内核 + 源配置（配置变更时重建内核）。
pub struct CloudState {
    config_path: PathBuf,
    /// 豆瓣首页磁盘缓存（热搜词优先取这里，避免为了搜索页再打豆瓣接口）。
    douban_cache: PathBuf,
    inner: Mutex<CloudInner>,
}

struct CloudInner {
    config: CloudSourcesConfig,
    kernel: SearchKernel,
}

impl CloudState {
    pub fn new(app_data_dir: &Path) -> Self {
        let config_path = app_data_dir.join("cloud-sources.json");
        let douban_cache = app_data_dir.join("douban-home.json");
        let config = load_config(&config_path).unwrap_or_default().normalized();
        let kernel = build_kernel(&config);
        Self {
            config_path,
            douban_cache,
            inner: Mutex::new(CloudInner { config, kernel }),
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
        let channels = builtin_channels();
        let mut statuses = Vec::with_capacity(2);
        statuses.push(SourceStatus {
            kind: SourceKind::BuiltinKernel,
            ready: !channels.is_empty(),
            version: builtin_version(),
            detail: format!(
                "内置 Telegram 频道 {} 个（{}）",
                channels.len(),
                if channels.is_empty() {
                    "清单为空"
                } else {
                    "无需配置"
                }
            ),
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
        let config = CloudSourcesConfig {
            pansou_url: url,
            pansou_token: token,
        }
        .normalized();
        if let Some(url) = &config.pansou_url {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("PanSou 服务地址必须以 http:// 或 https:// 开头".to_string());
            }
        }
        save_config(&self.config_path, &config)?;
        let kernel = build_kernel(&config);
        let mut guard = self.lock();
        guard.config = config;
        guard.kernel = kernel;
        drop(guard);
        Ok(self.statuses())
    }

    /// 连通性自检：PanSou 已配置时真的发一次最小搜索；未配置时只报告内置源状态。
    fn test_source(&self) -> Result<String, String> {
        let guard = self.lock();
        let config = guard.config.clone();
        drop(guard);
        let channels = builtin_channels();
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
    let text = serde_json::to_string_pretty(config).map_err(|err| format!("序列化配置失败：{err}"))?;
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

fn build_kernel(config: &CloudSourcesConfig) -> SearchKernel {
    let mut kernel = SearchKernel::default().with_timeout(KERNEL_TIMEOUT);
    match TgKernelSource::with_default_client(builtin_channels()) {
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

/// 搜索源自检（PanSou 已配置时发一次最小搜索）。
#[tauri::command]
pub fn cloud_source_test(state: State<'_, AppState>) -> Result<String, String> {
    state.cloud.test_source()
}

/// 远端频道清单热更新（P3 接入；当前使用编译期内置清单）。
#[tauri::command]
pub fn cloud_sources_update(_state: State<'_, AppState>) -> Result<String, String> {
    Err("远端频道清单热更新将在 W14 P3 接入，当前使用内置清单".to_string())
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
        assert!(state.set_source("pansou", Some("ftp://x".to_string()), None).is_err());
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
