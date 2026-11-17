//! `csp_AppMao` 原生实现（fantaiying「appto」框架，5 站）。
//!
//! ## 来源（REA 反编译证据）
//!
//! * jar `69dadb9a-Yoursmile.jar` 的 `com.github.catvod.spider.AppMao`（jadx 2002 行、
//!   37 处 skip → dex2jar 2.4 + Vineflower 1.10.1 单类还原 **4373 行 / 0 失败**，
//!   产物 `_rea/tmp/appmao/vf-single/AppMao.java`；短数组 XOR 解码 142/142，
//!   产物 `_rea/tmp/appmao/AppMao.decoded.java`）。
//! * ext：`Base64 → RC4("fantaiying666") → 每字节 -1 → JSON`（见
//!   `crate::spider::catvod::appto`，已对 5 个真实 ext 验证 5/5）。
//! * 站点 `_rea/samples/inventory/sites.json`：5 站（萌米/星奇/繁星/南坊/萌米App）。
//!
//! ## 协议（逐段还原）
//!
//! * `init`：ext JSON → `key`(zH)/`iv`(i)/`domain`(SN)/`host`(yq)/`__ic`(t0)/
//!   `token`(Gc)/`configV2`(tF)/`isV4`(N)；`domain` 非空时按 `|` 拆分逐个 GET，
//!   取响应 `"domain"` 覆盖 `host`；仍为空则去掉 `/appto.json` 后缀作基址。
//! * `SN()` = `host` + `api_prefix`（`isV4` → `/addons/apptov4/app.php/v1/`，
//!   否则 `/appto/v1/`）。
//! * 配置：GET `SN() + config/get?p=android`（`isV4` 走 v4 前缀、`configV2` 走 v2），
//!   响应 `{"ENCRYPTION":0|1,"data":…}` —— 1 为明文 JSON，否则 AES-256-CBC
//!   （key/iv 取自 ext）。
//! * 分类：配置 `get_home_cate[]` 的 `{cate, title}` → `{type_id, type_name}`。
//! * 列表 `categoryContent`：GET `SN()+"vod/getLists"`，参数
//!   `area/lang/year/order/type_id/pageSize=21/page`；响应 `data[]` 的
//!   `vod_id/vod_name/vod_pic|vod_pic_thumb/vod_remarks`。
//! * 详情 `detailContent`：GET `SN()+"vod/getVod"`，参数 `id/__platform=android/__ic`；
//!   响应 `data` 的 `vod_*` 与 `vod_play_list[]`（`title` + `urls[{name,url}]`）。
//! * 搜索 `searchContent`：GET `SN()+"vod/getVodSearch"`，查询
//!   `wd=…&page=1&type`。
//! * 播放 `playerContent`：id 按 `@` 拆成 `play_url@key`；遍历配置
//!   `get_parsing.lists[].label`，GET `SN()+"parsing/proxy"`，参数
//!   `play_url/label/key`，响应取 `url` 后按 `parse=0` 直出。
//!
//! ## 有意偏差（桌面端能力边界）
//!
//! 1. 上游 5 个 ext 的 `domain`/`host` 在 2026-11 全部不可达
//!    （`lmapp.org`、`app.mervod.com:9527` fetch failed；cos 桶 404 NoSuchBucket），
//!    本家族只能以 mock 夹具验收，无真站 `#[ignore]` 用例。
//! 2. `by.Gc(url, body, params, headers, extra)` 的原实现属宿主
//!    `com.github.catvod.parser`（不在 spider jar 内），本实现按可观测调用形态还原为
//!    「`body == null` → GET + 查询参数」；若真实宿主对某些端点是 POST，需要以真站
//!    抓包再校正（当前上游已死，无法验证）。
//! 3. `configV1` 标记与默认分支等价（都走 `/appto/v1/`），按原类保留读取。
//! 4. 详情剧集 id 在原类中为 `url@<播放串键>`，本实现用 `url@vod_id`；
//!    播放时把 `key` 原样传给 `parsing/proxy`（与原类参数名一致）。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::client_for;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::spider::catvod::appto::{
    decode_ext, decrypt_config, ApptoExt, API_V4, CONFIG_V1, CONFIG_V2, EP_GET_LISTS,
    EP_GET_VOD, EP_PARSING, EP_SEARCH,
};
use crate::spider::catvod::contract::{CatvodError, ErrorClass};

