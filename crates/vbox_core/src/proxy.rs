//! 本地代理桥（对齐 TVBox `/proxy` 的 JS 子集：`do=js` / `do=live`）
//!
//! 角色：
//! - 桌面端启动本地 HTTP 服务（`src-tauri/src/local_server.rs`）后，把根地址通过
//!   [`set_local_base`] 注入本模块；
//! - JS 沙箱里的 `getProxy()` / `js2Proxy(...)` 用该地址生成回环代理 URL（对齐
//!   catvod `Global.getProxy/js2Proxy`）；
//! - 本地服务的 `GET /proxy?do=js&...` 路由收到请求后，优先调用站点脚本的
//!   `proxy(params)`（`from=catvod` 约定），没有该函数时按 `url` + `header` 直通；
//! - 返回 m3u8 时把分片/密钥/变体地址重写为同一代理入口，保证后续请求同样带
//!   上 UA/Referer 等请求头（防盗链站点边下边播必需）。
//!
//! 安全边界：只代理 http(s)；请求头来自脚本，但禁止转发 `Host`/`Content-Length`
//! 等由客户端自行生成的头部（由调用方负责过滤）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value as Json};

use crate::model::Rules;
use crate::spider::sandbox::{JsArg, Sandbox, SandboxError, DEFAULT_TIMEOUT};

/// 本地服务根地址（`http://127.0.0.1:<port>`，无尾斜杠）。
static LOCAL_BASE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn base_slot() -> &'static Mutex<Option<String>> {
    LOCAL_BASE.get_or_init(|| Mutex::new(None))
}

/// 注入/清除本地服务根地址（桌面端在服务启动后调用；测试可自行设置）。
pub fn set_local_base(base: Option<String>) {
    let base = base
        .map(|b| b.trim().trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty());
    if let Ok(mut slot) = base_slot().lock() {
        *slot = base;
    }
}

/// 当前本地服务根地址（未启动本地服务时为 `None`）。
pub fn local_base() -> Option<String> {
    base_slot().lock().ok().and_then(|slot| slot.clone())
}

/// 查询参数编码：保留 URL 非保留字符（`A-Za-z0-9-._~`），其余百分号编码。
///
/// 与 `NON_ALPHANUMERIC` 不同，`.`/`-`/`_`/`~` 不转义，保证 m3u8 中的 URL 可读且
/// 与 TVBox 生成的代理地址一致。
pub fn encode_param(value: &str) -> String {
    use percent_encoding::{AsciiSet, CONTROLS};
    const QUERY_SET: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'&')
        .add(b'+')
        .add(b',')
        .add(b'/')
        .add(b':')
        .add(b';')
        .add(b'=')
        .add(b'?')
        .add(b'@')
        .add(b'[')
        .add(b']')
        .add(b'\\')
        .add(b'^')
        .add(b'`')
        .add(b'{')
        .add(b'|')
        .add(b'}')
        .add(b'%')
        .add(b'\'');
    percent_encoding::utf8_percent_encode(value, QUERY_SET).to_string()
}

/// `getProxy()` 的返回值：`<base>/proxy`；没有本地服务时返回空串。
pub fn get_proxy_url() -> String {
    local_base()
        .map(|base| format!("{base}/proxy"))
        .unwrap_or_default()
}

/// `js2Proxy(sync, dynamic, headers, url, times, flag)` 的返回值（对齐 catvod Global）。
#[allow(clippy::too_many_arguments)]
pub fn js2_proxy_url(
    sync: bool,
    dynamic: bool,
    headers: &Json,
    url: &str,
    times: &str,
    flag: &str,
    site_key: Option<&str>,
) -> String {
    match local_base() {
        Some(base) => js2_proxy_url_with_base(
            &base, sync, dynamic, headers, url, times, flag, site_key,
        ),
        None => String::new(),
    }
}

