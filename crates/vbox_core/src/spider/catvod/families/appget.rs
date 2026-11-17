//! `csp_AppGet` 原生家族：TVBox「app 接口」类 app.php 站点。
//!
//! 来源：`com.github.catvod.spider.AppGet`（`5fc50e17-…` jar，410 行、无混淆数组）。
//!
//! 协议要点：
//! 1. ext 是 JSON：`{url, dataKey, dataIv, deviceId, version, ua, token}`；`url` 为接口基址。
//! 2. 所有请求 `POST {base}/api.php{path}`，body 为 JSON 字符串，头带
//!    `app-user-device-id`/`app-version-code`/`app-api-verify-time`/`app-ui-mode`，可选 `app-user-token`。
//! 3. 响应信封 `{"data": "<AES-CBC(Base64) 密文>"}`，用 `dataKey`/`dataIv` 解出真实 JSON。
//! 4. 端点：`getappapi.index/initV119`（首页分类+推荐）、`typeFilterVodList?page=`（分类）、
//!    `vodDetail`（详情）、`searchList`（搜索）、`vodParse`（带签名 `app-api-verify-sign` 的解析）。
//!
//! 有意偏差（均在代码中注明）：
//! - ext 只支持 `url` 直连（原实现还支持 `site` 拉取 URL 列表并用 HEAD 探测可用性）；
//! - 播放直链分支剥掉 Android 侧的 `{名称}$` 前缀（原实现把整串当 URL）；
//! - `vodParse` 失败时返回明确错误码，而不是空字符串。

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use regex::Regex;
use serde_json::{json, Value};

use super::super::contract::{CatvodError, ErrorClass};
use crate::error::Result;
use crate::http::{client_for, get_text, post_json_text};
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

pub const API: &str = "csp_AppGet";

const DEFAULT_UA: &str = "okhttp/3.14.9";
/// 原实现直链/投屏分支使用 Chrome UA。
const PLAY_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36";
const SKIP_CATEGORIES: [&str; 3] = ["伦理", "福利", "小影院"];
const DIRECT_EXT: [&str; 3] = [".m3u8", ".mp4", ".mkv"];

fn parse_api_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"parse_api=(.*?)(?:&token|$)").expect("parse_api 正则合法"))
}

fn url_param_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:&|&amp;|\?)url=([^&]*)").expect("url 参数正则合法"))
}

/// ext JSON 解析出的运行配置。
#[derive(Debug, Clone)]
pub struct Config {
    pub base: String,
    pub key: String,
    pub iv: String,
    pub device_id: String,
    pub version: String,
    pub ua: String,
    pub token: String,
}

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn config(site: &Site) -> Result<Config> {
    let raw = site.ext_string();
    let value: Value = serde_json::from_str(raw.trim()).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_AppGet 的 ext 必须是 JSON 对象: {e}"),
        )
        .into_core()
    })?;
    let base = value
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "csp_AppGet 的 ext 缺少有效的 url 基址（原实现支持 site 动态拉取，本实现暂不支持）",
        )
        .into_core());
    }
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    Ok(Config {
        base: base.trim_end_matches('/').to_string(),
        key: text("dataKey"),
        iv: text("dataIv"),
        device_id: text("deviceId"),
        version: text("version"),
        ua: text("ua"),
        token: text("token"),
    })
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// AES-128-CBC 解密（Base64 标准编码，PKCS#7 填充；与 Android 侧 `merge.DM.m.a.b` 等价）。
pub fn aes_cbc_decrypt_b64(text: &str, key: &str, iv: &str) -> std::result::Result<String, String> {
    use aes::Aes128;
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    let raw = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|e| format!("Base64 解码失败: {e}"))?;
    let cipher = cbc::Decryptor::<Aes128>::new_from_slices(key.as_bytes(), iv.as_bytes())
        .map_err(|e| format!("AES-CBC 参数错误（key/iv 需 16 字节）: {e}"))?;
    let plain = cipher
        .decrypt_padded_vec_mut::<Pkcs7>(&raw)
        .map_err(|_| "AES-CBC 解密失败（key/iv/填充不匹配）".to_string())?;
    String::from_utf8(plain).map_err(|e| format!("解密结果不是 UTF-8: {e}"))
}

/// AES-128-CBC 加密（PKCS#7 + Base64 标准编码；等价 `merge.DM.m.a.i` 再 Base64）。
pub fn aes_cbc_encrypt_b64(text: &str, key: &str, iv: &str) -> std::result::Result<String, String> {
    use aes::Aes128;
    use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    let cipher = cbc::Encryptor::<Aes128>::new_from_slices(key.as_bytes(), iv.as_bytes())
        .map_err(|e| format!("AES-CBC 参数错误（key/iv 需 16 字节）: {e}"))?;
    let raw = cipher.encrypt_padded_vec_mut::<Pkcs7>(text.as_bytes());
    Ok(base64::engine::general_purpose::STANDARD.encode(raw))
}

