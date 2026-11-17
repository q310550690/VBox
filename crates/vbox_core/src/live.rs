//! 直播源解析（对齐 jun 版 TVBoxOS 的 `TxtSubscribe` / `ApiConfig.loadLives`）
//!
//! 覆盖 TVBox 直播的两种输入：
//!
//! 1. **订阅文本**（`parse_txt_or_m3u`）：
//!    - txt：`组名,#genre#` 切组，`频道名,url1#url2` 一行多源，`url$源名` 给源起名；
//!    - m3u：`#EXTM3U` / `#EXTINF group-title/tvg-*` / `#EXTHTTP:` / `#EXTVLCOPT:` /
//!      `#KODIPROP:` 设置行 / 行内 `url|header&...`；
//!    - JSON 数组：`[{ "group": ..., "channels": [{ "name", "urls": [...] }] }]`
//!      （兼容 `channel` 键名与单组对象）。
//! 2. **`lives[]` 配置项**（`load_live_source`）：type0 拉订阅文本；type3 的 `.js`/`.py`
//!    走对应 spider 运行时（`.py` 需系统 Python），jar 返回明确错误而不是空列表。
//!
//! 几个最容易踩错的点（均已按参考实现对齐）：
//!
//! - **分组密码的真实分隔符是 `_`，不是 `#`**：`福利_1234,#genre#` → 组名“福利”、
//!   密码“1234”（`ApiConfig.loadLives` 的 `groupName.split("_", 2)`，`#genre#` 的 `#`
//!   只是 txt 分组标记）。
//! - 多源地址：txt 里用 `#` 连接；m3u 里同一个频道名可多次出现；`url$源名` 的 `$` 之后
//!   是源名，缺省“源N”（按最终 url 位置编号）。
//! - 同组同名频道合并 urls（按 url 去重、元数据只补空），对应 `TxtSubscribe.addChannel`
//!   与 `ApiConfig.mergeLiveChannel`。
//! - `#EXTHTTP:` / `#EXTVLCOPT:` / `#KODIPROP:` / `ua=` 等设置行作用于**后续频道**；
//!   设置行在 `#genre#` 切组时被清空（与参考实现一致）。
//! - `proxy://do=live&type=txt&ext=<base64>` 只是把真实订阅地址编码进 ext；VBox 不需要
//!   本地 9978 代理，直接用 `resolve_live_address` 解码后请求即可
//!   （`Base64.URL_SAFE | NO_WRAP`，同时容忍带 padding / 标准表 / 百分号转义）。

use std::collections::HashMap;

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{CoreError, Result};
use crate::http;
use crate::model::{LiveSource, Site, TvConfig};

/// txt 里没有 `#genre#` 时使用的默认组名
pub const DEFAULT_GROUP_NAME: &str = "直播";
/// 老版本 TVBox 的默认组名，归一化为 [`DEFAULT_GROUP_NAME`]
const LEGACY_DEFAULT_GROUP_NAME: &str = "Ungrouped";

/* ============================================================
 * 数据模型（serde camelCase，供 Tauri 命令 / 前端直接使用）
 * ============================================================ */

/// 频道的单个播放源
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LiveUrl {
    pub url: String,
    /// 源名；文本里 `url$源名` 的 `$` 之后部分，缺省按位置生成“源N”
    #[serde(default)]
    pub name: String,
}

/// 单个直播频道
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LiveChannel {
    pub name: String,
    #[serde(default)]
    pub logo: String,
    /// 频道级 EPG 标识：m3u 的 `tvg-url`，缺省回退 `tvg-id`（XMLTV 的 channel id）
    #[serde(default)]
    pub epg: String,
    /// 所属组 key（= [`LiveGroup::key`]）
    #[serde(default)]
    pub group_key: String,
    /// 多源地址（url + 源名），换源时按序取用
    #[serde(default)]
    pub urls: Vec<LiveUrl>,
    #[serde(default)]
    pub ua: String,
    /// 频道级请求头（`#EXTHTTP:` / `url|header&...` / JSON `header`）
    #[serde(default)]
    pub header: HashMap<String, String>,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub referer: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub click: String,
    /// 频道级解析开关（参考实现只存不用；VBox 预留）
    #[serde(default)]
    pub parse: i32,
    /// catchup 对象：`{type, source, replace}`（m3u 的 `catchup*` 属性）
    #[serde(default)]
    pub catchup: Option<Value>,
}

impl LiveChannel {
    /// 当前默认播放地址（第一个源）
    pub fn first_url(&self) -> Option<&str> {
        self.urls.first().map(|u| u.url.as_str())
    }
}

/// 一个直播分组（`#genre#` 段 / m3u 的 group-title / JSON 的一个数组元素）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LiveGroup {
    /// 组标识：``组名_密码`` 的原始（归一化后）名字，密码分组也唯一
    pub key: String,
    /// 展示名（已去掉 `_密码` 后缀）
    pub name: String,
    /// 密码分组的密码；空串表示公开分组
    #[serde(default)]
    pub password: String,
    /// 组级 EPG 地址：m3u 头部的 `x-tvg-url` / `tvg-url`（TVBox 写全局 EPG_URL）
    #[serde(default)]
    pub epg: String,
    #[serde(default)]
    pub channels: Vec<LiveChannel>,
}

impl LiveGroup {
    /// 是否是需要密码才能查看频道的加密分组
    pub fn is_locked(&self) -> bool {
        !self.password.is_empty()
    }
}

/* ============================================================
 * 对外入口：订阅文本解析
 * ============================================================ */