/// 同上，但允许显式指定 base（便于单测）。
#[allow(clippy::too_many_arguments)]
pub fn js2_proxy_url_with_base(
    base: &str,
    sync: bool,
    dynamic: bool,
    headers: &Json,
    url: &str,
    times: &str,
    flag: &str,
    site_key: Option<&str>,
) -> String {
    let header_text = match headers {
        Json::Object(_) => headers.to_string(),
        Json::String(text) => text.clone(),
        _ => "{}".to_string(),
    };
    let mut query = format!(
        "do=js&from=catvod&url={}&header={}&times={}&flag={}&sync={}&dynamic={}",
        encode_param(url),
        encode_param(&header_text),
        encode_param(if times.is_empty() { "0" } else { times }),
        encode_param(flag),
        if sync { 1 } else { 0 },
        if dynamic { 1 } else { 0 },
    );
    if let Some(key) = site_key.map(str::trim).filter(|k| !k.is_empty()) {
        query.push_str("&siteKey=");
        query.push_str(&encode_param(key));
    }
    format!("{base}/proxy?{query}")
}

/* ============================================================
 * JS spider proxy 回调（按 siteKey 路由）
 * ============================================================ */

/// 注册过的 JS 站点信息：用于本地服务收到 `do=js` 时重建沙箱并调用 `proxy(params)`。
struct ProxySite {
    code: String,
    name: String,
    rules: Rules,
    headers: HashMap<String, String>,
    init: Option<Json>,
    base_url: Option<String>,
}

static SITES: OnceLock<Mutex<HashMap<String, Arc<ProxySite>>>> = OnceLock::new();

fn sites() -> &'static Mutex<HashMap<String, Arc<ProxySite>>> {
    SITES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 注册（或覆盖）一个站点脚本，供本地代理回调使用。`js.rs` 每次构建沙箱时调用。
#[allow(clippy::too_many_arguments)]
pub fn register_js_proxy(
    site_key: &str,
    code: String,
    name: String,
    rules: Rules,
    headers: HashMap<String, String>,
    init: Option<Json>,
    base_url: Option<String>,
) {
    let key = site_key.trim();
    if key.is_empty() {
        return;
    }
    let site = Arc::new(ProxySite {
        code,
        name,
        rules,
        headers,
        init,
        base_url,
    });
    if let Ok(mut map) = sites().lock() {
        if map.len() > 128 {
            map.clear();
        }
        map.insert(key.to_string(), site);
    }
}

/// 清空注册表（测试/切换配置时使用；配置重载会自然覆盖）。
pub fn clear_js_proxies() {
    if let Ok(mut map) = sites().lock() {
        map.clear();
    }
}

/// 代理响应：状态码 / MIME / 响应体 / 可选重定向地址。
pub struct ProxyReply {
    pub status: u16,
    pub mime: String,
    pub body: Vec<u8>,
    pub location: Option<String>,
}

impl ProxyReply {
    fn text(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            mime: "text/plain; charset=utf-8".to_string(),
            body: message.into().into_bytes(),
            location: None,
        }
    }
}

/// 调用站点脚本的 `proxy(params)`。
///
/// - `None`：该 siteKey 未注册，或脚本没有实现 `proxy` 函数（调用方应回退直通）；
/// - `Some(reply)`：脚本返回了结果（失败也返回 502 文本，便于定位）。
pub fn call_js_proxy(site_key: &str, params: &Map<String, Json>) -> Option<ProxyReply> {
    let site = {
        let map = sites().lock().ok()?;
        map.get(site_key.trim())?.clone()
    };
    let mut sandbox = Sandbox::new(site.code.clone(), site.name.clone(), site.rules.clone())
        .with_timeout(DEFAULT_TIMEOUT)
        .with_headers(site.headers.clone())
        .with_site_key(site_key.trim().to_string());
    if let Some(base) = &site.base_url {
        sandbox = sandbox.with_base_url(base.clone());
    }
    if let Some(init) = &site.init {
        sandbox = sandbox.with_init(init.clone());
    }
    let params_value = Json::Object(params.clone());
    match sandbox.call("proxy", &[JsArg::Json(params_value)]) {
        Ok(value) => Some(reply_from_value(value)),
        Err(SandboxError::Missing(_)) => None,
        Err(err) => Some(ProxyReply::text(502, format!("JS proxy 执行失败: {err}"))),
    }
}

