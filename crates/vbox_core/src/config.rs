//! 订阅加载：URL 归一化 → 拉取 → 解密 → 相对路径修正 → 线路/多仓展开 → TvConfig
//!
//! 对齐 jun 版 TVBoxOS 的真实语义：
//! - 顶层只有 `urls`、没有 `sites` → "线路合集"（一次激活一条线路，不做 sites 合并）
//! - 顶层有 `sites` → 普通配置（`urls` 只保留给 UI 展示，不参与加载）
//! - `;pk;KEY`、`[A-Za-z0-9]{8}**`+Base64、`2423` AES-CBC、`;pk;` AES-ECB 四种配置包装
//! - `"./`、`"../` 按配置 URL 修正为绝对地址

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crypto::decode_config_body;
use crate::error::{CoreError, Result};
use crate::http;
use crate::model::*;
use crate::normalize;
use crate::url_norm::{self, InputKind};

/* ============================================================
 * 公开类型
 * ============================================================ */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigLine {
    #[serde(default)]
    pub name: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ConfigKind {
    #[serde(rename = "single")]
    Single,
    /// 线路合集/多仓（前端沿用 "multi" 命名）
    #[serde(rename = "multi")]
    Lines,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInspection {
    pub kind: ConfigKind,
    pub lines: Vec<ConfigLine>,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedConfig {
    /// 用户原始输入（合集入口时是合集 URL；单配置时是自身）
    pub input: String,
    /// 合集的入口 URL；单配置时等于 input
    pub source_url: String,
    /// 实际请求地址（已去除 ;pk;）
    pub request_url: String,
    /// 当前激活的线路；单配置时等于 input
    pub active_line: String,
    pub kind: ConfigKind,
    /// 线路列表；单配置时只有一项
    pub lines: Vec<ConfigLine>,
    /// 当前线路解析出的真实配置
    pub config: TvConfig,
    /// 解包格式（plain / base64-marker / aes-cbc-2423 / aes-ecb-pk）
    pub format: String,
    pub cache_hit: bool,
    /// 兼容旧调用方：原始解码后的 JSON（不返回给前端时可忽略）
    #[serde(skip)]
    pub decoded: String,
}

#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// 是否读取/写入配置缓存
    pub use_cache: bool,
    /// 缓存目录（Tauri 传 app_data_dir/config-cache；CLI 默认不开启）
    pub cache_dir: Option<PathBuf>,
    /// 缓存有效期（秒）；0 表示不过期
    pub cache_ttl_secs: u64,
    /// 本地文件服务基址（例如 http://127.0.0.1:12345），用于 clan://
    pub local_base: Option<String>,
    /// 强制忽略缓存
    pub force_refresh: bool,
    /// 线路合集最大跳转深度
    pub max_depth: usize,
    /// 线路合集优先激活的线路 URL（切换线路/恢复上次线路时使用）
    pub preferred_line: Option<String>,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            use_cache: false,
            cache_dir: None,
            cache_ttl_secs: 6 * 60 * 60,
            local_base: None,
            force_refresh: false,
            max_depth: 3,
            preferred_line: None,
        }
    }
}

impl LoadOptions {
    pub fn with_cache(cache_dir: impl Into<PathBuf>, local_base: Option<String>) -> Self {
        Self {
            use_cache: true,
            cache_dir: Some(cache_dir.into()),
            local_base,
            ..Self::default()
        }
    }
}

/* ============================================================
 * 对外入口
 * ============================================================ */

/// 旧 API：加载并返回当前激活线路的配置（线路合集时自动取第一条）
pub fn load_config(input: &str) -> Result<TvConfig> {
    Ok(load_config_with_options(input, &LoadOptions::default())?.config)
}

/// 只加载当前输入本身（不跟随线路合集），用于判断单仓/多仓、展示地址列表
pub fn load_config_shallow(input: &str) -> Result<TvConfig> {
    let payload = fetch_and_decode(input, &LoadOptions::default(), 0, &mut HashSet::new())?;
    let value = parse_json_value(&payload.text)?;
    deserialize_tv_config(value)
}