/// 解析直播订阅文本：JSON 数组 / m3u / txt 自动识别。
///
/// `source_url` 仅用于错误信息与排障（不参与解析）。
pub fn parse_txt_or_m3u(content: &str, source_url: &str) -> Result<Vec<LiveGroup>> {
    let text = strip_bom(content).trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }

    // 1) JSON（参考 TxtSubscribe.parseToJsonArray：先试 JSON，失败再按 m3u/txt 兜底）
    if text.starts_with('[') || text.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            return parse_json_live_groups(value, source_url);
        }
    }

    // 2) m3u
    if text.starts_with("#EXTM3U") {
        let epg = extract_m3u_epg(text).unwrap_or_default();
        let mut groups = parse_m3u(text);
        // TVBox 把 m3u 头 x-tvg-url 写到全局 EPG_URL；VBox 没有全局状态，
        // 挂到每个组上供前端 `live_epg` 使用。
        for group in &mut groups {
            group.epg = epg.clone();
        }
        return Ok(groups);
    }

    // 3) txt
    Ok(parse_txt(text))
}

/// 从 m3u 头部提取 EPG 地址（`x-tvg-url` → `tvg-url` → `url-tvg`）。
///
/// 参考实现把它写到全局 `Hawk(EPG_URL)`；VBox 没有全局状态，交给调用方保存。
pub fn extract_m3u_epg(content: &str) -> Option<String> {
    for raw in content.lines() {
        let line = strip_bom(raw).trim();
        if !line.starts_with("#EXTM3U") {
            continue;
        }
        return attr(line, "x-tvg-url")
            .or_else(|| attr(line, "tvg-url"))
            .or_else(|| attr(line, "url-tvg"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    None
}

/* ============================================================
 * 对外入口：按 lives[] 配置加载
 * ============================================================ */

/// 按 `lives[]` 配置项加载直播分组。
///
/// - `type=0`：拉取 `url`（为空则 `api`）指向的 txt/m3u/JSON 并解析；
///   `proxy://do=live&type=txt&ext=<base64>` 会先解码 ext，再请求真实地址。
/// - `type=3`：`.js` 走 [`crate::spider::js::live`]；`.py` 走 [`crate::python::live_content`]；
///   jar 返回明确错误。
pub fn load_live_source(cfg: &TvConfig, live: &LiveSource) -> Result<Vec<LiveGroup>> {
    load_live_source_inner(cfg, live).map_err(|e| map_live_error(live, e))
}

/// 把直播源加载的裸错误收口成 catvod 稳定码（网络层 → `catvod.net.*`，
/// 其余 → `catvod.live.fetch`）。已是 `catvod.*` 的原样透传。
fn map_live_error(live: &LiveSource, err: CoreError) -> CoreError {
    use crate::spider::catvod::contract::{CatvodError, ErrorClass};

    let msg = err.to_string();
    if msg.contains("catvod.") {
        return err;
    }
    let class = match &err {
        CoreError::Http(e) => ErrorClass::classify_reqwest_error(e),
        _ => ErrorClass::LiveFetch,
    };
    let site = Site {
        key: if live.name.trim().is_empty() {
            live.address().to_string()
        } else {
            live.name.trim().to_string()
        },
        name: live.name.trim().to_string(),
        api: live.address().to_string(),
        ..Default::default()
    };
    CatvodError::new(class, &site, msg).into_core()
}

fn load_live_source_inner(cfg: &TvConfig, live: &LiveSource) -> Result<Vec<LiveGroup>> {
    match live.kind {
        0 => load_text_live_source(cfg, live),
        3 => {
            let address = live.address();
            if address.is_empty() {
                return Err(CoreError::Config(format!(
                    "直播源「{}」缺少 url/api",
                    live_label(live)
                )));
            }
            // type3 也可能被 proxy:// 包一层（参考 loadLiveApi 会统一改写成代理地址）
            let target = resolve_live_address(address)?;
            if is_js_spider(&target) || target.starts_with("inline:") {
                load_js_live_source(cfg, live, &target)
            } else if target.to_ascii_lowercase().contains(".py") {
                load_py_live_source(cfg, live, &target)
            } else {
                Err(unsupported_live_spider(&target))
            }
        }
        k => Err(CoreError::Config(format!(
            "不支持的直播源类型 type={k}（TVBox 只定义 0=文本订阅 / 3=spider）"
        ))),
    }
}

/// type3 `.js`：把 `LiveSource` 适配成最小 `Site`，调用 `live(url)` 拿频道文本再解析。
///
/// 参考 `LivePlayActivity.loadProxyLives`：解码后的订阅地址含 `.js`/`.py` 时走
/// `getLiveCSP(url).liveContent(url)`；这里对应 `spider::js::live(site, cfg, url)`。
fn load_js_live_source(
    cfg: &TvConfig,
    live: &LiveSource,
    script_url: &str,
) -> Result<Vec<LiveGroup>> {
    let site = Site {
        key: if live.name.trim().is_empty() {
            script_url.to_string()
        } else {
            live.name.trim().to_string()
        },
        name: live.name.trim().to_string(),
        kind: 3,
        api: script_url.to_string(),
        ext: live.ext.clone(),
        jar: live.jar.clone(),
        ..Default::default()
    };
    let text = crate::spider::js::live(&site, cfg, script_url)?;
    parse_txt_or_m3u(&text, script_url)
}

/// type3 `.py`：Python spider 的 `liveContent(url)`，返回 m3u/txt/JSON 文本。
fn load_py_live_source(
    cfg: &TvConfig,
    live: &LiveSource,
    script_url: &str,
) -> Result<Vec<LiveGroup>> {
    let site = Site {
        key: if live.name.trim().is_empty() {
            script_url.to_string()
        } else {
            live.name.trim().to_string()
        },
        name: live.name.trim().to_string(),
        kind: 3,
        api: script_url.to_string(),
        ext: live.ext.clone(),
        ..Default::default()
    };
    let text = crate::python::live_content(cfg, &site, script_url)?;
    parse_txt_or_m3u(&text, script_url)
}

fn load_text_live_source(cfg: &TvConfig, live: &LiveSource) -> Result<Vec<LiveGroup>> {
    let address = live.address();
    if address.is_empty() {
        return Err(CoreError::Config(format!(
            "直播源「{}」缺少 url/api",
            live_label(live)
        )));
    }

    let target = resolve_live_address(address)?;
    // proxy:// 解出来的地址如果还是 spider 脚本，参考实现会走 getLiveCSP().liveContent()
    // （LivePlayActivity.loadProxyLives），桌面端暂不支持，明确报错。
    if is_spider_script(&target) {
        return Err(unsupported_live_spider(&target));
    }

    let text = fetch_live_text(cfg, live, &target)?;
    parse_txt_or_m3u(&text, &target)
}

/// 拉取直播订阅文本（http(s) / file://）。
fn fetch_live_text(cfg: &TvConfig, live: &LiveSource, target: &str) -> Result<String> {
    if let Some(path) = target.strip_prefix("file://") {
        let path = if cfg!(windows) {
            path.trim_start_matches('/')
        } else {
            path
        };
        return std::fs::read_to_string(path).map_err(CoreError::Io);
    }

    let lower = target.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err(CoreError::Config(format!(
            "不支持的直播订阅地址协议: {target}（仅支持 http(s) / file://）"
        )));
    }

    // 组级 header 优先；没有 header 时才用组级 ua（对齐 ApiConfig.loadLiveApi）
    let mut extra = HashMap::new();
    let has_header = matches!(&live.header, Some(Value::Object(map)) if !map.is_empty());
    if has_header {
        if let Some(Value::Object(map)) = &live.header {
            for (k, v) in map {
                if let Some(s) = json_scalar_string(v) {
                    extra.insert(k.clone(), s);
                }
            }
        }
    } else if !live.ua.trim().is_empty() {
        extra.insert("User-Agent".to_string(), live.ua.trim().to_string());
    }

    let client = http::client_for(&cfg.http)?;
    http::get_text(&client, target, &extra, cfg.http.retry)
}

/// 把 TVBox 的代理地址还原成真实订阅地址。
///
/// 支持两种形态（参考 `DefaultConfig.checkReplaceProxy` + `LivePlayActivity.loadProxyLives`）：
/// - `proxy://do=live&type=txt&ext=<ext>`；
/// - 已经被替换成本地服务的 `http://127.0.0.1:9978/proxy?do=live&type=txt&ext=<ext>`。
///
/// `ext` 规则：`http(s)://` / `clan://` 原样使用；否则先按 `Base64.URL_SAFE|NO_WRAP`
/// 解码（容忍 padding、标准表、百分号转义）。非代理地址原样返回。
pub fn resolve_live_address(address: &str) -> Result<String> {
    let address = address.trim();
    if address.is_empty() {
        return Err(CoreError::Config("直播订阅地址为空".into()));
    }

    let query = if let Some(rest) = address.strip_prefix("proxy://") {
        Some(rest.trim_start_matches('?'))
    } else if address.contains("/proxy?")
        && (address.starts_with("http://127.0.0.1") || address.starts_with("http://localhost"))
    {
        address.split_once("/proxy?").map(|(_, q)| q)
    } else {
        None
    };

    let Some(query) = query else {
        if address.starts_with("clan://") {
            return Err(CoreError::Config(format!(
                "clan:// 直播地址需要本地文件服务，核心库无法直接读取: {address}"
            )));
        }
        return Ok(address.to_string());
    };

    let mut ext: Option<String> = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key.trim() != "ext" {
            continue;
        }
        ext = Some(percent_decode_str(value).decode_utf8_lossy().to_string());
        break;
    }
    let Some(ext) = ext else {
        return Err(CoreError::Config(format!(
            "直播代理地址缺少 ext 参数: {address}"
        )));
    };
    let ext = ext.trim();
    if ext.is_empty() {
        return Err(CoreError::Config(format!(
            "直播代理地址的 ext 参数为空: {address}"
        )));
    }

    let lower = ext.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || ext.starts_with("clan://") {
        return Ok(ext.to_string());
    }

    let bytes = decode_base64_any(ext).ok_or_else(|| {
        CoreError::Config(format!("直播代理地址的 ext 不是合法 Base64: {address}"))
    })?;
    let decoded = String::from_utf8(bytes).map_err(|_| {
        CoreError::Config(format!(
            "直播代理地址的 ext Base64 解码后不是 UTF-8: {address}"
        ))
    })?;
    let decoded = decoded.trim().to_string();
    if decoded.is_empty() {
        return Err(CoreError::Config(format!(
            "直播代理地址的 ext 解码后为空: {address}"
        )));
    }
    Ok(decoded)
}

/// 参考 `ApiConfig.isLiveSpiderApi`：api 里包含 `.py`/`.js` 即视作 spider 直播源。
fn is_spider_script(api: &str) -> bool {
    is_js_spider(api) || api.to_ascii_lowercase().contains(".py")
}

fn is_js_spider(api: &str) -> bool {
    api.to_ascii_lowercase().contains(".js")
}

fn unsupported_live_spider(address: &str) -> CoreError {
    let lower = address.to_ascii_lowercase();
    if lower.contains(".js") {
        CoreError::Config(format!(
            "直播 JS spider 无法执行（脚本缺少 live() 或运行失败）：{address}"
        ))
    } else if lower.contains(".py") {
        CoreError::Config(format!(
            "直播 Python spider 需要系统 Python 且脚本可运行：{address}"
        ))
    } else {
        CoreError::Config(format!(
            "直播 jar/csp spider 后续支持（需 Android Host）：{address}"
        ))
    }
}

fn live_label(live: &LiveSource) -> String {
    let name = live.name.trim();
    if name.is_empty() {
        live.address().to_string()
    } else {
        name.to_string()
    }
}

/* ============================================================
 * JSON 数组形态
 * ============================================================ */

fn parse_json_live_groups(value: Value, source_url: &str) -> Result<Vec<LiveGroup>> {
    match value {
        Value::Array(items) => Ok(parse_json_groups(&items)),
        Value::Object(obj) => {
            if obj.contains_key("channels") || obj.contains_key("channel") {
                return Ok(parse_json_groups(&[Value::Object(obj)]));
            }
            if let Some(Value::Array(items)) = obj.get("groups") {
                return Ok(parse_json_groups(items));
            }
            if obj.contains_key("lives") {
                return Err(CoreError::Config(format!(
                    "{source_url}: 这是完整 TVBox 配置 JSON（含 lives），请从 lives[].url 取订阅地址后再解析；\
                     直播组 JSON 应为数组 [{{group, channels}}]"
                )));
            }
            Ok(Vec::new())
        }
        _ => Ok(Vec::new()),
    }
}

fn parse_json_groups(items: &[Value]) -> Vec<LiveGroup> {
    let mut groups = Vec::new();
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let raw_name = obj
            .get("group")
            .and_then(Value::as_str)
            .or_else(|| obj.get("name").and_then(Value::as_str))
            .unwrap_or("");
        let idx = find_or_create_group(&mut groups, raw_name);
        let channels = obj.get("channels").or_else(|| obj.get("channel"));
        if let Some(Value::Array(channels)) = channels {
            for channel in channels {
                if let Some(channel) = channel.as_object().and_then(json_channel) {
                    add_or_merge_channel(&mut groups[idx], channel);
                }
            }
        }
    }
    finalize_groups(&mut groups);
    groups
}

