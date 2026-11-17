//! `csp_Panyq`（盘友圈）原生实现。
//!
//! 反编译证据（只读参考）：
//! * `_rea/decompiled/0f047745-FxzHm/deobf/com/github/catvod/spider/Panyq.java`（jadx，629 行）
//! * `_rea/tmp/w10/vf-deobf/com/github/catvod/spider/Panyq.java`（Vineflower，4471 行；`g()` 字节码可读）
//! * `_rea/tmp/panyq/vf-decode.mjs` 解出的 33 条常量（payload/key/iv/正则/封面等）
//!
//! 协议分两段：
//! 1. **token 段**：`AES-128-CBC/PKCS7("all|{kw}|{page}|0|0|0")` → 小写 hex → `GET {base}/search/{hex}`。
//! 2. **sign 段**：`GET {base}/api/search?sign={sign}` 取 `{hits:[…]}`。`sign` 由搜索页运行时脚本产生，
//!    jar 用 `WebViewClient.onLoadResource` 拦截；桌面端由宿主 WebView 捕获后经 [`put_sign`] 注入。
//!
//! 与反编译版的有意偏差：
//! 1. 结果用通用 `✔`/`名称：`/`提取码:` 解析，不按平台分支二次请求。
//! 2. 详情/播放直接复用 `csp_PushShare` 的分享展开（原版通过 `Push` 基类委派同一批子引擎）。
//! 3. `sign` 支持 ext / 环境变量 / 外部命令三种静态来源，缺省报 `catvod.host.unsupported`。
//!
//! 该家族此前 unported（`webview_dependency`），语义以本实现与测试为准。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use aes::Aes128;
use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use regex::Regex;
use serde_json::Value;

use super::pushshare;
use super::super::contract::{CatvodError, ErrorClass};
use crate::error::Result;
use crate::http::{client_for, get_text};
use crate::model::{Category, Site, TvConfig, VideoDetail, VideoItem};

/// 家族 API 名（与 CatVod 侧 `csp_Panyq` 保持一致）。
pub const API: &str = "csp_Panyq";

/// 站点默认基址（ext 可用 `base` / `url` 覆盖）。
const BASE_DEFAULT: &str = "https://panyq.com";

/// token 段的 AES-128 密钥（与反编译版一致）。
pub const TOKEN_KEY: &str = "Qb8uTX98yDe49fNL";
/// token 段的 AES-128 IV（与反编译版一致）。
pub const TOKEN_IV: &str = "gZ2PEPThrWGeWME9";

/// 请求 UA（与反编译版一致）。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// 搜索结果的剧集组名。
const GROUP: &str = "Panyq";

/// 分类：原家族只覆写搜索，首页固定一个「搜索」项。
const SEARCH_TYPE: &str = "JARSEARCH";
const SEARCH_NAME: &str = "搜索";

/// 宿主捕获的 sign 注入点：`site.key` → 完整 `/api/search?sign=...` URL。
fn sign_store() -> &'static Mutex<HashMap<String, String>> {
    static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 宿主 WebView 捕获到 sign 请求后调用（桌面端 `capture_api_url_async` 的结果）。
pub fn put_sign(site_key: &str, search_url: &str) {
    if let Ok(mut guard) = sign_store().lock() {
        guard.insert(site_key.to_string(), search_url.to_string());
    }
}

/// 取出宿主注入的 sign 请求 URL。
fn injected_sign(site_key: &str) -> Option<String> {
    sign_store()
        .lock()
        .ok()
        .and_then(|guard| guard.get(site_key).cloned())
}

/// 家族判定：`csp_Panyq`（大小写不敏感）。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

/// 计算搜索 token：`AES-128-CBC/PKCS7("all|{kw}|{page}|0|0|0")` → 小写 hex。
pub fn search_token(keyword: &str, page: u32) -> std::result::Result<String, String> {
    let payload = format!("all|{}|{}|0|0|0", keyword, page.max(1));
    let cipher = cbc::Encryptor::<Aes128>::new_from_slices(TOKEN_KEY.as_bytes(), TOKEN_IV.as_bytes())
        .map_err(|e| format!("AES-CBC 参数错误（key/iv 需 16 字节）: {e}"))?;
    let raw = cipher.encrypt_padded_vec_mut::<Pkcs7>(payload.as_bytes());
    Ok(hex::encode(raw))
}

