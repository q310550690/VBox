//! 云盘聚合搜索（W14）后端内核骨架。
//!
//! 模块划分：
//! - `mod.rs`：数据类型、`CloudSearchSource` trait、`SearchQuery` 归一化
//! - `detect`：分享链接识别（优先复用 `crate::pan::parse_share_link`）
//! - `rank`：去重合并、provider 过滤、排序、分页
//! - `cache`：进程内 TTL + FIFO 缓存
//! - `tg`：Telegram 公开频道检索（`t.me/s/{handle}?q=`）
//! - `pansou`：PanSou 兼容 HTTP 增强源（可选，容错解析）
//! - `kernel`：编排 / 并发 / 超时 / 降级 / 缓存
//! - `sources.json`：内置频道清单（可被远端签名清单热更新）
//!
//! 本模块不依赖 Tauri，保持 blocking 语义；`src-tauri` 的 commands 只做薄封装。

pub mod cache;
pub mod detect;
pub mod kernel;
pub mod manifest;
pub mod pansou;
pub mod rank;
pub mod tg;

use crate::error::CoreError;
use serde::{Deserialize, Serialize};

/// 云盘 / 磁力服务商标识。
///
/// 序列化名与前端 `providers` 入参一致（snake_case）：
/// `quark / aliyun / p115 / xunlei / pikpak / uc / baidu / tianyi / mobile / p123 / magnet / ed2k / other`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProvider {
    Quark,
    Aliyun,
    P115,
    Xunlei,
    // serde 的 snake_case 会把 `PikPak` 拆成 `pik_pak`，这里显式覆盖为前端约定的 `pikpak`。
    #[serde(rename = "pikpak")]
    PikPak,
    Uc,
    Baidu,
    Tianyi,
    Mobile,
    P123,
    Magnet,
    Ed2k,
    Other,
}

impl SearchProvider {
    /// 全部枚举值（顺序即前端筛选器的默认展示顺序）。
    pub const ALL: [SearchProvider; 13] = [
        SearchProvider::Quark,
        SearchProvider::Aliyun,
        SearchProvider::P115,
        SearchProvider::Xunlei,
        SearchProvider::PikPak,
        SearchProvider::Uc,
        SearchProvider::Baidu,
        SearchProvider::Tianyi,
        SearchProvider::Mobile,
        SearchProvider::P123,
        SearchProvider::Magnet,
        SearchProvider::Ed2k,
        SearchProvider::Other,
    ];

    /// 稳定的小写标识（用于 id 前缀、缓存键与降级源名）。
    pub fn as_str(self) -> &'static str {
        match self {
            SearchProvider::Quark => "quark",
            SearchProvider::Aliyun => "aliyun",
            SearchProvider::P115 => "p115",
            SearchProvider::Xunlei => "xunlei",
            SearchProvider::PikPak => "pikpak",
            SearchProvider::Uc => "uc",
            SearchProvider::Baidu => "baidu",
            SearchProvider::Tianyi => "tianyi",
            SearchProvider::Mobile => "mobile",
            SearchProvider::P123 => "p123",
            SearchProvider::Magnet => "magnet",
            SearchProvider::Ed2k => "ed2k",
            SearchProvider::Other => "other",
        }
    }

    /// 宽松解析：大小写不敏感，兼容常见别名；无法识别时返回 [`SearchProvider::Other`]。
    pub fn from_str_lossy(raw: &str) -> Self {
        let key = raw.trim().to_ascii_lowercase();
        match key.as_str() {
            "quark" | "quarkpan" | "夸克" => SearchProvider::Quark,
            "aliyun" | "ali" | "alipan" | "aliyundrive" | "阿里云盘" => SearchProvider::Aliyun,
            "p115" | "115" | "115pan" => SearchProvider::P115,
            "xunlei" | "thunder" | "迅雷" => SearchProvider::Xunlei,
            "pikpak" | "mypikpak" => SearchProvider::PikPak,
            "uc" | "ucdrive" | "uc网盘" => SearchProvider::Uc,
            "baidu" | "bd" | "baidupan" | "百度网盘" => SearchProvider::Baidu,
            "tianyi" | "189" | "cloud189" | "天翼云盘" => SearchProvider::Tianyi,
            "mobile" | "139" | "caiyun" | "移动云盘" => SearchProvider::Mobile,
            "p123" | "123" | "123pan" | "123云盘" => SearchProvider::P123,
            "magnet" | "bt" | "磁力" => SearchProvider::Magnet,
            "ed2k" | "电驴" => SearchProvider::Ed2k,
            _ => SearchProvider::Other,
        }
    }

    /// 网盘分享链接可继续走 `pan/*` 解析播放；磁力 / ED2K 在播放器接入前不可播放。
    pub fn is_playable(self) -> bool {
        !matches!(self, SearchProvider::Magnet | SearchProvider::Ed2k)
    }
}