/// 家族标识（`profiles.json` 的 api 名）。
pub const API: &str = "csp_AppMao";
/// 原类 `getHeaders()` 的 UA。
pub const UA: &str = "Dart/2.19 (dart:io)";
/// 分类页每页条数（原类 `pageSize="21"`）。
pub const PAGE_SIZE: u32 = 21;
/// 单线路最多保留的剧集数（与其它家族一致的保护上限）。
const MAX_EPISODES: usize = 500;
/// 配置端点 v4 前缀下的路径。
const CONFIG_V4: &str = "config/get?p=android";

/// 是否由本家族处理。
pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

fn parse_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::Parse, site, detail).into_core()
}

fn host_err(site: &Site, detail: impl Into<String>) -> CoreError {
    CatvodError::new(ErrorClass::HostApi, site, detail).into_core()
}

/// ext 解析：优先按 appto 编码解码，失败时接受裸 JSON（便于测试与手工配置）。
fn ext_of(site: &Site) -> Result<ApptoExt> {
    let raw = site.ext_string();
    if raw.trim().is_empty() {
        return Err(parse_err(site, "ext 为空：AppMao 需要 appto 配置串"));
    }
    if let Ok(ext) = decode_ext(&raw) {
        return Ok(ext);
    }
    let value: Value = serde_json::from_str(raw.trim())
        .map_err(|e| parse_err(site, format!("ext 既不是 appto 配置串也不是 JSON: {e}")))?;
    Ok(ext_from_value(&value))
}

fn ext_from_value(value: &Value) -> ApptoExt {
    let obj = value.as_object();
    let s = |k: &str| {
        obj.and_then(|o| o.get(k))
            .and_then(Value::as_str)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let flag = |k: &str| match obj.and_then(|o| o.get(k)) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) == 1,
        Some(Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    };
    ApptoExt {
        domain: s("domain"),
        host: s("host"),
        key: s("key"),
        iv: s("iv"),
        token: s("token"),
        ic: s("__ic"),
        is_v4: flag("isV4"),
        config_v1: flag("configV1"),
        config_v2: flag("configV2"),
    }
}

fn headers(ext: &ApptoExt) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    if let Some(token) = ext.token.as_deref().filter(|t| !t.is_empty()) {
        h.insert("token".to_string(), token.to_string());
    }
    h
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn with_query(url: &str, params: &[(&str, &str)]) -> String {
    if params.is_empty() {
        return url.to_string();
    }
    let mut out = url.to_string();
    out.push(if url.contains('?') { '&' } else { '?' });
    let mut first = true;
    for (k, v) in params {
        if !first {
            out.push('&');
        }
        first = false;
        out.push_str(k);
        out.push('=');
        out.push_str(&urlencode(v));
    }
    out
}

fn get_json(
    cfg: &TvConfig,
    site: &Site,
    url: &str,
    params: &[(&str, &str)],
    ext: &ApptoExt,
) -> Result<Value> {
    let client = client_for(&cfg.http)?;
    let full = with_query(url, params);
    let text = crate::http::get_text(&client, &full, &headers(ext), 2)?;
    serde_json::from_str(&text)
        .map_err(|e| parse_err(site, format!("响应不是 JSON（{full}）: {e}")))
}

/// 业务基址：`SN()` = host + 前缀；`isV4` 走 v4 路径。
fn api_base(site: &Site, ext: &ApptoExt) -> Result<String> {
    let host = ext
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .ok_or_else(|| parse_err(site, "AppMao ext 缺少 host"))?;
    Ok(format!("{}{}", host.trim_end_matches('/'), ext.api_prefix()))
}

/// 封面归一化基址：原类 `yq()` 用的是 ext.host，不含 API 前缀。
fn cover_base(ext: &ApptoExt, api_base: &str) -> String {
    if let Some(host) = ext.host.as_deref() {
        let host = host.trim().trim_end_matches('/');
        if !host.is_empty() {
            return host.to_string();
        }
    }
    let prefix = ext.api_prefix();
    api_base
        .strip_suffix(prefix)
        .unwrap_or(api_base)
        .trim_end_matches('/')
        .to_string()
}