fn json_channel(obj: &Map<String, Value>) -> Option<LiveChannel> {
    let name = json_string(obj, "name");
    if name.is_empty() {
        return None;
    }
    let mut channel = LiveChannel {
        name,
        ..Default::default()
    };
    channel.logo = json_string(obj, "logo");
    channel.epg = json_string(obj, "epg");
    if channel.epg.is_empty() {
        channel.epg = json_string(obj, "tvg-url");
    }
    if channel.epg.is_empty() {
        channel.epg = json_string(obj, "tvg-id");
    }
    channel.ua = json_string(obj, "ua");
    channel.click = json_string(obj, "click");
    channel.format = json_string(obj, "format");
    channel.origin = json_string(obj, "origin");
    channel.referer = json_string(obj, "referer");
    channel.parse = json_i32(obj, "parse");
    if let Some(Value::Object(header)) = obj.get("header") {
        channel.header = json_map_strings(header);
    }

    if let Some(catchup) = obj.get("catchup") {
        match catchup {
            Value::Object(_) => channel.catchup = Some(catchup.clone()),
            Value::String(kind) => {
                channel.catchup = Some(catchup_json(
                    kind,
                    &json_string(obj, "catchup-source"),
                    &json_string(obj, "catchup-replace"),
                ));
            }
            _ => {}
        }
    }

    let mut urls = Vec::new();
    match obj.get("urls") {
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(url) = item.as_str() {
                    push_live_url(&mut urls, url);
                }
            }
        }
        Some(Value::String(url)) => push_live_url(&mut urls, url),
        _ => {}
    }
    // 容错：部分自定义订阅直接写单个 url（参考实现只认 urls[]，这里多兼容一层）
    if urls.is_empty() {
        if let Some(url) = obj.get("url").and_then(Value::as_str) {
            push_live_url(&mut urls, url);
        }
    }
    if urls.is_empty() {
        return None;
    }
    channel.urls = urls;
    Some(channel)
}