impl std::fmt::Display for SearchProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 排序方式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    /// 关键词相关性（默认）
    #[default]
    Relevance,
    /// 发布时间倒序
    Time,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    20
}

/// 搜索请求（§7.1）。`page` 1 基，`page_size` 默认 20 / 上限 50，`providers` 为空表示全部。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    #[serde(default)]
    pub keyword: String,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default)]
    pub providers: Vec<SearchProvider>,
    #[serde(default)]
    pub sort: SortBy,
}

impl Default for SearchQuery {
    fn default() -> Self {
        SearchQuery {
            keyword: String::new(),
            page: default_page(),
            page_size: default_page_size(),
            providers: Vec::new(),
            sort: SortBy::default(),
        }
    }
}

impl SearchQuery {
    /// `page_size` 上限（§7.1）。
    pub const MAX_PAGE_SIZE: u32 = 50;

    /// 归一化：keyword 去首尾空白（为空报 `CoreError::Config`）、page 至少 1、
    /// page_size 收敛到 1..=50、providers 去重（保持原顺序）。
    pub fn normalized(&self) -> Result<SearchQuery, CoreError> {
        let keyword = self.keyword.trim();
        if keyword.is_empty() {
            return Err(CoreError::Config("云盘搜索关键词不能为空".to_string()));
        }
        let mut providers: Vec<SearchProvider> = Vec::with_capacity(self.providers.len());
        for provider in &self.providers {
            if !providers.contains(provider) {
                providers.push(*provider);
            }
        }
        Ok(SearchQuery {
            keyword: keyword.to_string(),
            page: self.page.max(1),
            page_size: self.page_size.clamp(1, Self::MAX_PAGE_SIZE),
            providers,
            sort: self.sort,
        })
    }
}

/// 单条搜索结果（§7.1）。`id = provider + ":" + share_id`，跨源同 id 由 `rank` 合并。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub id: String,
    pub title: String,
    pub provider: SearchProvider,
    pub share_url: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Unix 秒；未知为 `None`。
    #[serde(default)]
    pub published_at: Option<i64>,
    /// 来源频道（如 Telegram handle），合并时取并集。
    #[serde(default)]
    pub sources: Vec<String>,
    /// 磁力 / ED2K 为 `false`（播放器未接入前不可直接播放）。
    #[serde(default)]
    pub playable: bool,
}

/// 一页搜索结果（§7.1）。`total` 是去重 / 过滤后的可见总数，`items` 已按请求分页切片。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub items: Vec<SearchItem>,
    pub total: usize,
    /// 部分源失败 / 超时（`degraded_sources` 给出名字）。
    pub partial: bool,
    pub elapsed_ms: u64,
    pub degraded_sources: Vec<String>,
}

impl SearchPage {
    /// 空结果页（用于错误兜底 / 空关键词）。
    pub fn empty() -> Self {
        SearchPage {
            items: Vec::new(),
            total: 0,
            partial: false,
            elapsed_ms: 0,
            degraded_sources: Vec::new(),
        }
    }
}

/// 搜索源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// 内置内核（Telegram 频道 / 内置规则）
    BuiltinKernel,
    /// PanSou 兼容 HTTP 端点（本机 Docker 或远程 / 自建）
    #[serde(alias = "pansou")]
    PanSou,
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceKind::BuiltinKernel => f.write_str("builtin_kernel"),
            SourceKind::PanSou => f.write_str("pansou"),
        }
    }
}

/// 搜索源健康状态（§7.1 / `cloud_source_status` 返回体）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub kind: SourceKind,
    pub ready: bool,
    pub version: String,
    pub detail: String,
}

/// 聚合搜索源。实现必须 `Send + Sync`（内核用后台线程并行调用）。
pub trait CloudSearchSource: Send + Sync {
    /// 返回候选结果（不要求已分页；分页 / 排序 / 去重由 `rank` 统一处理）。
    fn search(&self, q: &SearchQuery) -> Result<SearchPage, CoreError>;