/// 检查订阅结构（单仓 / 线路合集），不加载子线路
pub fn inspect_config(input: &str, opts: &LoadOptions) -> Result<ConfigInspection> {
    let payload = fetch_and_decode(input, opts, 0, &mut HashSet::new())?;
    let value = parse_json_value(&payload.text)?;
    let kind = if is_line_collection(&value) {
        ConfigKind::Lines
    } else {
        ConfigKind::Single
    };
    let lines = if kind == ConfigKind::Single {
        vec![ConfigLine {
            name: None,
            url: input.trim().to_string(),
        }]
    } else {
        extract_lines(&value)
    };
    Ok(ConfigInspection {
        kind,
        lines,
        cache_hit: payload.cache_hit,
    })
}

/// 完整加载：解包当前输入；若是线路合集，跟随第一条线路并返回其真实配置
pub fn load_config_with_options(input: &str, opts: &LoadOptions) -> Result<LoadedConfig> {
    let mut visited = HashSet::new();
    let payload = fetch_and_decode(input, opts, 0, &mut visited)?;
    let value = parse_json_value(&payload.text)?;

    if is_line_collection(&value) {
        let lines = extract_lines(&value);
        if lines.is_empty() {
            return Err(CoreError::Config(
                "线路合集为空（urls 中没有有效地址）".into(),
            ));
        }
        let selected = opts
            .preferred_line
            .as_ref()
            .and_then(|want| lines.iter().find(|l| &l.url == want))
            .unwrap_or(&lines[0]);
        if opts.max_depth == 0 {
            return Err(CoreError::Config("线路合集跳转深度超限".into()));
        }
        let child_opts = LoadOptions {
            max_depth: opts.max_depth.saturating_sub(1),
            preferred_line: None,
            ..opts.clone()
        };
        let child = load_config_with_options(&selected.url, &child_opts)?;
        let mut config = child.config;
        // 保留合集线路给 UI（兼容旧 WarehouseUrl 形态）
        config.urls = lines
            .iter()
            .map(|l| WarehouseUrl {
                url: l.url.clone(),
                name: l.name.clone(),
            })
            .collect();
        return Ok(LoadedConfig {
            input: input.trim().to_string(),
            source_url: input.trim().to_string(),
            request_url: payload.request_url,
            active_line: selected.url.clone(),
            kind: ConfigKind::Lines,
            lines,
            config,
            format: payload.format,
            cache_hit: payload.cache_hit,
            decoded: payload.text,
        });
    }

    let config = deserialize_tv_config(value)?;
    Ok(LoadedConfig {
        input: input.trim().to_string(),
        source_url: input.trim().to_string(),
        request_url: payload.request_url,
        active_line: input.trim().to_string(),
        kind: ConfigKind::Single,
        lines: vec![ConfigLine {
            name: None,
            url: input.trim().to_string(),
        }],
        config,
        format: payload.format,
        cache_hit: payload.cache_hit,
        decoded: payload.text,
    })
}

/* ============================================================
 * 拉取 + 解密 + 修正
 * ============================================================ */

struct Payload {
    text: String,
    request_url: String,
    format: String,
    cache_hit: bool,
}