impl Config {
    fn headers(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert(
            "User-Agent".to_string(),
            if self.ua.is_empty() {
                DEFAULT_UA.to_string()
            } else {
                self.ua.clone()
            },
        );
        headers.insert(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );
        headers.insert("app-user-device-id".to_string(), self.device_id.clone());
        headers.insert("app-version-code".to_string(), self.version.clone());
        if !self.token.is_empty() {
            headers.insert("app-user-token".to_string(), self.token.clone());
        }
        headers.insert(
            "app-api-verify-time".to_string(),
            now_secs().to_string(),
        );
        headers.insert("app-ui-mode".to_string(), "light".to_string());
        headers
    }

    fn decode_envelope(&self, site: &Site, text: &str) -> Result<Value> {
        let envelope: Value = serde_json::from_str(text).map_err(|e| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                format!("csp_AppGet 响应不是 JSON: {e}"),
            )
            .into_core()
        })?;
        let data = envelope
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CatvodError::new(
                    ErrorClass::HostApi,
                    site,
                    format!(
                        "csp_AppGet 响应缺少 data 字段: {}",
                        truncate(text, 120)
                    ),
                )
                .into_core()
            })?;
        let plain = aes_cbc_decrypt_b64(data, &self.key, &self.iv).map_err(|e| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                format!("csp_AppGet 响应解密失败: {e}"),
            )
            .into_core()
        })?;
        serde_json::from_str(&plain).map_err(|e| {
            CatvodError::new(
                ErrorClass::Parse,
                site,
                format!("csp_AppGet 解密后不是 JSON: {e}"),
            )
            .into_core()
        })
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        text.chars().take(limit).collect::<String>() + "…"
    }
}

fn call(
    cfg: &TvConfig,
    site: &Site,
    app: &Config,
    path: &str,
    body: &str,
) -> Result<Value> {
    let url = format!("{}/api.php{}", app.base, path);
    let client = client_for(&cfg.http)?;
    let text = post_json_text(&client, &url, &app.headers(), body).map_err(|e| {
        CatvodError::new(
            ErrorClass::JarFetch,
            site,
            format!("csp_AppGet 请求失败 {url}: {e}"),
        )
        .into_core()
    })?;
    app.decode_envelope(site, &text)
}