/// 配置端点：`isV4` → v4 前缀，`configV2` → v2，否则 v1。
fn config_url(ext: &ApptoExt, base_host: &str) -> String {
    let path = if ext.is_v4 {
        format!("{API_V4}{CONFIG_V4}")
    } else if ext.config_v2 {
        CONFIG_V2.to_string()
    } else {
        CONFIG_V1.to_string()
    };
    format!("{}{}", base_host.trim_end_matches('/'), path)
}

/// 解析 `domain` 指向的配置地址（按 `|` 依次尝试），得到真实业务基址。
fn resolve_host(cfg: &TvConfig, site: &Site, ext: &ApptoExt) -> Result<String> {
    if let Some(h) = ext.host.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
        return Ok(h.trim_end_matches('/').to_string());
    }
    if let Some(domain) = ext.domain.as_deref() {
        for candidate in domain.split('|').map(str::trim).filter(|s| s.starts_with("http")) {
            if let Ok(value) = get_json(cfg, site, candidate, &[], ext) {
                if let Some(host) = value.get("domain").and_then(Value::as_str) {
                    if !host.trim().is_empty() {
                        return Ok(host.trim().trim_end_matches('/').to_string());
                    }
                }
            }
        }
        let trimmed = domain.trim().trim_end_matches('/');
        let stripped = trimmed.strip_suffix("/appto.json").unwrap_or(trimmed);
        if stripped.starts_with("http") {
            return Ok(stripped.to_string());
        }
    }
    Err(parse_err(site, "ext 缺少 host/domain，无法确定 AppMao 基址"))
}

/// 拉取并解密配置。
fn load_config(cfg: &TvConfig, site: &Site, ext: &ApptoExt) -> Result<Value> {
    let base = resolve_host(cfg, site, ext)?;
    let url = config_url(ext, &base);
    let client = client_for(&cfg.http)?;
    let text = crate::http::get_text(&client, &url, &headers(ext), 2)?;
    decrypt_config(ext, &text).map_err(|e| parse_err(site, format!("配置解密失败（{url}）: {e}")))
}

/// 数组或 `data` 字段（兼容 `{data:[…]}` / `{data:{list:[…]}}`）。
fn data_list(value: &Value) -> Vec<Value> {
    if let Some(arr) = value.as_array() {
        return arr.clone();
    }
    match value.get("data") {
        Some(Value::Array(arr)) => arr.clone(),
        Some(Value::Object(obj)) => obj
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn pick_str(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        match value.get(*key) {
            Some(Value::String(s)) if !s.trim().is_empty() => return s.trim().to_string(),
            Some(Value::Number(n)) => return n.to_string(),
            _ => {}
        }
    }
    String::new()
}

fn normalize_url(base: &str, url: &str) -> String {
    let url = url.trim();
    if url.is_empty() || url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if url.starts_with("//") {
        format!("https:{url}")
    } else if url.starts_with('/') {
        format!("{}{}", base.trim_end_matches('/'), url)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), url)
    }
}

fn item_from(value: &Value, base: &str) -> Option<VideoItem> {
    let id = pick_str(value, &["vod_id", "id"]);
    let name = pick_str(value, &["vod_name", "name"]);
    if id.is_empty() || name.is_empty() {
        return None;
    }
    Some(VideoItem {
        id,
        name,
        cover: {
            let cover = pick_str(value, &["vod_pic", "vod_pic_thumb", "pic"]);
            if cover.is_empty() {
                None
            } else {
                Some(normalize_url(base, &cover))
            }
        },
        remark: Some(pick_str(value, &["vod_remarks", "remarks", "note"])),
        ..Default::default()
    })
}

fn parse_items(value: &Value, base: &str) -> Vec<VideoItem> {
    data_list(value)
        .iter()
        .filter_map(|item| item_from(item, base))
        .collect()
}

fn categories_from_config(value: &Value) -> Vec<Category> {
    let mut out = Vec::new();
    let Some(arr) = value.get("get_home_cate").and_then(Value::as_array) else {
        return out;
    };
    for item in arr {
        let type_id = pick_str(item, &["cate", "type_id", "id"]);
        let type_name = pick_str(item, &["title", "type_name", "name"]);
        if type_id.is_empty() || type_name.is_empty() {
            continue;
        }
        if out.iter().any(|c: &Category| c.type_id == type_id) {
            continue;
        }
        out.push(Category { type_id, type_name });
    }
    out
}