fn fetch_and_decode(
    input: &str,
    opts: &LoadOptions,
    depth: usize,
    visited: &mut HashSet<String>,
) -> Result<Payload> {
    let prepared = url_norm::prepare_input(input, opts.local_base.as_deref())?;
    if !visited.insert(prepared.request_url.clone()) {
        return Err(CoreError::Config(format!(
            "线路合集出现循环引用: {}",
            prepared.request_url
        )));
    }
    if depth > opts.max_depth {
        return Err(CoreError::Config("线路合集跳转深度超限".into()));
    }

    let mut cache_hit = false;
    let body = match prepared.kind {
        InputKind::InlineJson => prepared.request_url.clone(),
        InputKind::File => read_text_file(Path::new(&prepared.request_url))?,
        InputKind::Http | InputKind::Clan => {
            let cache_path = cache_file(opts, &prepared.original);
            let cached = if opts.use_cache && !opts.force_refresh {
                read_cache(&cache_path, opts.cache_ttl_secs)
            } else {
                None
            };
            match cached {
                Some(c) => {
                    cache_hit = true;
                    c.body
                }
                None => match fetch_http(&prepared.request_url) {
                    Ok(body) => {
                        if opts.use_cache {
                            let _ = write_cache(
                                &cache_path,
                                &prepared.request_url,
                                prepared.pk_key.as_deref(),
                                &body,
                            );
                        }
                        body
                    }
                    Err(e) => {
                        // 网络失败时回退过期缓存
                        if let Some(c) = read_cache(&cache_path, 0) {
                            tracing::warn!("配置拉取失败，使用过期缓存: {e}");
                            cache_hit = true;
                            c.body
                        } else {
                            return Err(e);
                        }
                    }
                },
            }
        }
    };

    let decoded = decode_config_body(&body, prepared.pk_key.as_deref())?;
    let mut text = decoded.text;
    if prepared.kind == InputKind::Clan {
        if let Some(base) = file_prefix(&prepared.request_url) {
            text = url_norm::clan_content_fix(&text, &base);
        }
    }
    text = url_norm::fix_relative_paths(&text, &prepared.request_url);

    Ok(Payload {
        text,
        request_url: prepared.request_url,
        format: decoded.format.as_str().to_string(),
        cache_hit,
    })
}

fn fetch_http(url: &str) -> Result<String> {
    let rules = Rules::default();
    let client = http::client_for(&rules)?;
    http::get_text(&client, url, &Default::default(), rules.retry)
}

fn read_text_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(CoreError::Io)?;
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok(s.trim_start_matches('\u{feff}').to_string());
    }
    let (cow, _, _) = encoding_rs::GBK.decode(&bytes);
    Ok(cow.into_owned())
}

/// 从 `http://host/file/xxx` 中取出 `http://host/file/` 前缀
fn file_prefix(request_url: &str) -> Option<String> {
    let idx = request_url.find("/file/")?;
    Some(request_url[..idx + 6].to_string())
}

/* ============================================================
 * JSON 解析与线路
 * ============================================================ */

fn parse_json_value(text: &str) -> Result<Value> {
    let text = normalize::normalize(text);
    let text = text.trim();
    if text.is_empty() {
        return Err(CoreError::Config("配置内容为空".into()));
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|e| CoreError::Config(format!("配置 JSON 解析失败: {e}")))?;
    if !matches!(value, Value::Object(_)) {
        return Err(CoreError::Config("配置顶层不是 JSON 对象".into()));
    }
    Ok(value)
}

/// 线路合集判定：顶层是对象、没有 `sites` 字段、有 `urls` 数组
fn is_line_collection(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    !obj.contains_key("sites") && obj.get("urls").map(|v| v.is_array()).unwrap_or(false)
}