fn list_items(value: &Value, key: &str) -> Vec<VideoItem> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let id = row.get("vod_id")?.as_str().map(str::to_string).or_else(|| {
                        row.get("vod_id")
                            .and_then(Value::as_i64)
                            .map(|n| n.to_string())
                    })?;
                    let name = row.get("vod_name")?.as_str().unwrap_or("").trim().to_string();
                    let cover = row
                        .get("vod_pic")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .filter(|s| !s.is_empty());
                    let remark = row
                        .get("vod_remarks")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .filter(|s| !s.is_empty());
                    Some(VideoItem {
                        id,
                        name,
                        cover,
                        remark,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let app = config(site)?;
    let value = call(cfg, site, &app, "/getappapi.index/initV119", "{}")?;
    let mut categories = Vec::new();
    if let Some(rows) = value.get("type_list").and_then(Value::as_array) {
        for row in rows {
            let id = row
                .get("type_id")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            let name = row
                .get("type_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if id.is_empty() || name.is_empty() || SKIP_CATEGORIES.contains(&name.as_str()) {
                continue;
            }
            categories.push(Category {
                type_id: id,
                type_name: name,
            });
        }
    }
    Ok(categories)
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let app = config(site)?;
    let path = format!("/getappapi.index/typeFilterVodList?page={}", page.max(1));
    let body = json!({ "type_id": cat }).to_string();
    let value = call(cfg, site, &app, &path, &body)?;
    Ok(list_items(&value, "recommend_list"))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let app = config(site)?;
    let body = json!({ "type_id": 0, "keywords": keyword, "page": 1 }).to_string();
    let value = call(cfg, site, &app, "/getappapi.index/searchList", &body)?;
    Ok(list_items(&value, "search_list"))
}

/// 构造详情里的播放串（与 Android 侧 `detailContent` 完全一致）。
fn play_entry(app: &Config, item: &Value, vod_name: &str, parse: &str) -> Option<String> {
    let name = item.get("name").and_then(Value::as_str).unwrap_or("").trim();
    let url = item.get("url").and_then(Value::as_str).unwrap_or("").trim();
    if url.is_empty() {
        return None;
    }
    let token = item.get("token").and_then(Value::as_str).unwrap_or("");
    let nid = item.get("nid").and_then(Value::as_str).unwrap_or("");
    let parse_api_url = item
        .get("parse_api_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let entry = if parse_api_url.starts_with("http://") || parse_api_url.starts_with("https://")
    {
        format!("{name}${parse_api_url}|{vod_name}|{nid}")
    } else {
        let encrypted = aes_cbc_encrypt_b64(url, &app.key, &app.iv)
            .unwrap_or_default();
        format!(
            "{name}$parse_api={parse}&url={encrypted}&token={token}|{vod_name}|{nid}"
        )
    };
    Some(entry)
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let app = config(site)?;
    let body = json!({ "vod_id": id }).to_string();
    let value = call(cfg, site, &app, "/getappapi.index/vodDetail", &body)?;
    let vod = value.get("vod").ok_or_else(|| {
        CatvodError::new(ErrorClass::Parse, site, "csp_AppGet 详情缺少 vod 字段").into_core()
    })?;
    let text = |key: &str| {
        vod.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let vod_name = text("vod_name");
    let mut groups = Vec::new();
    if let Some(rows) = value.get("vod_play_list").and_then(Value::as_array) {
        for row in rows {
            let player = row.get("player_info").unwrap_or(&Value::Null);
            let show = player
                .get("show")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let parse = player
                .get("parse")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            let mut items = Vec::new();
            if let Some(urls) = row.get("urls").and_then(Value::as_array) {
                for item in urls {
                    if let Some(entry) = play_entry(&app, item, &vod_name, &parse) {
                        let title = item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        items.push(Episode {
                            title: if title.is_empty() {
                                format!("第{}集", items.len() + 1)
                            } else {
                                title
                            },
                            url: entry,
                        });
                    }
                }
            }
            if items.is_empty() {
                continue;
            }
            groups.push(EpisodeGroup {
                group: if show.is_empty() {
                    format!("线路{}", groups.len() + 1)
                } else {
                    show
                },
                items,
            });
        }
    }
    if groups.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "csp_AppGet 详情没有可播放线路",
        )
        .into_core());
    }
    let year = text("vod_year");
    let area = text("vod_area");
    Ok(VideoDetail {
        id: id.to_string(),
        name: vod_name,
        cover: Some(text("vod_pic")).filter(|s| !s.is_empty()),
        desc: Some(text("vod_content")).filter(|s| !s.is_empty()),
        year: Some(year).filter(|s| !s.is_empty()),
        area: Some(area).filter(|s| !s.is_empty()),
        director: Some(text("vod_director")).filter(|s| !s.is_empty()),
        actor: Some(text("vod_actor")).filter(|s| !s.is_empty()),
        episodes: groups,
    })
}

/// 播放串结构：`{名称}$...|{vod_name}|{nid}`。
fn split_play_id(id: &str) -> (&str, &str, &str) {
    let mut parts = id.splitn(3, '|');
    (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    )
}

fn strip_name_prefix(segment: &str) -> &str {
    segment.rsplit_once('$').map(|(_, rest)| rest).unwrap_or(segment)
}

fn play_json(url: &str, ua: &str) -> Value {
    json!({
        "parse": 0,
        "url": url,
        "playUrl": "",
        "header": { "User-Agent": ua },
    })
}

/// `vodParse`：带签名头的解析接口，返回最终直链。
fn vod_parse(cfg: &TvConfig, site: &Site, app: &Config, body: &str) -> Result<String> {
    let ts = now_secs().to_string();
    let sign = aes_cbc_encrypt_b64(&ts, &app.key, &app.iv).map_err(|e| {
        CatvodError::new(ErrorClass::Parse, site, format!("签名加密失败: {e}")).into_core()
    })?;
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), DEFAULT_UA.to_string());
    headers.insert("Connection".to_string(), "Keep-Alive".to_string());
    headers.insert(
        "Content-Type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    );
    headers.insert("app-version-code".to_string(), app.version.clone());
    if !app.token.is_empty() {
        headers.insert("app-user-token".to_string(), app.token.clone());
    }
    headers.insert("app-ui-mode".to_string(), "light".to_string());
    headers.insert("app-user-device-id".to_string(), app.device_id.clone());
    headers.insert("app-api-verify-time".to_string(), ts);
    headers.insert("app-api-verify-sign".to_string(), sign);
    let url = format!("{}/api.php/getappapi.index/vodParse", app.base);
    let client = client_for(&cfg.http)?;
    let text = post_json_text(&client, &url, &headers, body).map_err(|e| {
        CatvodError::new(
            ErrorClass::HostApi,
            site,
            format!("csp_AppGet 解析接口请求失败: {e}"),
        )
        .into_core()
    })?;
    let value = app.decode_envelope(site, &text)?;
    let json_field = value.get("json").cloned().unwrap_or(Value::Null);
    let inner = if let Some(text) = json_field.as_str() {
        serde_json::from_str::<Value>(text).unwrap_or(Value::Null)
    } else {
        json_field
    };
    inner
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CatvodError::new(ErrorClass::HostApi, site, "csp_AppGet 解析接口没有返回 url").into_core()
        })
}