fn strip_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ").replace("&amp;", "&").trim().to_string()
}

/// 分类：读取配置的 `get_home_cate`。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let ext = ext_of(site)?;
    let config = load_config(cfg, site, &ext)?;
    let cats = categories_from_config(&config);
    if cats.is_empty() {
        return Err(parse_err(site, "配置 get_home_cate 未解析出分类"));
    }
    Ok(cats)
}

/// 列表：`vod/getLists`。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let ext = ext_of(site)?;
    let base = api_base(site, &ext)?;
    let url = format!("{base}{EP_GET_LISTS}");
    let page_text = page.to_string();
    let size_text = PAGE_SIZE.to_string();
    let value = get_json(
        cfg,
        site,
        &url,
        &[
            ("area", ""),
            ("lang", ""),
            ("year", ""),
            ("order", "hits"),
            ("type_id", type_id),
            ("pageSize", &size_text),
            ("page", &page_text),
        ],
        &ext,
    )?;
    let items = parse_items(&value, &cover_base(&ext, &base));
    if items.is_empty() {
        return Err(parse_err(
            site,
            format!("分类「{type_id}」第 {page} 页未解析出条目：{url}"),
        ));
    }
    Ok(items)
}

/// 搜索：`vod/getVodSearch`。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let ext = ext_of(site)?;
    let base = api_base(site, &ext)?;
    let url = format!("{base}{EP_SEARCH}");
    let value = get_json(
        cfg,
        site,
        &url,
        &[("wd", keyword), ("page", "1"), ("type", "")],
        &ext,
    )?;
    Ok(parse_items(&value, &cover_base(&ext, &base)))
}