/* ============================================================
 * m3u
 * ============================================================ */

fn parse_m3u(text: &str) -> Vec<LiveGroup> {
    let mut groups: Vec<LiveGroup> = Vec::new();
    let mut current_group: Option<usize> = None;
    let mut pending_meta = ChannelMeta::default();
    let mut pending_channel: Option<LiveChannel> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        // #EXTM3U 头部属性：x-tvg-url 等是“组级 EPG”，由 extract_m3u_epg 单独取，
        // 不并入频道元数据（避免把 EPG 地址塞进频道标识字段）。
        if line.starts_with("#EXTM3U") {
            continue;
        }
        // ua=/parse=/click=/header=/format=/... 与 #EXTHTTP:/#EXTVLCOPT:/#KODIPROP:
        if is_setting(line) {
            pending_meta.merge_from(meta_from_setting(line));
            continue;
        }
        if line.starts_with("#EXTINF") || line.contains("#EXTINF") {
            let group_name = attr(line, "group-title").unwrap_or_default();
            let idx = find_or_create_group(&mut groups, &group_name);
            current_group = Some(idx);

            let mut channel = LiveChannel {
                name: channel_name_from_extinf(line),
                ..Default::default()
            };
            apply_meta(&mut channel, meta_from_extinf_line(line));
            apply_meta(&mut channel, pending_meta.take());
            pending_channel = Some(channel);
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        // 地址行：可带 `|header&...`
        let idx = match current_group {
            Some(idx) => idx,
            None => {
                let idx = find_or_create_group(&mut groups, DEFAULT_GROUP_NAME);
                current_group = Some(idx);
                idx
            }
        };
        let (url_part, header_part) = match line.split_once('|') {
            Some((url, header)) => (url.trim(), Some(header)),
            None => (line, None),
        };
        let mut parsed = Vec::new();
        push_live_url(&mut parsed, url_part);
        if parsed.is_empty() {
            continue;
        }
        if let Some(header) = header_part {
            let headers = header_pairs(header);
            if !headers.is_empty() {
                pending_meta.header.extend(headers);
            }
        }

        // 参考实现里 pendingChannel 在地址行之后不清空，同一个 #EXTINF 下的多个地址
        // 会继续追加到同一频道；下一个 #EXTINF 才替换。
        let mut channel = pending_channel.take().unwrap_or_else(|| LiveChannel {
            name: "未命名频道".to_string(),
            ..Default::default()
        });
        channel.urls.extend(parsed);
        apply_meta(&mut channel, pending_meta.take());
        add_or_merge_channel(&mut groups[idx], channel.clone());
        pending_channel = Some(channel);
    }

    finalize_groups(&mut groups);
    groups
}