fn decode_url_params(app: &Config, text: &str) -> String {
    url_param_re()
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let raw = &caps[1];
            match aes_cbc_decrypt_b64(raw, &app.key, &app.iv) {
                Ok(plain) => format!("url={plain}"),
                Err(_) => format!("url={raw}"),
            }
        })
        .to_string()
}

fn encode_url_params(text: &str) -> String {
    url_param_re()
        .replace_all(text, |caps: &regex::Captures<'_>| {
            format!("url={}", percent_encoding::utf8_percent_encode(&caps[1], percent_encoding::NON_ALPHANUMERIC))
        })
        .to_string()
}

pub fn player_content(
    cfg: &TvConfig,
    site: &Site,
    _flag: &str,
    id: &str,
) -> Result<Value> {
    let app = config(site)?;
    let (segment, _vod_name, _nid) = split_play_id(id);
    let client = client_for(&cfg.http)?;

    // 分支一：带 ?url= / ?key= 的解析页
    if segment.starts_with("http://") || segment.starts_with("https://") {
        if segment.contains("?url=") || segment.contains("?key=") {
            let text = get_text(&client, segment, &HashMap::new(), 2).unwrap_or_default();
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if let Some(url) = value.get("url").and_then(Value::as_str) {
                    return Ok(play_json(url, PLAY_UA));
                }
            }
        }
    }

    let bare = strip_name_prefix(segment);

    // 分支二：直链
    if DIRECT_EXT.iter().any(|ext| bare.contains(ext)) {
        return Ok(play_json(bare, PLAY_UA));
    }

    // 分支三：`?url=` / `?key=` / html 类，先解出 url= 参数再看 parse_api
    if bare.contains("?url=") || bare.contains("?key=") || bare.contains("html") {
        let decoded = decode_url_params(&app, bare);
        if let Some(caps) = parse_api_re().captures(&decoded) {
            let api_url = caps[1].trim();
            if api_url.starts_with("http") {
                let text = get_text(&client, api_url, &HashMap::new(), 2).unwrap_or_default();
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    let url = value
                        .get("data")
                        .and_then(|d| d.get("url"))
                        .and_then(Value::as_str)
                        .or_else(|| value.get("url").and_then(Value::as_str));
                    if let Some(url) = url.filter(|u| !u.is_empty()) {
                        return Ok(play_json(url, PLAY_UA));
                    }
                }
            }
        }
    }

    // 分支四：`vodParse`
    let body = encode_url_params(bare);
    let url = vod_parse(cfg, site, &app, &body)?;
    Ok(play_json(&url, PLAY_UA))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const KEY: &str = "0123456789abcdef";
    const IV: &str = "fedcba9876543210";
    const HOME_VEC: &str = "N2VihQRYIhfgDJiQWeiREP5YrPQXWIHswemLsKsMo/m01KoXEn/WP/wy4nswbdq+BAYwNtaq4Eoq4Qm7iCAhxHoS5kxlcW6t5rNMg8FpteG577Pve9pGU0+l4JmEzt0FzDmm8OqMF6QaL/bMg9l9aTw7SI7j56DrC6CnR0iMqykN22z0jXCTPBbpZ55Fv92vWbeYEizTdG1fhnKju+sLCPCDVlWL1CYfKjjGvl4Fg2t3z9TmLSzEOj+2xdk7/sYcM/UstLioxLc+xsA8rdjuM0CkifrlXPC+gAj2KOwPoXb0AlQWniuAD0kaEdN67mPI1N39VzD9v+FVJ+atJ7kJFw==";
    const LIST_VEC: &str = "ReDkzkVE8zUe/Yt1fc3IcWSRg2CTo0i/MeUZ9+OB0Ynqou4UOiN434Gs9r94i9aEuu+LZLPXS4OX/MT6yu0LQ/elN/y9Gf3qlQhZLxYMrv1Mr56pSWKfcm5zA1n5nkPcIUBzMm6hxW9Rb8EJ/JEdhbpJ2Sh63sAzGCDmZ+B8RqjxSB7/P//a7VEKFb3hH9EDcHfdS95xhYJ/mx8LMzxOHszpFoA+SOEUqx306JZOLSBySTD+pzMOblodgKanpucLM+OBn8FuMkckAaCt3IfT+w==";

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_AppGet"));
        assert!(handles(" CSP_APPGET "));
        assert!(!handles("csp_App"));
    }

    #[test]
    fn aes_roundtrip() {
        let plain = r#"{"ok":true}"#;
        let enc = aes_cbc_encrypt_b64(plain, KEY, IV).unwrap();
        assert_eq!(aes_cbc_decrypt_b64(&enc, KEY, IV).unwrap(), plain);
        assert_eq!(aes_cbc_decrypt_b64(HOME_VEC, KEY, IV).unwrap().contains("测试影片"), true);
    }

    #[test]
    fn parses_home_payload() {
        let value: Value =
            serde_json::from_str(&aes_cbc_decrypt_b64(HOME_VEC, KEY, IV).unwrap()).unwrap();
        let mut cats = Vec::new();
        for row in value["type_list"].as_array().unwrap() {
            let name = row["type_name"].as_str().unwrap();
            if SKIP_CATEGORIES.contains(&name) {
                continue;
            }
            cats.push(Category {
                type_id: row["type_id"].to_string().trim_matches('"').to_string(),
                type_name: name.to_string(),
            });
        }
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[1].type_name, "电视剧");
        let items = list_items(&value, "recommend_list");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "11");
    }

    #[test]
    fn parses_list_payload() {
        let value: Value =
            serde_json::from_str(&aes_cbc_decrypt_b64(LIST_VEC, KEY, IV).unwrap()).unwrap();
        let items = list_items(&value, "recommend_list");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].name, "分类影片B");
        assert_eq!(items[0].remark.as_deref(), Some("HD"));
    }

    #[test]
    fn play_entry_encrypts_url() {
        let app = Config {
            base: "http://api.test".into(),
            key: KEY.into(),
            iv: IV.into(),
            device_id: "dev".into(),
            version: "9".into(),
            ua: String::new(),
            token: "tok".into(),
        };
        let item = json!({"name":"第1集","url":"http://cdn/ep1.m3u8","token":"t1","nid":"n1","parse_api_url":""});
        let entry = play_entry(&app, &item, "影片", "1").unwrap();
        assert!(entry.starts_with("第1集$parse_api=1&url="));
        assert!(entry.ends_with("&token=t1|影片|n1"));
        let encoded = entry.split("&url=").nth(1).unwrap().split('&').next().unwrap();
        assert_eq!(aes_cbc_decrypt_b64(encoded, KEY, IV).unwrap(), "http://cdn/ep1.m3u8");
    }

    #[test]
    fn play_entry_uses_parse_api_url_when_http() {
        let app = Config {
            base: "http://api.test".into(),
            key: KEY.into(),
            iv: IV.into(),
            device_id: String::new(),
            version: String::new(),
            ua: String::new(),
            token: String::new(),
        };
        let item = json!({"name":"正片","url":"http://cdn/ep1.m3u8","parse_api_url":"https://jx/play?url=http://cdn/ep1.m3u8","nid":"n9"});
        assert_eq!(
            play_entry(&app, &item, "影片", "1").unwrap(),
            "正片$https://jx/play?url=http://cdn/ep1.m3u8|影片|n9"
        );
    }

    #[test]
    fn split_play_id_and_prefix() {
        let (seg, vod, nid) = split_play_id("第1集$parse_api=1&url=xx&token=t|影片|n1");
        assert!(seg.starts_with("第1集$"));
        assert_eq!(vod, "影片");
        assert_eq!(nid, "n1");
        assert_eq!(strip_name_prefix(seg), "parse_api=1&url=xx&token=t");
    }

    #[test]
    fn encode_and_decode_url_param() {
        let app = Config {
            base: String::new(),
            key: KEY.into(),
            iv: IV.into(),
            device_id: String::new(),
            version: String::new(),
            ua: String::new(),
            token: String::new(),
        };
        let enc = aes_cbc_encrypt_b64("http://cdn/a b.m3u8", KEY, IV).unwrap();
        let decoded = decode_url_params(&app, &format!("parse_api=1&url={enc}&token=t"));
        assert!(decoded.contains("url=http://cdn/a b.m3u8"));
        let encoded = encode_url_params("parse_api=1&url=http://cdn/a?x=1&token=t");
        assert!(encoded.contains("url=http%3A%2F%2Fcdn%2Fa%3Fx%3D1"));
    }
}
