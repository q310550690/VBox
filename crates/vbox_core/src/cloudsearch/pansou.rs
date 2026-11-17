//! PanSou 兼容的 HTTP 增强源（可选，W14 §4）。
//!
//! 契约按 PanSou 常见实现取「够用就好」的版本：
//!
//! ```text
//! GET {base}/api/search?kw=<keyword>&res=<page_size>&page=<page>
//! Authorization: Bearer <token>   // token 可选
//! ```
//!
//! 真实 PanSou 的字段（`res` 的取值、聚合结构、`cloud_types` 过滤等）因部署版本而异，
//! 这里**只做容错解析**：响应体先尝试 `data` / `results` / `items` / `list` / `hits`
//! 数组，单条字段 `title` / `url` / `share_url` / `size` / `published_at` / `password`
//! 都宽松读取；无法识别的条目跳过，不因个别脏数据整体失败。
//!
//! TODO(W14)：接入真实 PanSou 服务后，按实际响应补 `res` 枚举（merge/all 等）与分页语义。

use std::time::{Duration, Instant};

use serde_json::Value;

use super::detect::detect_share;
use super::tg::parse_rfc3339_seconds;
use super::{
    CloudSearchSource, SearchItem, SearchPage, SearchProvider, SearchQuery, SourceKind,
    SourceStatus,
};
use crate::error::CoreError;

/// PanSou 兼容源。
pub struct PanSouSource {
    base_url: String,
    token: Option<String>,
    client: reqwest::blocking::Client,
}

impl PanSouSource {
    /// 构造（`base_url` 允许不带 scheme，缺省会补 `http://`）。
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Result<Self, CoreError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(CoreError::Http)?;
        Ok(Self::with_client(base_url, token, client))
    }

    /// 用调用方提供的 blocking client 构造（便于共享连接池 / 统一代理设置）。
    pub fn with_client(
        base_url: impl Into<String>,
        token: Option<String>,
        client: reqwest::blocking::Client,
    ) -> Self {
        PanSouSource {
            base_url: normalize_base_url(&base_url.into()),
            token,
            client,
        }
    }

    /// 服务地址。
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 是否已配置服务地址。
    pub fn is_configured(&self) -> bool {
        !self.base_url.is_empty()
    }

    /// 拼搜索 URL（`kw` 走 reqwest 的 query 编码）。
    fn request_url(&self) -> String {
        format!("{}/api/search", self.base_url)
    }

    /// 请求并解析；HTTP 失败 / 响应不是合法 JSON 都返回错误（由 kernel 降级处理）。
    fn fetch(&self, q: &SearchQuery) -> Result<Vec<SearchItem>, CoreError> {
        if !self.is_configured() {
            return Err(CoreError::Config("未配置 PanSou 服务地址".to_string()));
        }
        let mut request = self.client.get(self.request_url()).query(&[
            ("kw", q.keyword.as_str().to_string()),
            ("res", q.page_size.to_string()),
            ("page", q.page.max(1).to_string()),
        ]);
        if let Some(token) = &self.token {
            request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }

        let response = request.send()?.error_for_status()?;
        let payload: Value = response.json()?;
        validate_payload(&payload)?;
        Ok(parse_payload(&payload))
    }
}

impl CloudSearchSource for PanSouSource {
    fn search(&self, q: &SearchQuery) -> Result<SearchPage, CoreError> {
        let started = Instant::now();
        let keyword = q.keyword.trim();
        if keyword.is_empty() {
            let mut page = SearchPage::empty();
            page.elapsed_ms = started.elapsed().as_millis() as u64;
            return Ok(page);
        }
        let items = self.fetch(&SearchQuery {
            keyword: keyword.to_string(),
            ..q.clone()
        })?;
        Ok(SearchPage {
            total: items.len(),
            items,
            partial: false,
            elapsed_ms: started.elapsed().as_millis() as u64,
            degraded_sources: Vec::new(),
            deduped: 0,
            provider_counts: Vec::new(),
        })
    }