/// 搜索页 URL（`{base}/search/{hex}`）：宿主 WebView 需要先加载它才能捕获 sign。
pub fn search_page_url(
    base: &str,
    keyword: &str,
    page: u32,
) -> std::result::Result<String, String> {
    let token = search_token(keyword, page)?;
    Ok(format!("{}/search/{}", base.trim_end_matches('/'), token))
}

/// 宿主 WebView 应加载的搜索页 URL（内部按站点 ext/默认值计算基址）。
pub fn host_page_url(site: &Site, keyword: &str, page: u32) -> std::result::Result<String, String> {
    search_page_url(&base_of(site), keyword, page)
}

/// 宿主是否应立刻做一次 WebView 捕获（已配置或已注入 sign 时不必）。
pub fn host_capture_needed(site: &Site) -> bool {
    if !handles(&site.api) || injected_sign(&site.key).is_some() {
        return false;
    }
    if std::env::var("VBOX_PANYQ_SIGN")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    let obj = ext_object(site);
    ["sign", "sign_url", "search_url"].iter().any(|key| {
        obj.as_ref()
            .and_then(|map| map.get(*key))
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    }) == false
}

fn ext_object(site: &Site) -> Option<Value> {
    match site.ext.as_ref() {
        Some(Value::Object(_)) => site.ext.clone(),
        Some(Value::String(raw)) => {
            let text = raw.trim();
            if !text.starts_with('{') {
                return None;
            }
            serde_json::from_str::<Value>(text).ok()
        }
        _ => None,
    }
}