fn channel_name_from_extinf(line: &str) -> String {
    // 参考 NAME_PATTERN `.*,(.+?)$`：取最后一个逗号之后的内容
    if let Some((_, name)) = line.rsplit_once(',') {
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Some(name) = attr(line, "tvg-name") {
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    "未命名频道".to_string()
}

/// 解析 `#EXTINF` 行上的 tvg/logo/ua/catchup 属性
fn meta_from_extinf_line(line: &str) -> ChannelMeta {
    let mut meta = ChannelMeta::default();
    if let Some(v) = attr(line, "tvg-logo") {
        meta.logo = v.trim().to_string();
    }
    if let Some(v) = attr(line, "tvg-url") {
        meta.epg = v.trim().to_string();
    }
    if meta.epg.is_empty() {
        if let Some(v) = attr(line, "tvg-id") {
            meta.epg = v.trim().to_string();
        }
    }
    if let Some(v) = attr(line, "http-user-agent") {
        meta.ua = v.trim().to_string();
    }
    let catchup = attr(line, "catchup").unwrap_or_default();
    let source = attr(line, "catchup-source").unwrap_or_default();
    let replace = attr(line, "catchup-replace").unwrap_or_default();
    if !catchup.is_empty() || !source.is_empty() || !replace.is_empty() {
        meta.catchup = Some(catchup_json(&catchup, &source, &replace));
    }
    meta
}

/* ============================================================
 * txt
 * ============================================================ */

fn parse_txt(text: &str) -> Vec<LiveGroup> {
    let mut groups: Vec<LiveGroup> = Vec::new();
    let mut current: Option<usize> = None;
    let mut pending_meta = ChannelMeta::default();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        // 参考实现：txt 里只认 `#` 开头的设置行（#EXTHTTP/#EXTVLCOPT/#KODIPROP）
        if line.starts_with('#') {
            if is_setting(line) {
                pending_meta.merge_from(meta_from_setting(line));
            }
            continue;
        }

        if line.contains("#genre#") {
            let group_name = line.split(',').next().unwrap_or("");
            let idx = find_or_create_group(&mut groups, group_name);
            current = Some(idx);
            // 切组时清空待合并设置（对齐 TxtSubscribe.parseTxtToJsonArray）
            pending_meta = ChannelMeta::default();
            continue;
        }

        let Some((name, url_part)) = line.split_once(',') else {
            continue;
        };
        let idx = match current {
            Some(idx) => idx,
            None => {
                let idx = find_or_create_group(&mut groups, DEFAULT_GROUP_NAME);
                current = Some(idx);
                idx
            }
        };

        let mut urls = Vec::new();
        for part in url_part.trim().split('#') {
            push_live_url(&mut urls, part);
        }
        if urls.is_empty() {
            continue;
        }

        let mut channel = LiveChannel {
            name: name.trim().to_string(),
            urls,
            ..Default::default()
        };
        apply_meta(&mut channel, pending_meta.take());
        add_or_merge_channel(&mut groups[idx], channel);
    }

    finalize_groups(&mut groups);
    groups
}

/* ============================================================
 * 设置行 / 属性解析
 * ============================================================ */

/// 待合并到下一个频道的元数据（对应 TxtSubscribe 的 pendingMeta）
#[derive(Debug, Clone, Default)]
struct ChannelMeta {
    logo: String,
    epg: String,
    ua: String,
    header: HashMap<String, String>,
    format: String,
    referer: String,
    origin: String,
    click: String,
    parse: i32,
    catchup: Option<Value>,
}

impl ChannelMeta {
    /// 后出现的非空设置覆盖先前的值（对齐 mergeMeta 的 dst.add(...) 覆盖语义）。
    fn merge_from(&mut self, other: ChannelMeta) {
        if !other.logo.is_empty() {
            self.logo = other.logo;
        }
        if !other.epg.is_empty() {
            self.epg = other.epg;
        }
        if !other.ua.is_empty() {
            self.ua = other.ua;
        }
        if !other.header.is_empty() {
            self.header.extend(other.header);
        }
        if !other.format.is_empty() {
            self.format = other.format;
        }
        if !other.referer.is_empty() {
            self.referer = other.referer;
        }
        if !other.origin.is_empty() {
            self.origin = other.origin;
        }
        if !other.click.is_empty() {
            self.click = other.click;
        }
        if other.parse != 0 {
            self.parse = other.parse;
        }
        if other.catchup.is_some() {
            self.catchup = other.catchup;
        }
    }

    fn take(&mut self) -> ChannelMeta {
        std::mem::take(self)
    }
}

fn apply_meta(channel: &mut LiveChannel, meta: ChannelMeta) {
    if !meta.logo.is_empty() {
        channel.logo = meta.logo;
    }
    if !meta.epg.is_empty() {
        channel.epg = meta.epg;
    }
    if !meta.ua.is_empty() {
        channel.ua = meta.ua;
    }
    if !meta.header.is_empty() {
        channel.header.extend(meta.header);
    }
    if !meta.format.is_empty() {
        channel.format = meta.format;
    }
    if !meta.referer.is_empty() {
        channel.referer = meta.referer;
    }
    if !meta.origin.is_empty() {
        channel.origin = meta.origin;
    }
    if !meta.click.is_empty() {
        channel.click = meta.click;
    }
    if meta.parse != 0 {
        channel.parse = meta.parse;
    }
    if meta.catchup.is_some() {
        channel.catchup = meta.catchup;
    }
}

/// 参考 TxtSubscribe.isSetting
fn is_setting(line: &str) -> bool {
    [
        "ua",
        "parse",
        "click",
        "player",
        "header",
        "format",
        "origin",
        "referer",
        "#EXTHTTP:",
        "#EXTVLCOPT:",
        "#KODIPROP:",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

/// 参考 TxtSubscribe.buildSetting
fn meta_from_setting(line: &str) -> ChannelMeta {
    let mut meta = ChannelMeta::default();

    if line.starts_with("ua") {
        if let Some(v) = setting_value(line, "ua") {
            meta.ua = v;
        }
    }
    if line.starts_with("parse") {
        if let Some(v) = setting_value(line, "parse") {
            meta.parse = v.parse::<i32>().unwrap_or(0);
        }
    }
    if line.starts_with("click") {
        if let Some(v) = setting_value(line, "click") {
            meta.click = v;
        }
    }
    if line.starts_with("header") {
        // 参考实现会对整个 JSON 去引号导致解析失败，这里保留原文按 JSON 解析
        if let Some(v) = setting_value_raw(line, "header") {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(v) {
                meta.header = json_map_strings(&map);
            }
        }
    }
    if line.starts_with("format") {
        if let Some(v) = setting_value(line, "format") {
            meta.format = v;
        }
    }
    if line.starts_with("origin") {
        if let Some(v) = setting_value(line, "origin") {
            meta.origin = v;
        }
    }
    if line.starts_with("referer") {
        if let Some(v) = setting_value(line, "referer") {
            meta.referer = v;
        }
    }
    if line.starts_with("#EXTHTTP:") {
        let value = line.trim_start_matches("#EXTHTTP:").trim();
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(value) {
            meta.header = json_map_strings(&map);
        }
    }
    if line.starts_with("#EXTVLCOPT:") {
        if line.contains("http-user-agent") {
            if let Some(v) = setting_value(line, "http-user-agent") {
                meta.ua = v;
            }
        }
        if line.contains("http-origin") {
            if let Some(v) = setting_value(line, "http-origin") {
                meta.origin = v;
            }
        }
        if line.contains("http-referrer") {
            if let Some(v) = setting_value(line, "http-referrer") {
                meta.referer = v;
            }
        }
    }
    if line.starts_with("#KODIPROP:") && line.contains("manifest_type=") {
        if let Some(v) = setting_value(line, "manifest_type") {
            meta.format = v;
        }
    }

    meta
}

/// `key=value` 的 value（去掉两侧引号，对齐 TxtSubscribe.getValue）
fn setting_value(line: &str, key: &str) -> Option<String> {
    let raw = setting_value_raw(line, key)?;
    Some(raw.trim_matches('"').trim().to_string())
}

/// `key=` 之后的原始内容（不处理引号，JSON 类值需要）
fn setting_value_raw<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    let idx = line.find(&needle)?;
    Some(line[idx + needle.len()..].trim())
}

/// 从一行 m3u/header 里取 `key="value"` 属性
fn attr(text: &str, key: &str) -> Option<String> {
    let mut from = 0usize;
    while from < text.len() {
        let rel = text[from..].find(key)?;
        let abs = from + rel;
        let before_ok = text[..abs]
            .chars()
            .next_back()
            .map(|c| c.is_whitespace() || c == ',')
            .unwrap_or(true);
        if before_ok {
            let rest = &text[abs + key.len()..];
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('"') {
                    if let Some(end) = rest.find('"') {
                        return Some(rest[..end].to_string());
                    }
                }
            }
        }
        from = abs + key.len();
    }
    None
}

/// `url|Referer=...&User-Agent=...` 的 `&` 参数解析（参考 parseHeaderString）
fn header_pairs(text: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for param in text.split('&') {
        let Some((key, value)) = param.split_once('=') else {
            continue;
        };
        let key = key.trim().replace('"', "");
        let value = value.trim().replace('"', "");
        if !key.is_empty() {
            headers.insert(key, value);
        }
    }
    headers
}

/* ============================================================
 * 公共小工具
 * ============================================================ */

/// 取组（按原始组名合并），返回下标
fn find_or_create_group(groups: &mut Vec<LiveGroup>, raw_name: &str) -> usize {
    let raw_name = normalize_group_name(raw_name);
    if let Some(idx) = groups.iter().position(|g| g.key == raw_name) {
        return idx;
    }
    let (name, password) = split_group_password(&raw_name);
    groups.push(LiveGroup {
        key: raw_name,
        name,
        password,
        epg: String::new(),
        channels: Vec::new(),
    });
    groups.len() - 1
}

/// `组名_密码` 拆分（参考 ApiConfig.loadLives 的 `split("_", 2)`）
fn split_group_password(raw: &str) -> (String, String) {
    match raw.split_once('_') {
        Some((name, password)) => {
            let name = name.trim();
            let password = password.trim();
            if name.is_empty() {
                (raw.trim().to_string(), password.to_string())
            } else {
                (name.to_string(), password.to_string())
            }
        }
        None => (raw.trim().to_string(), String::new()),
    }
}

fn normalize_group_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case(LEGACY_DEFAULT_GROUP_NAME) {
        DEFAULT_GROUP_NAME.to_string()
    } else {
        name.to_string()
    }
}