    fn health(&self) -> SourceStatus {
        SourceStatus {
            kind: SourceKind::PanSou,
            ready: self.is_configured(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            detail: if self.is_configured() {
                format!("pansou: {}", self.base_url)
            } else {
                "pansou: 未配置 base_url".to_string()
            },
        }
    }

    fn name(&self) -> String {
        "pansou".to_string()
    }
}

/// `base_url` 归一化：去尾部 `/`，缺 scheme 补 `http://`（PanSou 通常是本机服务）。
fn normalize_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

/// 从任意容错结构里收集条目数组。
///
/// 支持：
/// - `[...]`（根数组）
/// - `{"data": [...]}` / `{"results": [...]}` / `{"items": [...]}` / `{"list": [...]}` / `{"hits": [...]}`
/// - 再嵌一层：`{"data": {"results": [...]}}`（PanSou 不同版本）
fn collect_entries<'a>(value: &'a Value, depth: usize) -> Vec<&'a Value> {
    if depth > 3 {
        return Vec::new();
    }
    match value {
        Value::Array(entries) => entries.iter().collect(),
        Value::Object(object) => {
            // PanSou 官方响应：`merged_by_type: { "quark": [...], "aliyun": [...] }`
            if let Some(grouped) = object.get("merged_by_type") {
                let Some(groups) = grouped.as_object() else {
                    return Vec::new();
                };
                let mut entries: Vec<&Value> = Vec::new();
                for group in groups.values() {
                    if let Value::Array(list) = group {
                        entries.extend(list.iter());
                    }
                }
                return entries;
            }
            for key in ["data", "results", "items", "list", "hits"] {
                if let Some(inner) = object.get(key) {
                    let entries = collect_entries(inner, depth + 1);
                    if !entries.is_empty() || inner.is_array() {
                        return entries;
                    }
                }
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// 校验响应顶层结构：PanSou 不同版本可能用 `data` / `merged_by_type` 等字段，
/// 但完全不认识的响应应报 `cloudsearch.pansou_invalid`，而不是静默空结果。
fn validate_payload(value: &Value) -> Result<(), CoreError> {
    match value {
        Value::Array(_) => Ok(()),
        Value::Object(object) => {
            const KEYS: [&str; 6] = ["data", "results", "items", "list", "hits", "merged_by_type"];
            if KEYS.iter().any(|key| object.contains_key(*key)) {
                Ok(())
            } else {
                Err(CoreError::Json {
                    path: "pansou.merged_by_type".to_string(),
                    msg: "响应缺少 data/merged_by_type 等结果字段".to_string(),
                })
            }
        }
        _ => Err(CoreError::Json {
            path: "pansou".to_string(),
            msg: "响应不是 JSON 对象或数组".to_string(),
        }),
    }
}

/// 把 PanSou 响应映射成 [`SearchItem`] 列表。
pub fn parse_payload(payload: &Value) -> Vec<SearchItem> {
    collect_entries(payload, 0)
        .into_iter()
        .filter_map(parse_entry)
        .collect()
}

fn parse_entry(entry: &Value) -> Option<SearchItem> {
    let object = entry.as_object()?;
    let title = first_str(object, &["title", "name", "message"]).unwrap_or_default();

    let raw_url = first_str(object, &["share_url", "url", "link", "href", "magnet"])?;
    let raw_url = raw_url.trim();
    if raw_url.is_empty() {
        return None;
    }

    let detected = detect_share(raw_url);
    let url_provider = detected.as_ref().map(|share| share.provider);
    let declared_provider = first_str(object, &["provider", "pan", "drive", "cloud_type"])
        .map(SearchProvider::from_str_lossy)
        .filter(|provider| *provider != SearchProvider::Other)
        .or(url_provider)
        .unwrap_or_else(|| SearchProvider::from_str_lossy("other"));

    let share_url = detected
        .as_ref()
        .map(|share| share.share_url.clone())
        .unwrap_or_else(|| raw_url.to_string());
    let share_id = detected
        .as_ref()
        .map(|share| share.share_id.clone())
        .unwrap_or_else(|| share_url.clone());

    let password = first_str(object, &["password", "passcode", "pwd", "access_code"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| detected.as_ref().and_then(|share| share.password.clone()));

    let size = object.get("size").and_then(parse_size);
    let published_at = object
        .get("published_at")
        .or_else(|| object.get("time"))
        .or_else(|| object.get("date"))
        .and_then(parse_timestamp);

    let sources = parse_sources(object);
    let playable = declared_provider.is_playable();

    Some(SearchItem {
        id: format!("{}:{}", declared_provider.as_str(), share_id),
        title: title.to_string(),
        provider: declared_provider,
        share_url,
        password,
        size,
        published_at,
        sources,
        playable,
    })
}

fn first_str<'a>(object: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(value) = object.get(*key) {
            if let Some(text) = value.as_str() {
                let text = text.trim();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

/// `sources`：兼容字符串 / 数组；缺省记为 `pansou`。
fn parse_sources(object: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut sources = Vec::new();
    for key in ["sources", "source", "channel", "channels"] {
        match object.get(key) {
            Some(Value::String(text)) if !text.trim().is_empty() => {
                sources.push(text.trim().to_string());
            }
            Some(Value::Array(values)) => {
                for value in values {
                    if let Some(text) = value.as_str() {
                        if !text.trim().is_empty() {
                            sources.push(text.trim().to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if sources.is_empty() {
        sources.push("pansou".to_string());
    }
    sources.dedup();
    sources
}

/// 体积：接受数字或 `"1.5 GB"` / `"2048"` 这类字符串。
fn parse_size(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .filter(|float| *float > 0.0)
                .map(|float| float as u64)
        }),
        Value::String(text) => parse_size_text(text),
        _ => None,
    }
}

fn parse_size_text(raw: &str) -> Option<u64> {
    let cleaned = raw.trim().replace(',', "").to_lowercase();
    if cleaned.is_empty() {
        return None;
    }
    let number_end = cleaned
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit() || *ch == '.')
        .last()
        .map(|(index, ch)| index + ch.len_utf8())?;
    if number_end == 0 {
        return None;
    }
    let number: f64 = cleaned[..number_end].parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let unit = cleaned[number_end..].trim();
    let multiplier: u64 = match unit {
        "" | "b" | "byte" | "bytes" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        "t" | "tb" | "tib" => 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };
    Some((number * multiplier as f64) as u64)
}

/// 时间：接受 Unix 秒（数字 / 数字字符串）或 RFC3339 字符串。
fn parse_timestamp(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => {
            let text = text.trim();
            text.parse::<i64>()
                .ok()
                .or_else(|| parse_rfc3339_seconds(text))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_http::{Response, Server};

    const PAYLOAD: &str = r#"{
      "data": [
        {
          "title": "阿里云盘 4K 电影合集",
          "share_url": "https://www.alipan.com/s/abc123?pwd=xy9z",
          "size": 2048,
          "published_at": 1793536496
        },
        {
          "title": "磁力资源 合集",
          "url": "magnet:?xt=urn:btih:aaaabbbbccccddddeeeeffff0000111122223333",
          "size": "3.5 GB",
          "published_at": "1793491200"
        },
        {
          "title": "没有链接的条目"
        }
      ]
    }"#;

    /// 起一个只应答一次的本地 HTTP server，返回 (地址, join handle)。
    fn mock_server(body: &'static str, status: u16) -> (String, std::thread::JoinHandle<()>) {
        let server = Server::http("127.0.0.1:0").expect("启动 mock server");
        let address = format!("http://{}", server.server_addr());
        let handle = std::thread::spawn(move || {
            if let Ok(request) = server.recv() {
                let mut response = Response::from_string(body);
                if status != 200 {
                    response = response.with_status_code(tiny_http::StatusCode::from(status));
                }
                let _ = request.respond(response);
            }
        });
        (address, handle)
    }

    fn client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("client")
    }

    #[test]
    fn maps_pansou_payload() {
        let (address, handle) = mock_server(PAYLOAD, 200);
        let source = PanSouSource::with_client(address, None, client());

        let page = source
            .search(&SearchQuery {
                keyword: "电影".to_string(),
                ..SearchQuery::default()
            })
            .expect("mock 请求应成功");
        handle.join().expect("mock server 线程");

        assert_eq!(page.total, 2, "没有链接的条目应跳过");
        let first = &page.items[0];
        assert_eq!(first.title, "阿里云盘 4K 电影合集");
        assert_eq!(first.provider, SearchProvider::Aliyun);
        assert_eq!(first.id, "aliyun:abc123");
        assert_eq!(first.password.as_deref(), Some("xy9z"));
        assert_eq!(first.size, Some(2048));
        assert_eq!(first.published_at, Some(1_793_536_496));
        assert!(first.playable);
        assert_eq!(first.sources, vec!["pansou".to_string()]);

        let magnet = &page.items[1];
        assert_eq!(magnet.provider, SearchProvider::Magnet);
        assert!(!magnet.playable);
        assert_eq!(
            magnet.size,
            Some((3.5f64 * 1024.0 * 1024.0 * 1024.0) as u64),
            "带单位体积应换算成字节"
        );
        assert_eq!(magnet.published_at, Some(1_793_491_200));
    }

    #[test]
    fn tolerates_alternate_response_shapes() {
        let payload = serde_json::json!({
            "results": [
                {"name": "夸克资源", "link": "pan.quark.cn/s/qq112233", "provider": "quark"}
            ]
        });
        let items = parse_payload(&payload);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "夸克资源");
        assert_eq!(items[0].provider, SearchProvider::Quark);
        assert_eq!(items[0].id, "quark:qq112233");

        let nested = serde_json::json!({"data": {"items": [
            {"title": "百度资源", "url": "https://pan.baidu.com/s/1abc?pwd=1234"}
        ]}});
        let items = parse_payload(&nested);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].provider, SearchProvider::Baidu);
        assert_eq!(items[0].password.as_deref(), Some("1234"));
    }

    #[test]
    fn non_2xx_returns_error() {
        let (address, handle) = mock_server("{}", 500);
        let source = PanSouSource::with_client(address, None, client());
        let result = source.search(&SearchQuery {
            keyword: "电影".to_string(),
            ..SearchQuery::default()
        });
        handle.join().expect("mock server 线程");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CoreError::Http(_)));
    }

    #[test]
    fn empty_base_url_is_not_ready_and_fails() {
        let source = PanSouSource::with_client("  ", None, client());
        assert!(!source.health().ready);
        assert!(!source.is_configured());
        assert_eq!(source.health().kind, SourceKind::PanSou);
        assert!(source
            .search(&SearchQuery {
                keyword: "电影".to_string(),
                ..SearchQuery::default()
            })
            .is_err());
    }

    #[test]
    fn empty_keyword_does_not_request() {
        let source = PanSouSource::with_client("http://127.0.0.1:9", None, client());
        let page = source
            .search(&SearchQuery {
                keyword: "   ".to_string(),
                ..SearchQuery::default()
            })
            .expect("空关键词应直接返回空页");
        assert_eq!(page.total, 0);
    }

    #[test]
    fn parses_size_and_timestamp_variants() {
        assert_eq!(parse_size_text("2048"), Some(2048));
        assert_eq!(parse_size_text("1,024"), Some(1024));
        assert_eq!(parse_size_text("1.5 GB"), Some(1_610_612_736));
        assert_eq!(parse_size_text("2mb"), Some(2 * 1024 * 1024));
        assert_eq!(parse_size_text(""), None);
        assert_eq!(parse_size_text("abc"), None);

        assert_eq!(parse_timestamp(&serde_json::json!(123)), Some(123));
        assert_eq!(parse_timestamp(&serde_json::json!("456")), Some(456));
        assert_eq!(
            parse_timestamp(&serde_json::json!("2021-01-01T00:00:00Z")),
            Some(1_609_459_200)
        );
        assert_eq!(parse_timestamp(&serde_json::json!(null)), None);
    }

    #[test]
    fn parses_merged_by_type_payload() {
        let payload = serde_json::json!({
            "merged_by_type": {
                "quark": [{
                    "title": "三体 全季 4K",
                    "share_url": "https://pan.quark.cn/s/abc123",
                    "password": "8888",
                    "size": "12.5 GB"
                }],
                "aliyun": [{
                    "title": "流浪地球2",
                    "share_url": "https://www.aliyundrive.com/s/xyz789"
                }]
            }
        });
        let items = parse_payload(&payload);
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| item.provider == SearchProvider::Quark));
        assert!(items.iter().any(|item| item.provider == SearchProvider::Aliyun));
    }

    #[test]
    fn rejects_unrecognized_payload_structure() {
        let error = validate_payload(&serde_json::json!({"code": 0, "message": "ok"})).unwrap_err();
        assert!(matches!(error, CoreError::Json { ref path, .. } if path == "pansou.merged_by_type"));
        assert!(validate_payload(&serde_json::json!({"data": []})).is_ok());
        assert!(validate_payload(&serde_json::json!([{"title": "x"}])).is_ok());
        assert!(validate_payload(&serde_json::json!("plain text")).is_err());
    }
}