fn reply_from_value(value: Json) -> ProxyReply {
    match value {
        Json::Object(map) => {
            let status = map
                .get("code")
                .or_else(|| map.get("status"))
                .and_then(Json::as_u64)
                .filter(|code| (100..=599).contains(code))
                .unwrap_or(200) as u16;
            let mime = map
                .get("mime")
                .or_else(|| map.get("contentType"))
                .or_else(|| map.get("type"))
                .and_then(Json::as_str)
                .unwrap_or("application/octet-stream")
                .to_string();
            if let Some(location) = map
                .get("url")
                .or_else(|| map.get("location"))
                .and_then(Json::as_str)
                .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
            {
                return ProxyReply {
                    status: 302,
                    mime,
                    body: Vec::new(),
                    location: Some(location.to_string()),
                };
            }
            let body = map
                .get("content")
                .or_else(|| map.get("body"))
                .or_else(|| map.get("data"))
                .cloned()
                .unwrap_or(Json::Null);
            let body = match body {
                Json::Null => Vec::new(),
                Json::String(text) => text.into_bytes(),
                other => serde_json::to_vec(&other).unwrap_or_default(),
            };
            ProxyReply {
                status,
                mime,
                body,
                location: None,
            }
        }
        Json::String(text) => {
            // 有些 spider 会把可读响应再 JSON.stringify 一层
            if let Ok(inner) = serde_json::from_str::<Json>(&text) {
                if matches!(inner, Json::Object(_)) {
                    return reply_from_value(inner);
                }
            }
            ProxyReply {
                status: 200,
                mime: "text/plain; charset=utf-8".to_string(),
                body: text.into_bytes(),
                location: None,
            }
        }
        other => {
            let mime = if other.is_number() || other.is_boolean() || other.is_null() {
                "text/plain; charset=utf-8"
            } else {
                "application/json"
            };
            let body = match &other {
                Json::Null => Vec::new(),
                Json::String(text) => text.clone().into_bytes(),
                _ => serde_json::to_vec(&other).unwrap_or_default(),
            };
            ProxyReply {
                status: 200,
                mime: mime.to_string(),
                body,
                location: None,
            }
        }
    }
}

/* ============================================================
 * m3u8 重写
 * ============================================================ */

/// 判断响应是否需要按 m3u8 处理：优先 Content-Type，其次看正文头。
pub fn looks_like_m3u8(content_type: &str, body: &[u8]) -> bool {
    let content_type = content_type.to_ascii_lowercase();
    if content_type.contains("mpegurl") || content_type.contains("x-mpegurl") {
        return true;
    }
    let head = &body[..body.len().min(64)];
    String::from_utf8_lossy(head)
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("#EXTM3U")
}

/// 重写 m3u8 里的分片/密钥/变体地址。
///
/// - `playlist_url`：当前播放列表的绝对地址（用于解析相对 URI）；
/// - `query_without_url`：代理入口已有查询串（`do=js&...`，不含 `url`）；
/// - `local_base`：本地服务根地址。
pub fn rewrite_m3u8(
    playlist_url: &str,
    body: &str,
    query_without_url: &str,
    local_base: &str,
) -> String {
    let proxy = |target: &str| -> String {
        let absolute = resolve_url(playlist_url, target);
        format!(
            "{local_base}/proxy?{query_without_url}&url={}",
            encode_param(&absolute)
        )
    };
    let mut out = String::with_capacity(body.len() + body.len() / 4);
    for line in body.split_inclusive('\n') {
        let (content, ending) = match line.strip_suffix('\n') {
            Some(value) => (value, "\n"),
            None => (line, ""),
        };
        let content = content.strip_suffix('\r').unwrap_or(content);
        if content.starts_with('#') {
            let rewritten = uri_regex().replace_all(content, |caps: &regex::Captures<'_>| {
                format!("URI=\"{}\"", proxy(&caps[1]))
            });
            out.push_str(&rewritten);
        } else if content.trim().is_empty() {
            out.push_str(content);
        } else {
            out.push_str(&proxy(content.trim()));
        }
        out.push_str(ending);
    }
    out
}

fn uri_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r#"URI="([^"]*)""#).expect("URI 属性正则"))
}