/// 组内同名频道合并 urls（按 url 去重、元数据只补空）
fn add_or_merge_channel(group: &mut LiveGroup, channel: LiveChannel) {
    if channel.name.is_empty() {
        group.channels.push(channel);
        return;
    }
    if let Some(existing) = group
        .channels
        .iter_mut()
        .find(|item| item.name == channel.name)
    {
        merge_channel(existing, channel);
    } else {
        group.channels.push(channel);
    }
}

fn merge_channel(dst: &mut LiveChannel, src: LiveChannel) {
    for url in src.urls {
        if !dst.urls.iter().any(|item| item.url == url.url) {
            dst.urls.push(url);
        }
    }
    if dst.logo.is_empty() {
        dst.logo = src.logo;
    }
    if dst.epg.is_empty() {
        dst.epg = src.epg;
    }
    if dst.ua.is_empty() {
        dst.ua = src.ua;
    }
    if dst.header.is_empty() {
        dst.header = src.header;
    }
    if dst.format.is_empty() {
        dst.format = src.format;
    }
    if dst.referer.is_empty() {
        dst.referer = src.referer;
    }
    if dst.origin.is_empty() {
        dst.origin = src.origin;
    }
    if dst.click.is_empty() {
        dst.click = src.click;
    }
    if dst.parse == 0 {
        dst.parse = src.parse;
    }
    if dst.catchup.is_none() {
        dst.catchup = src.catchup;
    }
}