fn ext_str(site: &Site, keys: &[&str]) -> Option<String> {
    let object = ext_object(site)?;
    for key in keys {
        if let Some(Value::String(text)) = object.get(*key) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn base_of(site: &Site) -> String {
    ext_str(site, &["base", "url", "api"])
        .filter(|text| text.starts_with("http"))
        .unwrap_or_else(|| BASE_DEFAULT.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn headers() -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Referer".to_string(), format!("{BASE_DEFAULT}/"));
    headers
}

fn strip_tags(text: &str) -> String {
    static TAGS: OnceLock<Regex> = OnceLock::new();
    let re = TAGS.get_or_init(|| Regex::new(r"<[^>]*>").expect("html tag regex"));
    re.replace_all(text, "").to_string()
}

/// 把 ext / 环境变量 / 外部命令给的 sign 值归一成完整 `/api/search` URL。
fn normalize_sign(base: &str, raw: &str) -> String {
    let raw = raw.trim();
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return raw.to_string();
    }
    let sign = raw.strip_prefix("sign=").unwrap_or(raw);
    format!("{base}/api/search?sign={sign}")
}

fn run_command(command: &str) -> Option<String> {
    let mut parts = command.split_whitespace();
    let program = parts.next()?;
    let output = std::process::Command::new(program)
        .args(parts)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// 依次尝试：宿主注入 → 站点 ext → 环境变量 → 外部命令；都没有时返回可操作的错误提示。
fn resolve_sign(site: &Site) -> Result<String> {
    let base = base_of(site);
    if let Some(url) = injected_sign(&site.key) {
        return Ok(url);
    }
    if let Some(raw) = ext_str(site, &["sign", "sign_url", "search_url"]) {
        return Ok(normalize_sign(&base, &raw));
    }
    if let Ok(raw) = std::env::var("VBOX_PANYQ_SIGN") {
        let raw = raw.trim();
        if !raw.is_empty() {
            return Ok(normalize_sign(&base, raw));
        }
    }
    for key in ["sign_command", "sign_cmd"] {
        if let Some(command) = ext_str(site, &[key]) {
            if let Some(raw) = run_command(&command) {
                return Ok(normalize_sign(&base, &raw));
            }
        }
    }
    if let Ok(command) = std::env::var("VBOX_PANYQ_SIGN_CMD") {
        if let Some(raw) = run_command(command.trim()) {
            return Ok(normalize_sign(&base, &raw));
        }
    }
    let page = search_page_url(&base, "", 1)
        .unwrap_or_else(|_| format!("{base}/search/<token>"));
    Err(host_err(
        site,
        format!(
            "csp_Panyq 搜索需要宿主 WebView 捕获 sign（页面运行时生成）：请打开 {page} 并捕获其发出的 \
             /api/search?sign= 请求，或把 sign 写入站点 ext 的 sign/sign_url，或用 sign_command 产出"
        ),
    ))
}

/// 解析单个 hit 的 `d` 字段。
fn parse_hit(hit: &Value, keyword: &str) -> Option<VideoItem> {
    let raw = hit.get("d")?.as_str()?;
    let text = strip_tags(raw);
    let first = text.split('✔').next().unwrap_or("").trim();
    let name = first.replace("名称：", "").trim().to_string();
    if name.is_empty() || !name.contains(keyword) {
        return None;
    }
    let start = text.find("https://")?;
    let mut url = text[start..].trim().to_string();
    if let Some(space) = url.find(char::is_whitespace) {
        url.truncate(space);
    }
    if url.is_empty() {
        return None;
    }
    if let Some(at) = text.find("提取码:") {
        let pass = text[at + "提取码:".len()..].trim();
        let pass = pass.split(char::is_whitespace).next().unwrap_or("");
        if !pass.is_empty() && !url.contains("?pwd=") {
            url.push_str("?pwd=");
            url.push_str(pass);
        }
    }
    let group = hit.get("group").and_then(Value::as_str).unwrap_or("");
    let cover = match group {
        "ali" => Some("https://inews.gtimg.com/newsapp_bt/0/13263837859/1000".to_string()),
        "quark" => Some(
            "https://bkimg.cdn.bcebos.com/pic/6d81800a19d8bc3e93bd4e218c8ba61ea9d345a7".to_string(),
        ),
        "pikpak" => Some(
            "https://i0.hdslb.com/bfs/archive/5ae3b02b3c3246afda72ac84a497475dc29187c7.jpg".to_string(),
        ),
        "xunlei" | "com/xunlei" | "thunder" => Some(
            "https://bkimg.cdn.bcebos.com/pic/d4628535e5dde7114164dfafa8efce1b9d166131".to_string(),
        ),
        _ => None,
    };
    let remark = hit
        .get("desc")
        .and_then(Value::as_str)
        .map(strip_tags)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    Some(VideoItem {
        id: url,
        name,
        cover,
        remark,
    })
}

/// 从 `/api/search` 响应里取 hits 并解析。
pub fn parse_hits(value: &Value, keyword: &str) -> Vec<VideoItem> {
    let rows = value
        .get("hits")
        .and_then(Value::as_array)
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| data.get("hits"))
                .and_then(Value::as_array)
        });
    let Some(rows) = rows else {
        return Vec::new();
    };
    let keyword = keyword.trim();
    rows.iter().filter_map(|hit| parse_hit(hit, keyword)).collect()
}

/// 分类：固定一个「搜索」项。
pub fn categories(_cfg: &TvConfig, _site: &Site) -> Result<Vec<Category>> {
    Ok(vec![Category {
        type_id: SEARCH_TYPE.to_string(),
        type_name: SEARCH_NAME.to_string(),
    }])
}

/// 列表：原家族没有分类列表，返回空。
pub fn videos(_cfg: &TvConfig, _site: &Site, _cat: &str, _page: u32) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

/// 把站点换成 `csp_PushShare` 视图，用于复用分享浏览/播放。
fn synthetic_site(site: &Site) -> Site {
    let mut view = site.clone();
    view.api = pushshare::API.to_string();
    view
}

/// 详情：搜索结果 id 就是分享链接，直接复用 PushShare 的分享展开。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "csp_Panyq 详情 id 为空"));
    }
    pushshare::detail(cfg, &synthetic_site(site), id)
}

/// 播放：分享链接 + `###fid` 的引用交给 PushShare 解析直链。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "csp_Panyq 播放地址为空"));
    }
    pushshare::player_content(cfg, &synthetic_site(site), flag, id)
}