    /// 健康检查（不发起网络请求）。
    fn health(&self) -> SourceStatus;

    /// 源名字（用于 `degraded_sources` 与日志）。
    fn name(&self) -> String {
        "cloudsearch".to_string()
    }
}

/// 内置频道清单原文（编译期内嵌；桌面端可据此展示「内置源」版本）。
pub fn builtin_sources_json() -> &'static str {
    include_str!("sources.json")
}

/// 内置 Telegram 频道 handle 列表（仅 `enabled = true`；解析失败回退空列表）。
///
/// 清单文件格式：
/// ```json
/// {"version":"2026.11.17","updated_at":0,
///  "channels":[{"handle":"yunpan_channel","enabled":true}]}
/// ```
pub fn builtin_channels() -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct SourcesFile {
        #[serde(default)]
        channels: Vec<ChannelEntry>,
    }

    #[derive(serde::Deserialize)]
    struct ChannelEntry {
        #[serde(default)]
        handle: String,
        #[serde(default = "default_enabled")]
        enabled: bool,
    }

    fn default_enabled() -> bool {
        true
    }

    let Ok(parsed) = serde_json::from_str::<SourcesFile>(builtin_sources_json()) else {
        return Vec::new();
    };

    let mut handles: Vec<String> = Vec::new();
    for channel in parsed.channels {
        if !channel.enabled {
            continue;
        }
        let handle = channel.handle.trim().trim_start_matches('@');
        if handle.is_empty() || handles.iter().any(|existing| existing.as_str() == handle) {
            continue;
        }
        handles.push(handle.to_string());
    }
    handles
}

#[cfg(test)]
mod builtin_sources_tests {
    use super::*;

    #[test]
    fn builtin_sources_json_is_valid() {
        let value: serde_json::Value =
            serde_json::from_str(builtin_sources_json()).expect("sources.json 必须是合法 JSON");
        assert!(value.get("version").and_then(|v| v.as_str()).is_some());
        assert_eq!(value.get("updated_at").and_then(|v| v.as_u64()), Some(0));
        let channels = value
            .get("channels")
            .and_then(|v| v.as_array())
            .expect("channels 必须是数组");
        assert!(
            (6..=10).contains(&channels.len()),
            "内置频道数量应在 6~10 之间，实际 {}",
            channels.len()
        );
        for channel in channels {
            let handle = channel
                .get("handle")
                .and_then(|v| v.as_str())
                .expect("每个频道必须有 handle");
            assert!(!handle.trim().is_empty());
            assert!(
                channel.get("enabled").and_then(|v| v.as_bool()).is_some(),
                "每个频道必须有 enabled"
            );
        }
    }