/// 给每个频道补 group_key，并给未命名的源补“源N”（参考 ApiConfig.loadLives）
fn finalize_groups(groups: &mut [LiveGroup]) {
    for group in groups {
        let key = group.key.clone();
        for channel in &mut group.channels {
            channel.group_key = key.clone();
            for (index, url) in channel.urls.iter_mut().enumerate() {
                if url.name.trim().is_empty() {
                    url.name = format!("源{}", index + 1);
                }
            }
        }
    }
}

/// 把 `url$源名` 解析成 [`LiveUrl`]；无 scheme 或非法地址忽略，重复 url 去重。
fn push_live_url(urls: &mut Vec<LiveUrl>, raw: &str) {
    let raw = raw.trim();
    if raw.is_empty() {
        return;
    }
    let (url, name) = match raw.split_once('$') {
        Some((url, name)) => (url.trim(), name.trim()),
        None => (raw, ""),
    };
    if !is_stream_url(url) {
        return;
    }
    if urls.iter().any(|item| item.url == url) {
        return;
    }
    urls.push(LiveUrl {
        url: url.to_string(),
        name: name.to_string(),
    });
}

/// 参考 TxtSubscribe.isUrl：只接受 http/rtp/rtsp/rtmp 开头的地址
fn is_stream_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    !lower.is_empty()
        && (lower.starts_with("http")
            || lower.starts_with("rtp")
            || lower.starts_with("rtsp")
            || lower.starts_with("rtmp"))
}

fn catchup_json(kind: &str, source: &str, replace: &str) -> Value {
    let mut map = Map::new();
    if !kind.is_empty() {
        map.insert("type".to_string(), Value::String(kind.to_string()));
    }
    if !source.is_empty() {
        map.insert("source".to_string(), Value::String(source.to_string()));
    }
    if !replace.is_empty() {
        map.insert("replace".to_string(), Value::String(replace.to_string()));
    }
    Value::Object(map)
}

fn json_string(obj: &Map<String, Value>, key: &str) -> String {
    obj.get(key)
        .and_then(json_scalar_string)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn json_i32(obj: &Map<String, Value>, key: &str) -> i32 {
    match obj.get(key) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or_default() as i32,
        Some(Value::String(s)) => s.trim().parse::<i32>().unwrap_or_default(),
        Some(Value::Bool(b)) => i32::from(*b),
        _ => 0,
    }
}

fn json_map_strings(map: &Map<String, Value>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (key, value) in map {
        if let Some(s) = json_scalar_string(value) {
            out.insert(key.clone(), s);
        }
    }
    out
}

fn decode_base64_any(raw: &str) -> Option<Vec<u8>> {
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    for engine in [URL_SAFE, URL_SAFE_NO_PAD, STANDARD, STANDARD_NO_PAD] {
        if let Ok(bytes) = engine.decode(compact.as_bytes()) {
            return Some(bytes);
        }
    }
    None
}

fn strip_bom(text: &str) -> &str {
    text.trim_start_matches('\u{feff}')
}