/// 解析相对地址（m3u8 里可能是 `/path`、`../seg.ts`、`seg.ts` 或绝对地址）。
fn resolve_url(base: &str, target: &str) -> String {
    let target = target.trim();
    if target.starts_with("http://") || target.starts_with("https://") {
        return target.to_string();
    }
    match url::Url::parse(base).and_then(|base| base.join(target)) {
        Ok(url) => url.to_string(),
        Err(_) => target.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(value: &str) -> String {
        percent_encoding::percent_decode_str(value)
            .decode_utf8_lossy()
            .into_owned()
    }

    #[test]
    fn js2_proxy_url_contains_expected_params() {
        let headers = serde_json::json!({"referer": "https://a.com/"});
        let url = js2_proxy_url_with_base(
            "http://127.0.0.1:9527",
            false,
            true,
            &headers,
            "https://cdn.example.com/a.m3u8",
            "300",
            "flag1",
            Some("site-1"),
        );
        assert!(url.starts_with("http://127.0.0.1:9527/proxy?do=js&from=catvod&url="));
        assert!(url.contains("siteKey=site-1"));
        assert!(url.contains("flag=flag1"));
        assert!(url.contains("times=300"));
        assert!(decode(url.split("header=").nth(1).unwrap().split('&').next().unwrap())
            .contains("referer"));
    }

    #[test]
    fn js2_proxy_url_empty_without_local_service() {
        set_local_base(None);
        assert!(get_proxy_url().is_empty());
        assert!(js2_proxy_url(false, false, &Json::Null, "https://x/a", "", "", None).is_empty());
    }

    #[test]
    fn set_local_base_normalizes_trailing_slash() {
        set_local_base(Some("http://127.0.0.1:1/".to_string()));
        assert_eq!(get_proxy_url(), "http://127.0.0.1:1/proxy");
        set_local_base(None);
    }

    #[test]
    fn m3u8_rewrite_absolute_and_relative_and_uri_attrs() {
        let playlist = "https://cdn.example.com/live/index.m3u8";
        let body = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXTINF:2,\nseg1.ts\n#EXTINF:2,\nhttps://other.example.com/seg2.ts\n#EXT-X-MAP:URI=\"/init.mp4\"\n";
        let out = rewrite_m3u8(playlist, body, "do=js&from=catvod&header=%7B%7D", "http://127.0.0.1:9527");
        assert!(out.contains("#EXTM3U"));
        assert!(out.contains("URI=\"http://127.0.0.1:9527/proxy?do=js&from=catvod&header=%7B%7D&url=https%3A%2F%2Fcdn.example.com%2Flive%2Fkey.bin\""));
        assert!(out.contains("url=https%3A%2F%2Fcdn.example.com%2Flive%2Fseg1.ts"));
        assert!(out.contains("url=https%3A%2F%2Fother.example.com%2Fseg2.ts"));
        assert!(out.contains("url=https%3A%2F%2Fcdn.example.com%2Finit.mp4"));
        assert!(!out.contains("\nseg1.ts"));
    }

    #[test]
    fn looks_like_m3u8_detects_type_and_magic() {
        assert!(looks_like_m3u8("application/vnd.apple.mpegurl", b""));
        assert!(looks_like_m3u8("text/plain", b"#EXTM3U\n#EXTINF:1,\n"));
        assert!(!looks_like_m3u8("text/html", b"<html>"));
    }

    #[test]
    fn js_proxy_callback_returns_script_result() {
        clear_js_proxies();
        register_js_proxy(
            "unit-proxy-site",
            r#"function proxy(params) { return JSON.stringify({ code: 200, mime: 'text/plain', content: 'hello:' + params.url }); }"#
                .to_string(),
            "unit-proxy".to_string(),
            Rules::default(),
            HashMap::new(),
            None,
            None,
        );
        let mut params = Map::new();
        params.insert("url".to_string(), Json::String("https://cdn/x".into()));
        let reply = call_js_proxy("unit-proxy-site", &params).expect("proxy reply");
        assert_eq!(reply.status, 200);
        assert_eq!(String::from_utf8_lossy(&reply.body), "hello:https://cdn/x");
        clear_js_proxies();
    }

    #[test]
    fn js_proxy_missing_function_falls_back() {
        clear_js_proxies();
        register_js_proxy(
            "unit-proxy-missing",
            "function home() { return { class: [] }; }".to_string(),
            "unit-proxy-missing".to_string(),
            Rules::default(),
            HashMap::new(),
            None,
            None,
        );
        let params = Map::new();
        assert!(call_js_proxy("unit-proxy-missing", &params).is_none());
        clear_js_proxies();
    }
}