    #[test]
    fn builtin_channels_filters_disabled_and_normalizes() {
        let handles = builtin_channels();
        assert!(!handles.is_empty());
        assert!((6..=10).contains(&handles.len()));
        assert!(
            !handles.iter().any(|handle| handle == "legacy_channel"),
            "enabled=false 的频道不应返回"
        );
        assert!(handles.iter().all(|handle| !handle.starts_with('@')));
        // handle 不重复。
        let mut unique = handles.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), handles.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_serde_round_trip() {
        for provider in SearchProvider::ALL {
            let json = serde_json::to_string(&provider).expect("serialize");
            assert_eq!(
                json,
                format!("\"{}\"", provider.as_str()),
                "provider {provider:?} 的 serde 名应与 as_str 一致"
            );
            let back: SearchProvider = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, provider);
        }
        // 关键固定值（前端契约）。
        assert_eq!(
            serde_json::to_string(&SearchProvider::P115).unwrap(),
            "\"p115\""
        );
        assert_eq!(
            serde_json::to_string(&SearchProvider::PikPak).unwrap(),
            "\"pikpak\""
        );
        assert_eq!(
            serde_json::to_string(&SearchProvider::Magnet).unwrap(),
            "\"magnet\""
        );
    }

    #[test]
    fn provider_from_str_lossy_aliases() {
        assert_eq!(
            SearchProvider::from_str_lossy(" QUARK "),
            SearchProvider::Quark
        );
        assert_eq!(
            SearchProvider::from_str_lossy("Ali"),
            SearchProvider::Aliyun
        );
        assert_eq!(SearchProvider::from_str_lossy("115"), SearchProvider::P115);
        assert_eq!(
            SearchProvider::from_str_lossy("PikPak"),
            SearchProvider::PikPak
        );
        assert_eq!(SearchProvider::from_str_lossy("uc"), SearchProvider::Uc);
        assert_eq!(
            SearchProvider::from_str_lossy("189"),
            SearchProvider::Tianyi
        );
        assert_eq!(
            SearchProvider::from_str_lossy("139"),
            SearchProvider::Mobile
        );
        assert_eq!(
            SearchProvider::from_str_lossy("123pan"),
            SearchProvider::P123
        );
        assert_eq!(
            SearchProvider::from_str_lossy("磁力"),
            SearchProvider::Magnet
        );
        assert_eq!(SearchProvider::from_str_lossy("ed2k"), SearchProvider::Ed2k);
        assert_eq!(
            SearchProvider::from_str_lossy("未知"),
            SearchProvider::Other
        );
        assert_eq!(SearchProvider::from_str_lossy(""), SearchProvider::Other);
    }

    #[test]
    fn provider_playable_flags() {
        assert!(SearchProvider::Quark.is_playable());
        assert!(SearchProvider::Aliyun.is_playable());
        assert!(!SearchProvider::Magnet.is_playable());
        assert!(!SearchProvider::Ed2k.is_playable());
    }

    #[test]
    fn normalized_clamps_and_trims() {
        let q = SearchQuery {
            keyword: "  电影  ".to_string(),
            page: 0,
            page_size: 999,
            providers: vec![
                SearchProvider::Quark,
                SearchProvider::Baidu,
                SearchProvider::Quark,
            ],
            sort: SortBy::Time,
        };
        let n = q.normalized().expect("normalized");
        assert_eq!(n.keyword, "电影");
        assert_eq!(n.page, 1);
        assert_eq!(n.page_size, SearchQuery::MAX_PAGE_SIZE);
        assert_eq!(
            n.providers,
            vec![SearchProvider::Quark, SearchProvider::Baidu]
        );
        assert_eq!(n.sort, SortBy::Time);
    }

    #[test]
    fn normalized_page_size_lower_bound() {
        let q = SearchQuery {
            keyword: "x".into(),
            page_size: 0,
            ..SearchQuery::default()
        };
        assert_eq!(q.normalized().unwrap().page_size, 1);
    }

    #[test]
    fn normalized_rejects_blank_keyword() {
        for keyword in ["", "   ", "\t\n"] {
            let q = SearchQuery {
                keyword: keyword.to_string(),
                ..SearchQuery::default()
            };
            assert!(
                matches!(q.normalized(), Err(CoreError::Config(_))),
                "空关键词应返回 CoreError::Config: {keyword:?}"
            );
        }
    }

    #[test]
    fn query_defaults_from_partial_json() {
        let q: SearchQuery = serde_json::from_str(r#"{"keyword":"abc"}"#).expect("query");
        assert_eq!(q.page, 1);
        assert_eq!(q.page_size, 20);
        assert_eq!(q.sort, SortBy::Relevance);
        assert!(q.providers.is_empty());
        assert_eq!(serde_json::to_string(&SortBy::Time).unwrap(), "\"time\"");
        assert_eq!(
            serde_json::to_string(&SortBy::Relevance).unwrap(),
            "\"relevance\""
        );
    }

    #[test]
    fn source_kind_serde_and_display() {
        assert_eq!(
            serde_json::to_string(&SourceKind::BuiltinKernel).unwrap(),
            "\"builtin_kernel\""
        );
        let pansou: SourceKind = serde_json::from_str("\"pansou\"").expect("alias pansou");
        assert_eq!(pansou, SourceKind::PanSou);
        assert_eq!(SourceKind::PanSou.to_string(), "pansou");
        assert_eq!(SourceKind::BuiltinKernel.to_string(), "builtin_kernel");
    }

    #[test]
    fn search_item_json_uses_camel_case() {
        let item = SearchItem {
            id: "quark:abc".into(),
            title: "t".into(),
            provider: SearchProvider::Quark,
            share_url: "https://pan.quark.cn/s/abc".into(),
            password: Some("xy9z".into()),
            size: Some(1024),
            published_at: Some(1_700_000_000),
            sources: vec!["channel".into()],
            playable: true,
        };
        let value = serde_json::to_value(&item).expect("json");
        assert_eq!(
            value["shareUrl"],
            serde_json::json!("https://pan.quark.cn/s/abc")
        );
        assert_eq!(value["publishedAt"], serde_json::json!(1_700_000_000_i64));
        assert_eq!(value["provider"], serde_json::json!("quark"));
    }
}