/// 详情：`vod/getVod`。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let ext = ext_of(site)?;
    let base = api_base(site, &ext)?;
    let url = format!("{base}{EP_GET_VOD}");
    let ic = ext.ic.clone().unwrap_or_default();
    let value = get_json(
        cfg,
        site,
        &url,
        &[("id", id), ("__platform", "android"), ("__ic", ic.as_str())],
        &ext,
    )?;
    let data = value.get("data").unwrap_or(&value);
    let name = pick_str(data, &["vod_name", "name"]);
    if name.is_empty() {
        return Err(parse_err(site, format!("详情响应缺少 vod_name：{url}")));
    }
    let vod_id = pick_str(data, &["vod_id", "id"]);
    let play = data
        .get("vod_play_list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut groups: Vec<EpisodeGroup> = Vec::new();
    for line in play {
        let mut group = pick_str(&line, &["title", "name"]);
        if group.is_empty() {
            group = line
                .get("player_info")
                .and_then(|v| v.get("show"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
        let urls = line
            .get("urls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut episodes = Vec::new();
        for raw in urls {
            let play_url = pick_str(&raw, &["url", "play_url"]);
            if play_url.is_empty() {
                continue;
            }
            let title = pick_str(&raw, &["name", "title"]);
            let title = if title.is_empty() {
                format!("第{}集", episodes.len() + 1)
            } else {
                title
            };
            episodes.push(Episode {
                title,
                url: format!("{play_url}@{vod_id}"),
            });
            if episodes.len() >= MAX_EPISODES {
                break;
            }
        }
        if episodes.is_empty() {
            continue;
        }
        if groups.iter().any(|g| g.group == group) {
            continue;
        }
        groups.push(EpisodeGroup {
            group: if group.is_empty() {
                "默认".to_string()
            } else {
                group
            },
            items: episodes,
        });
    }
    if groups.is_empty() {
        return Err(parse_err(site, "详情响应没有可播放线路（vod_play_list）"));
    }
    let content = pick_str(data, &["vod_content", "content", "desc"]);
    let mut desc = strip_html(&content);
    if desc.is_empty() {
        let year = pick_str(data, &["vod_year"]);
        let area = pick_str(data, &["vod_area"]);
        let remark = pick_str(data, &["vod_remarks"]);
        desc = [year, area, remark].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" / ");
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: {
            let cover = pick_str(data, &["vod_pic", "vod_pic_thumb", "pic"]);
            if cover.is_empty() {
                None
            } else {
                Some(normalize_url(&cover_base(&ext, &base), &cover))
            }
        },
        desc: Some(desc).filter(|d| !d.is_empty()),
        episodes: groups,
        ..Default::default()
    })
}

/// 配置里的解析器标签（`get_parsing.lists[].label`）。
fn parsing_labels(value: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(arr) = value
        .get("get_parsing")
        .and_then(|p| p.get("lists"))
        .and_then(Value::as_array)
    else {
        return out;
    };
    for item in arr {
        let label = pick_str(item, &["label", "name", "title"]);
        if label.is_empty() || out.iter().any(|l| *l == label) {
            continue;
        }
        out.push(label);
    }
    out
}

/// 播放：`play_url@key` → `parsing/proxy`。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    let Some((play_url, key)) = id.rsplit_once('@') else {
        return Err(parse_err(
            site,
            format!("剧集标识缺少 `@key`（线路「{flag}」）：{id}"),
        ));
    };
    let ext = ext_of(site)?;
    let base = api_base(site, &ext)?;
    let mut labels = Vec::new();
    if let Ok(config) = load_config(cfg, site, &ext) {
        labels = parsing_labels(&config);
    }
    if labels.is_empty() {
        // 原类只遍历配置里的解析器；没有解析器时把地址当作直链返回，避免死路。
        if play_url.starts_with("http") {
            return Ok(json!({
                "url": play_url,
                "parse": 0,
                "jx": 0,
                "playUrl": "",
                "header": { "User-Agent": UA },
            }));
        }
        return Err(host_err(site, "配置中没有解析器，且播放串不是 http 地址"));
    }
    let url = format!("{base}{EP_PARSING}");
    // 原类用线路的 `player_info.show` 选择解析器；命中则优先，否则按配置顺序尝试。
    if let Some(pos) = labels.iter().position(|l| l == flag) {
        let first = labels.remove(pos);
        labels.insert(0, first);
    }
    let mut last_error = None;
    for label in labels {
        let value = get_json(
            cfg,
            site,
            &url,
            &[("play_url", play_url), ("label", label.as_str()), ("key", key)],
            &ext,
        );
        match value {
            Ok(value) => {
                let resolved = pick_str(&value, &["url", "play_url"]);
                if !resolved.is_empty() {
                    return Ok(json!({
                        "url": resolved,
                        "parse": 0,
                        "jx": 0,
                        "playUrl": "",
                        "header": { "User-Agent": UA },
                    }));
                }
            }
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| parse_err(site, format!("parsing/proxy 未返回可用地址：{url}"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spider::catvod::appto::API_V1;

    const EXT_ENCODED_MENGMI_APP: &str = "FbjKZ1gO+9u81wMIxzHg+7x1Oep4CIFuZAlCy8a4qCgjGLacnxDo6wiBr66jP3X8J2mRx31u1XgkTTVrENVjfhQKUN2Yi12XNWQSlmVZxLtiSaSiZG0xf1Hw";

    fn site(ext: &str) -> Site {
        Site {
            key: "appmao-test".to_string(),
            name: "AppMao".to_string(),
            api: API.to_string(),
            kind: 3,
            ext: Some(json!(ext)),
            ..Default::default()
        }
    }

    fn raw_ext() -> String {
        json!({
            "host": "http://127.0.0.1:9000/",
            "key": "0123456789abcdef0123456789abcdef",
            "iv": "1238389483762837",
            "token": "tk",
            "__ic": "ic",
            "configV2": true,
            "isV4": false
        })
        .to_string()
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_AppMao"));
        assert!(handles("  CSP_APPMAO "));
        assert!(!handles("csp_AppSK"));
    }

    #[test]
    fn parses_raw_json_ext() {
        let ext = ext_of(&site(&raw_ext())).expect("raw ext");
        assert_eq!(ext.host.as_deref(), Some("http://127.0.0.1:9000/"));
        assert_eq!(ext.iv.as_deref(), Some("1238389483762837"));
        assert_eq!(ext.token.as_deref(), Some("tk"));
        assert_eq!(ext.ic.as_deref(), Some("ic"));
        assert!(ext.config_v2);
        assert!(!ext.is_v4);
    }

    #[test]
    fn decodes_encoded_ext() {
        let ext = ext_of(&site(EXT_ENCODED_MENGMI_APP)).expect("encoded ext");
        assert_eq!(ext.host.as_deref(), Some("http://app.mervod.com:9527"));
        assert!(ext.is_v4);
        assert!(ext.key.is_some());
    }

    #[test]
    fn api_base_uses_v4_prefix_when_is_v4() {
        let site = site("{}");
        let mut ext = ApptoExt::default();
        ext.host = Some("http://h:1/".to_string());
        assert_eq!(api_base(&site, &ext).unwrap(), format!("http://h:1{API_V1}"));
        ext.is_v4 = true;
        assert_eq!(api_base(&site, &ext).unwrap(), format!("http://h:1{API_V4}"));
    }

    #[test]
    fn config_url_selects_v4_v2_v1() {
        let mut ext = ApptoExt {
            host: Some("http://h:1".to_string()),
            ..Default::default()
        };
        assert_eq!(config_url(&ext, "http://h:1"), format!("http://h:1{CONFIG_V1}"));
        ext.config_v2 = true;
        assert_eq!(config_url(&ext, "http://h:1"), format!("http://h:1{CONFIG_V2}"));
        ext.is_v4 = true;
        assert_eq!(
            config_url(&ext, "http://h:1"),
            format!("http://h:1{API_V4}{CONFIG_V4}")
        );
    }

    #[test]
    fn categories_come_from_get_home_cate() {
        let config = json!({
            "get_home_cate": [
                { "mold": "1", "cate": "1", "title": "电影" },
                { "cate": 2, "title": "电视剧" },
                { "cate": "3" },
                { "cate": "1", "title": "重复" }
            ]
        });
        let cats = categories_from_config(&config);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].type_id, "1");
        assert_eq!(cats[0].type_name, "电影");
        assert_eq!(cats[1].type_id, "2");
    }

    #[test]
    fn parse_items_accepts_data_list_and_poster_fallback() {
        let value = json!({ "data": { "list": [
            { "vod_id": 7, "vod_name": "A", "vod_pic": "https://c/1.jpg", "vod_remarks": "更新至 1 集" },
            { "id": "8", "name": "B", "vod_pic_thumb": "https://c/2.jpg" },
            { "vod_name": "缺 id" }
        ]}});
        let items = parse_items(&value, "https://api.example.com");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "7");
        assert_eq!(items[0].cover.as_deref(), Some("https://c/1.jpg"));
        assert_eq!(items[0].remark.as_deref(), Some("更新至 1 集"));
        assert_eq!(items[1].id, "8");
        assert_eq!(items[1].cover.as_deref(), Some("https://c/2.jpg"));
    }

    #[test]
    fn parsing_labels_are_deduped() {
        let config = json!({
            "get_parsing": { "lists": [
                { "label": "解析一" },
                { "name": "解析二" },
                { "label": "解析一" },
                {}
            ]}
        });
        assert_eq!(parsing_labels(&config), vec!["解析一", "解析二"]);
    }

    #[test]
    fn normalize_url_handles_relative_forms() {
        let base = "https://api.example.com";
        assert_eq!(normalize_url(base, "https://cdn/x.jpg"), "https://cdn/x.jpg");
        assert_eq!(normalize_url(base, "//cdn/x.jpg"), "https://cdn/x.jpg");
        assert_eq!(normalize_url(base, "/img/x.jpg"), "https://api.example.com/img/x.jpg");
        assert_eq!(normalize_url(base, "img/x.jpg"), "https://api.example.com/img/x.jpg");
    }

    #[test]
    fn strip_html_removes_tags_and_entities() {
        assert_eq!(
            strip_html("<p>第一段&amp;第二段</p><br/>"),
            "第一段&第二段"
        );
    }

    #[test]
    fn player_requires_at_key_separator() {
        let cfg = TvConfig::default();
        let site = site(&raw_ext());
        let err = player_content(&cfg, &site, "默认", "no-separator").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("@key"), "{text}");
    }
}