fn extract_lines(value: &Value) -> Vec<ConfigLine> {
    let Some(arr) = value
        .as_object()
        .and_then(|o| o.get("urls"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        match item {
            Value::String(url) => {
                let url = url.trim();
                if !url.is_empty() {
                    out.push(ConfigLine {
                        name: None,
                        url: url.to_string(),
                    });
                }
            }
            Value::Object(o) => {
                let url = o
                    .get("url")
                    .and_then(|v| v.as_str())
                    .or_else(|| o.get("api").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .trim();
                if url.is_empty() {
                    continue;
                }
                let name = o
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                out.push(ConfigLine {
                    name,
                    url: url.to_string(),
                });
            }
            _ => {}
        }
    }
    out
}

fn deserialize_tv_config(value: Value) -> Result<TvConfig> {
    let mut cfg: TvConfig = serde_json::from_value(value)
        .map_err(|e| CoreError::Config(format!("配置字段解析失败: {e}")))?;
    cfg.http = Rules::default();
    for site in &mut cfg.sites {
        site.key = site.key.trim().to_string();
        site.api = site.api.trim().to_string();
        site.jar = site.jar.trim().to_string();
        if site.key.is_empty() {
            site.key = format!("site_{}", site.api);
        }
        if site.name.trim().is_empty() {
            site.name = site.key.clone();
        }
    }
    // 对齐 TVBoxOS ApiConfig.addSuperParse：解析器非空时在 index 0 注入"超级解析"，
    // 使其成为默认解析。仅在存在 type0/1 解析器时注入，避免纯 jar 配置被一个必然失败的
    // 默认解析挡住（TVBox 本身没有这个防御，VBox 做兼容性收紧）。
    let has_web_or_json = cfg.parses.iter().any(|p| p.kind == 0 || p.kind == 1);
    let has_super = cfg.parses.iter().any(|p| p.kind == 4);
    if has_web_or_json && !has_super {
        cfg.parses.insert(
            0,
            Parser {
                name: "超级解析".to_string(),
                kind: 4,
                url: "SuperParse".to_string(),
                ext: None,
            },
        );
    }
    Ok(cfg)
}

/* ============================================================
 * 缓存
 * ============================================================ */

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    saved_at: u64,
    request_url: String,
    #[serde(default)]
    pk_key: Option<String>,
    body: String,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_file(opts: &LoadOptions, original: &str) -> Option<PathBuf> {
    let dir = opts.cache_dir.as_ref()?;
    let key = md5::compute(original.as_bytes());
    Some(dir.join("config").join(format!("{:x}.json", key)))
}

fn read_cache(path: &Option<PathBuf>, ttl_secs: u64) -> Option<CacheEntry> {
    let path = path.as_ref()?;
    let text = fs::read_to_string(path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&text).ok()?;
    if ttl_secs > 0 && now_secs().saturating_sub(entry.saved_at) > ttl_secs {
        return None;
    }
    Some(entry)
}

fn write_cache(
    path: &Option<PathBuf>,
    request_url: &str,
    pk_key: Option<&str>,
    body: &str,
) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let entry = CacheEntry {
        saved_at: now_secs(),
        request_url: request_url.to_string(),
        pk_key: pk_key.map(|s| s.to_string()),
        body: body.to_string(),
    };
    let data = serde_json::to_string(&entry)
        .map_err(|e| CoreError::Config(format!("缓存序列化失败: {e}")))?;
    fs::write(path, data)?;
    Ok(())
}

/* ============================================================
 * 站点查找
 * ============================================================ */

pub fn find_site<'a>(cfg: &'a TvConfig, key: &str) -> Result<&'a Site> {
    cfg.sites
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| CoreError::SiteNotFound(key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> LoadOptions {
        LoadOptions::default()
    }

    #[test]
    fn inline_single_config() {
        let json = r#"{"sites":[{"key":"a","name":"A","type":1,"api":"http://x"}],"lives":[],"parses":[]}"#;
        let cfg = load_config(json).unwrap();
        assert_eq!(cfg.sites.len(), 1);
        assert_eq!(cfg.sites[0].kind, 1);
    }

    #[test]
    fn inline_line_collection_inspect() {
        let json = r#"{"urls":[{"name":"A","url":"http://127.0.0.1:9/does-not-exist.json"},{"name":"B","url":"http://127.0.0.1:9/b.json"}]}"#;
        let inspection = inspect_config(json, &opts()).unwrap();
        assert_eq!(inspection.kind, ConfigKind::Lines);
        assert_eq!(inspection.lines.len(), 2);
        assert_eq!(inspection.lines[1].name.as_deref(), Some("B"));
    }

    #[test]
    fn inline_config_with_sites_and_urls_is_single() {
        let json = r#"{"sites":[{"key":"a","type":1,"api":"http://x"}],"urls":["http://y"]}"#;
        let inspection = inspect_config(json, &opts()).unwrap();
        assert_eq!(inspection.kind, ConfigKind::Single);
        assert_eq!(inspection.lines.len(), 1);
    }
}