/// 搜索：token 段只用于定位页面；实际请求需要宿主签发的 sign。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }
    let sign_url = resolve_sign(site)?;
    let client = client_for(&cfg.http)?;
    let retries = cfg.http.retry.max(2);
    let text = get_text(&client, &sign_url, &headers(), retries)?;
    let value: Value = serde_json::from_str(&text).map_err(|e| {
        parse_err(
            site,
            format!("csp_Panyq JSON 解析失败: {e}; url={sign_url}"),
        )
    })?;
    Ok(parse_hits(&value, keyword))
}

/// 供宿主查询：该站点是否需要 WebView 捕获 sign。
pub fn needs_host_sign(site: &Site) -> bool {
    handles(&site.api)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site() -> Site {
        Site {
            key: "panyq".into(),
            name: "盘友圈".into(),
            api: API.into(),
            ..Default::default()
        }
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Panyq"));
        assert!(handles(" csp_panyq "));
        assert!(!handles("csp_PushShare"));
    }

    #[test]
    fn token_matches_known_vector() {
        assert_eq!(
            search_token("test", 1).unwrap(),
            "f676b1c78c22fdf8eeae05d3b0bb6a92055c18a48f5853102db0b8dd1067653c"
        );
        assert_eq!(
            search_page_url("https://panyq.com/", "test", 1).unwrap(),
            "https://panyq.com/search/f676b1c78c22fdf8eeae05d3b0bb6a92055c18a48f5853102db0b8dd1067653c"
        );
    }

    #[test]
    fn parse_hit_extracts_name_url_and_password() {
        let hit = json!({
            "d": "<b>名称：</b>三体 全集✔<a href=\"x\">https://pan.quark.cn/s/abc123</a> 提取码: 4f2a",
            "group": "quark"
        });
        let item = parse_hit(&hit, "三体").expect("hit");
        assert_eq!(item.name, "三体 全集");
        assert_eq!(item.id, "https://pan.quark.cn/s/abc123?pwd=4f2a");
        assert!(item.cover.unwrap().contains("bkimg"));
    }

    #[test]
    fn parse_hits_filters_by_keyword_and_bad_rows() {
        let value = json!({
            "data": {"hits": [
                {"d": "名称：三体 全集✔https://pan.quark.cn/s/a", "group": "quark"},
                {"d": "名称：无关资源✔https://pan.quark.cn/s/b", "group": "quark"},
                {"d": "没有链接"},
                {"nope": 1}
            ]}
        });
        let items = parse_hits(&value, "三体");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "https://pan.quark.cn/s/a");
    }

    #[test]
    fn resolve_sign_prefers_injection_then_ext() {
        let mut site = site();
        site.key = "panyq-resolve".into();
        site.ext = Some(json!({"sign": "abcdef"}));
        put_sign(&site.key, "https://panyq.com/api/search?sign=injected");
        assert_eq!(
            resolve_sign(&site).unwrap(),
            "https://panyq.com/api/search?sign=injected"
        );
        if let Ok(mut guard) = sign_store().lock() {
            guard.remove(&site.key);
        }
        assert_eq!(
            resolve_sign(&site).unwrap(),
            "https://panyq.com/api/search?sign=abcdef"
        );
    }

    #[test]
    fn missing_sign_reports_host_error() {
        let mut site = site();
        site.key = "panyq-missing".into();
        if let Ok(mut guard) = sign_store().lock() {
            guard.remove(&site.key);
        }
        std::env::remove_var("VBOX_PANYQ_SIGN");
        std::env::remove_var("VBOX_PANYQ_SIGN_CMD");
        let err = resolve_sign(&site).unwrap_err();
        assert!(err.to_string().contains("catvod.host.unsupported"), "{err}");
    }

    #[test]
    fn categories_has_single_search_entry() {
        let cats = categories(&TvConfig::default(), &site()).unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].type_id, SEARCH_TYPE);
    }

    #[test]
    fn detail_requires_id() {
        let err = detail(&TvConfig::default(), &site(), "  ").unwrap_err();
        assert!(err.to_string().contains("catvod.result.parse"));
    }
}