/* ============================================================
 * 单元测试（随模块一起跑，集成测试在 tests/live_test.rs）
 * ============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_group_password_and_multi_source() {
        let text = "福利_1234,#genre#\n测试频道,http://a/1#http://b/2$备用\n";
        let groups = parse_txt_or_m3u(text, "inline").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "福利");
        assert_eq!(groups[0].password, "1234");
        assert_eq!(groups[0].key, "福利_1234");
        assert!(groups[0].is_locked());
        let channel = &groups[0].channels[0];
        assert_eq!(channel.urls.len(), 2);
        assert_eq!(channel.urls[0].name, "源1");
        assert_eq!(channel.urls[1].name, "备用");
        assert_eq!(channel.group_key, "福利_1234");
    }

    #[test]
    fn txt_legacy_default_group_name() {
        let groups = parse_txt_or_m3u("Ungrouped,#genre#\nA,http://a/1", "inline").unwrap();
        assert_eq!(groups[0].name, DEFAULT_GROUP_NAME);
    }

    #[test]
    fn m3u_attrs_and_settings() {
        let text = concat!(
            "#EXTM3U x-tvg-url=\"http://epg.example.com/all.xml\"\n",
            "#EXTVLCOPT:http-referrer=http://ref.example\n",
            "#EXTINF:-1 tvg-id=\"cctv1\" tvg-name=\"CCTV1\" tvg-logo=\"http://logo/1.png\" ",
            "group-title=\"央视\" http-user-agent=\"okhttp/3.15\",CCTV-1 综合\n",
            "http://a/cctv1.m3u8|Referer=http://h5&User-Agent=Mozilla\n",
            "#EXTINF:-1 group-title=\"央视\" catchup=\"append\" catchup-source=\"?playseek={utc}\" ",
            "catchup-replace=\"0\",CCTV-2 财经\n",
            "#KODIPROP:manifest_type=hls\n",
            "http://a/cctv2.m3u8\n",
            "#EXTINF:-1 group-title=\"央视\",CCTV-1 综合\n",
            "http://b/cctv1.m3u8\n",
        );
        assert_eq!(
            extract_m3u_epg(text).as_deref(),
            Some("http://epg.example.com/all.xml")
        );
        let groups = parse_txt_or_m3u(text, "inline").unwrap();
        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        assert_eq!(group.name, "央视");
        assert_eq!(group.channels.len(), 2);

        let cctv1 = &group.channels[0];
        assert_eq!(cctv1.name, "CCTV-1 综合");
        assert_eq!(cctv1.logo, "http://logo/1.png");
        assert_eq!(cctv1.ua, "okhttp/3.15");
        assert_eq!(cctv1.referer, "http://ref.example");
        assert_eq!(cctv1.epg, "cctv1");
        assert_eq!(
            cctv1.header.get("Referer").map(String::as_str),
            Some("http://h5")
        );
        assert_eq!(
            cctv1.header.get("User-Agent").map(String::as_str),
            Some("Mozilla")
        );
        assert_eq!(cctv1.urls.len(), 2);
        assert_eq!(cctv1.urls[0].url, "http://a/cctv1.m3u8");
        assert_eq!(cctv1.urls[1].url, "http://b/cctv1.m3u8");
        assert_eq!(cctv1.urls[1].name, "源2");

        let cctv2 = &group.channels[1];
        assert_eq!(cctv2.format, "hls");
        let catchup = cctv2.catchup.as_ref().unwrap();
        assert_eq!(catchup.get("type").and_then(Value::as_str), Some("append"));
        assert_eq!(
            catchup.get("source").and_then(Value::as_str),
            Some("?playseek={utc}")
        );
    }

    #[test]
    fn json_groups_merge_and_aliases() {
        let text = r#"[
            {"group": "央视", "channels": [
                {"name": "CCTV-1", "urls": ["http://a/1", "http://b/1$备用"], "logo": "l",
                 "ua": "ua1", "header": {"Referer": "r"}, "parse": "1",
                 "catchup": {"type": "default"}},
                {"name": "CCTV-1", "urls": ["http://c/1"]}
            ]},
            {"name": "卫视", "channel": [{"name": "湖南卫视", "url": "rtmp://h/1"}]}
        ]"#;
        let groups = parse_txt_or_m3u(text, "inline").unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].channels.len(), 1);
        let cctv1 = &groups[0].channels[0];
        assert_eq!(cctv1.urls.len(), 3);
        assert_eq!(cctv1.urls[1].name, "备用");
        assert_eq!(cctv1.urls[2].name, "源3");
        assert_eq!(cctv1.parse, 1);
        assert_eq!(cctv1.ua, "ua1");
        assert_eq!(cctv1.header.get("Referer").map(String::as_str), Some("r"));
        assert!(cctv1.catchup.is_some());
        assert_eq!(groups[1].channels[0].name, "湖南卫视");
    }

    #[test]
    fn proxy_ext_is_decoded() {
        let real = "https://example.com/live/list.txt?token=x";
        let padded = URL_SAFE.encode(real);
        let decoded =
            resolve_live_address(&format!("proxy://do=live&type=txt&ext={padded}")).unwrap();
        assert_eq!(decoded, real);

        let unpadded = URL_SAFE_NO_PAD.encode(real);
        let decoded = resolve_live_address(&format!(
            "http://127.0.0.1:9978/proxy?do=live&type=txt&ext={unpadded}"
        ))
        .unwrap();
        assert_eq!(decoded, real);

        // 百分号转义的 padding
        let escaped = padded.replace('=', "%3D");
        let decoded =
            resolve_live_address(&format!("proxy://do=live&type=txt&ext={escaped}")).unwrap();
        assert_eq!(decoded, real);

        // ext 直接是 http 地址时原样使用
        assert_eq!(
            resolve_live_address("proxy://do=live&type=txt&ext=http%3A%2F%2Fa%2Fb.txt").unwrap(),
            "http://a/b.txt"
        );
    }

    #[test]
    fn json_config_object_is_rejected_with_hint() {
        let err = parse_txt_or_m3u(
            r#"{"lives":[{"name":"L","type":0,"url":"http://a/x.txt"}]}"#,
            "x.json",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("lives"), "错误应提示从 lives 里取地址: {err}");
    }

    #[test]
    fn empty_text_is_empty_result() {
        assert!(parse_txt_or_m3u("  \n\t", "inline").unwrap().is_empty());
    }
}
