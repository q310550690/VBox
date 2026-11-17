//! 网盘搜索簇原生实现：`csp_PanSou` / `csp_UpYun` / `csp_Dovx` / `csp_YiSo`。
//!
//! 四个 api 共享同一形态：**只有搜索没有分类**，返回「网盘分享链接」列表；
//! 详情/播放依赖宿主侧的网盘解析（Ali/Push 家族），不属于本文件范围。
//! 语义来自静态反编译（`_rea/decompiled/<jar>/deobf/com/github/catvod/spider/*.java`）：
//!
//! * `PanSou`（`0f047745-FxzHm`，227 行）：3 个基址对应 3 个网盘标签
//!   （`www.alipansou.com`→阿里、`aipanso.com`→夸克、`xunjiso.com`→迅雷），
//!   逐个 `GET {host}/search?k={kw}&page={page}&s=0&t=-1`（UA Chrome122），
//!   Jsoup 取 `van-row:has(>a)` 行：首个 `template` 文本为标题，末个 `template`
//!   文本去掉 `时间: `/`格式: ` 后作备注（拼上标签）；`a[href]` 归一化作 id 去重。
//! * `UpYun`（46 行）：`GET {base}/search?keyword={kw}&page={page}&s_type=2`，
//!   响应 **Base64 → JSON** 的 `result.items[]`：`page_url`/`title`/`insert_time`。
//! * `Dovx`（23 行）：`GET {base}/ali/search?wd={kw}`，响应即 `Videos` JSON（`list[]`）。
//! * `YiSo`（49 行）：`GET {base}/api/search?name={kw}&pageNo=1&from=ali`，
//!   头 UA(Android)/Referer/Cookie `satoken`；`data.list[]` 的 `fileInfos[0].fileName`
//!   为标题、`gmtCreate` 为备注，`url` 是 AES-128-CBC/PKCS5 密文（Base64）。
//! * `Upyunso`（83 行）：`GET https://upapi.juapp9.com/search?keyword={kw}&page=1&s_type=1`
//!   （Origin/Referer `https://www.upyunso.com`），响应 **Base64 → JSON** 的
//!   `result.items[]`；只保留 `page_url` 命中 `(alipan|aliyundrive).com/s/` 且标题含关键词的行。
//! * `Yisou`（96 行）：与 `YiSo` 同端点的另一份部署（Cookie `satoken` 初值不同）。
//! * `AliPS`（93 行）：`GET {base}/search?k={kw}&t={7|1}`（`7`=文件夹、`1`=视频，两轮），
//!   Jsoup 行 `van-row a` → `template div` 文本必须命中 `(时间: \S+)`；
//!   `vod_name` = `[类型]` + 行文本、`remark` = `时间: …`。
//!
//! 站点可用 `ext` 覆盖基址（测试/自建镜像）；YiSo 的 ext 支持 `base$$$cookie`。

use std::collections::HashMap;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use aes::Aes128;
use base64::Engine as _;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use scraper::{Html, Selector};
use serde_json::Value;

use crate::error::Result;
use crate::model::{Category, Site, TvConfig, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const PANSOU: &str = "csp_PanSou";
pub const UPYUN: &str = "csp_UpYun";
pub const DOVX: &str = "csp_Dovx";
pub const YISO: &str = "csp_YiSo";
pub const FUNLETU: &str = "csp_Funletu";
pub const QUARKPANSO: &str = "csp_QuarkPanso";
pub const UPYUNSO: &str = "csp_Upyunso";
pub const YISOU: &str = "csp_Yisou";
pub const ALIPS: &str = "csp_AliPS";
pub const HUNHEPAN: &str = "csp_HunHePan";
pub const DAPANSO: &str = "csp_DaPanSo";
pub const MIAOSOU: &str = "csp_MiaoSou";
pub const EIGHTEIGHT: &str = "csp_EightEight";
pub const YUNPANONE: &str = "csp_YunPanOne";
pub const PIKASO: &str = "csp_PikaSo";
pub const QIANFAN: &str = "csp_Qianfan";
pub const YUNSO: &str = "csp_YunSo";

const UA_DESKTOP: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
const UA_ANDROID: &str = "Mozilla/5.0 (Linux; Android 12; V2049A Build/SP1A.210812.003; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/103.0.5060.129 Mobile Safari/537.36";
const YISO_COOKIE: &str = "satoken=2854cb58-3884-473b-84c4-34161f67a409";
const YISOU_COOKIE: &str = "satoken=b3bfc960-dc31-4cc4-bfa8-9bfa67f881c3";
const UPYUNSO_ORIGIN: &str = "https://www.upyunso.com";
const ALIPAN_URL_RE: &str = r"https://www\.(?:alipan|aliyundrive)\.com/s/";
const YISO_AES_KEY: &str = "4OToScUFOaeVTrHE";
const YISO_AES_IV: &str = "9CLGao1vHKqm17Oz";

const COVER_ALI: &str = "https://inews.gtimg.com/newsapp_bt/0/13263837859/1000";
const COVER_QUARK: &str = "https://bkimg.cdn.bcebos.com/pic/6d81800a19d8bc3e93bd4e218c8ba61ea9d345a7";
const COVER_THUNDER: &str = "https://bkimg.cdn.bcebos.com/pic/d4628535e5dde7114164dfafa8efce1b9d166131";
const COVER_PIKPAK: &str = "https://i0.hdslb.com/bfs/archive/5ae3b02b3c3246afda72ac84a497475dc29187c7.jpg";
const COVER_UPYUN: &str = "https://pic.imgdb.cn/item/65767399c458853aeff8a6a0.webp";
/// Funletu / QuarkPanso 原实现共用的固定封面（百度静态图）。
const COVER_BDSTATIC: &str = "https://b0.bdstatic.com/29cef38974f5f6b679dfeb8c6181ee4b.jpg";

pub fn handles(api: &str) -> bool {
    let api = api.trim();
    api.eq_ignore_ascii_case(PANSOU)
        || api.eq_ignore_ascii_case(UPYUN)
        || api.eq_ignore_ascii_case(DOVX)
        || api.eq_ignore_ascii_case(YISO)
        || api.eq_ignore_ascii_case(FUNLETU)
        || api.eq_ignore_ascii_case(QUARKPANSO)
        || api.eq_ignore_ascii_case(UPYUNSO)
        || api.eq_ignore_ascii_case(YISOU)
        || api.eq_ignore_ascii_case(ALIPS)
        || api.eq_ignore_ascii_case(HUNHEPAN)
        || api.eq_ignore_ascii_case(DAPANSO)
        || api.eq_ignore_ascii_case(MIAOSOU)
        || api.eq_ignore_ascii_case(EIGHTEIGHT)
        || api.eq_ignore_ascii_case(YUNPANONE)
        || api.eq_ignore_ascii_case(PIKASO)
        || api.eq_ignore_ascii_case(QIANFAN)
        || api.eq_ignore_ascii_case(YUNSO)
}

/// 网盘搜索站点没有分类。
pub fn categories() -> Vec<Category> {
    Vec::new()
}

/// 没有分类就没有分类列表。
pub fn videos(_cfg: &TvConfig, _site: &Site, _cat: &str, _page: u32) -> Result<Vec<VideoItem>> {
    Ok(Vec::new())
}

/// 详情依赖宿主网盘解析，本文件不实现。
pub fn detail(_cfg: &TvConfig, site: &Site, _id: &str) -> Result<crate::model::VideoDetail> {
    Err(not_supported(site, "网盘搜索只返回分享链接，没有 detailContent 契约"))
}

/// 播放同理：分享链接需要网盘解析器。
pub fn player_content(_cfg: &TvConfig, site: &Site, _flag: &str, _id: &str) -> Result<Value> {
    Err(not_supported(site, "网盘搜索返回的是分享页链接，不支持直接播放"))
}

fn not_supported(site: &Site, msg: &str) -> crate::error::CoreError {
    CatvodError::new(ErrorClass::HostApi, site, msg).into_core()
}

/// 按 api 分派搜索。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let api = site.api.trim();
    if api.eq_ignore_ascii_case(PANSOU) {
        search_pansou(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(UPYUN) {
        search_upyun(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(DOVX) {
        search_dovx(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(YISO) {
        search_yiso(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(FUNLETU) {
        search_funletu(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(QUARKPANSO) {
        search_quarkpanso(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(UPYUNSO) {
        search_upyunso(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(YISOU) {
        search_yisou(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(ALIPS) {
        search_alips(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(HUNHEPAN) {
        search_hunhepan(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(DAPANSO) {
        search_dapanso(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(MIAOSOU) {
        search_miaosou(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(EIGHTEIGHT) {
        search_eighteight(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(YUNPANONE) {
        search_yunpanone(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(PIKASO) {
        search_pikaso(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(QIANFAN) {
        search_qianfan(cfg, site, keyword)
    } else if api.eq_ignore_ascii_case(YUNSO) {
        search_yunso(cfg, site, keyword)
    } else {
        Ok(Vec::new())
    }
}

/* ============================================================
 * csp_PanSou
 * ============================================================ */

const PANSOU_HOSTS: [(&str, &str); 3] = [
    ("https://www.alipansou.com", "阿里"),
    ("https://aipanso.com", "夸克"),
    ("https://xunjiso.com", "迅雷"),
];

fn search_pansou(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let client = crate::http::client_for(&cfg.http)?;
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let hosts: Vec<(String, &str)> = match base_override(site) {
        Some(base) => vec![(base, "阿里")],
        None => PANSOU_HOSTS
            .iter()
            .map(|(host, label)| ((*host).to_string(), *label))
            .collect(),
    };

    let mut out: Vec<VideoItem> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (host, label) in hosts {
        let url = format!("{host}/search?k={kw}&page=1&s=0&t=-1");
        let mut extra = HashMap::new();
        extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
        let html = crate::http::get_text(&client, &url, &extra, cfg.http.retry)?;
        for item in parse_pansou(&html, &host, label) {
            if !item.name.contains(keyword) {
                continue;
            }
            if seen.insert(item.id.clone()) {
                out.push(item);
            }
        }
    }
    Ok(out)
}

/// 解析 PanSou 结果行（`van-row:has(>a)` 的近似：行内出现 `a` 且含 `template`）。
pub fn parse_pansou(html: &str, host: &str, label: &str) -> Vec<VideoItem> {
    // 真实页面是 Vue SSR 输出，标题/备注放在 `<template>` 里。按 HTML5 规范，
    // `<template>` 的子节点属于独立的 template contents，scraper（html5ever）看不到；
    // 而 Android 端的 jsoup 是宽松解析、能读到文本。这里把标签名改写成普通 div，
    // 保持取值语义等价。
    let patched = preprocess_templates(html);
    let doc = Html::parse_document(&patched);
    let row_sel = sel("van-row");
    let a_sel = sel("a");
    let tpl_sel = sel("[data-vbox-tpl]");
    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let Some(anchor) = row.select(&a_sel).next() else {
            continue;
        };
        let href = anchor
            .value()
            .attr("href")
            .unwrap_or_default()
            .trim()
            .to_string();
        if href.is_empty() {
            continue;
        }
        let tpls: Vec<_> = row.select(&tpl_sel).collect();
        let Some(first) = tpls.first() else {
            continue;
        };
        let name = normalize_ws(&text_of(first));
        if name.is_empty() {
            continue;
        }
        let secondary = tpls
            .last()
            .map(|el| text_of(el))
            .unwrap_or_default()
            .replace("时间: ", "")
            .replace("格式: ", "");
        let cleaned = normalize_ws(&secondary);
        let remark = format!("{label} {cleaned}").trim().to_string();
        out.push(VideoItem {
            id: join_url(host, &href),
            name,
            cover: Some(cover_for(label).to_string()),
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

fn cover_for(label: &str) -> &'static str {
    match label {
        "夸克" => COVER_QUARK,
        "迅雷" => COVER_THUNDER,
        "PikPak" => COVER_PIKPAK,
        _ => COVER_ALI,
    }
}

/* ============================================================
 * csp_UpYun
 * ============================================================ */

fn search_upyun(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, "https://upapi.juapp9.com");
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/search?keyword={kw}&page=1&s_type=2");
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    let body = crate::http::get_text(&client, &url, &extra, cfg.http.retry)?;
    let decoded = decode_base64_lenient(&body).ok_or_else(|| {
        CatvodError::new(ErrorClass::Parse, site, "csp_UpYun 响应不是合法 Base64").into_core()
    })?;
    let json: Value = serde_json::from_str(&decoded).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_UpYun 响应不是合法 JSON: {e}"),
        )
        .into_core()
    })?;
    Ok(parse_upyun(&json, keyword))
}

/// 解析 UpYun 的 `result.items[]`。
pub fn parse_upyun(json: &Value, keyword: &str) -> Vec<VideoItem> {
    let mut out = Vec::new();
    let items = json
        .get("result")
        .and_then(|v| v.get("items"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for item in items {
        let url = item
            .get("page_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if url.is_empty() || title.is_empty() || !title.contains(keyword) {
            continue;
        }
        let time = item
            .get("insert_time")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        out.push(VideoItem {
            id: url,
            name: title,
            cover: Some(COVER_UPYUN.to_string()),
            remark: if time.is_empty() { None } else { Some(time) },
        });
    }
    out
}

/* ============================================================
 * csp_Dovx
 * ============================================================ */

fn search_dovx(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, "https://api.dovx.tk");
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/ali/search?wd={kw}");
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    let body = crate::http::get_text(&client, &url, &extra, cfg.http.retry)?;
    let json: Value = serde_json::from_str(&body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Dovx 响应不是合法 JSON: {e}"),
        )
        .into_core()
    })?;
    Ok(parse_dovx(&json))
}

/// 解析 Dovx 的 `Videos` JSON（`list[]`，字段为 `vod_*`）。
pub fn parse_dovx(json: &Value) -> Vec<VideoItem> {
    let items = json
        .get("list")
        .or_else(|| json.get("data"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    items
        .iter()
        .filter_map(|item| {
            let id = pick_str(item, &["vod_id", "id", "url"])?;
            let name = pick_str(item, &["vod_name", "name", "title"])?;
            Some(VideoItem {
                id,
                name,
                cover: pick_str(item, &["vod_pic", "pic", "cover"]),
                remark: pick_str(item, &["vod_remarks", "remarks", "remark"]),
            })
        })
        .collect()
}

/* ============================================================
 * csp_YiSo
 * ============================================================ */

fn search_yiso(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let (base, cookie) = yiso_base(site);
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/api/search?name={kw}&pageNo=1&from=ali");
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_ANDROID.to_string());
    extra.insert("Referer".to_string(), format!("{base}/"));
    extra.insert("Cookie".to_string(), cookie);
    let body = crate::http::get_text(&client, &url, &extra, cfg.http.retry)?;
    let json: Value = serde_json::from_str(&body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_YiSo 响应不是合法 JSON: {e}"),
        )
        .into_core()
    })?;
    Ok(parse_yiso(&json))
}

/// 解析 YiSo 的 `data.list[]`（`url` 为 AES 密文）。
pub fn parse_yiso(json: &Value) -> Vec<VideoItem> {
    let mut out = Vec::new();
    let items = json
        .get("data")
        .and_then(|v| v.get("list"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for item in items {
        let enc = item.get("url").and_then(|v| v.as_str()).unwrap_or_default();
        let Some(link) = aes_cbc_decrypt_b64(enc) else {
            continue;
        };
        let name = item
            .get("fileInfos")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("fileName"))
            .and_then(|v| v.as_str())
            .or_else(|| item.get("fileName").and_then(|v| v.as_str()))
            .unwrap_or_default()
            .trim()
            .to_string();
        if link.is_empty() || name.is_empty() {
            continue;
        }
        let time = item
            .get("gmtCreate")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        out.push(VideoItem {
            id: link,
            name,
            cover: Some(COVER_ALI.to_string()),
            remark: if time.is_empty() { None } else { Some(time) },
        });
    }
    out
}

/// `ext` 形如 `base` 或 `base$$$cookie`（原实现允许第二段覆盖 Cookie）。
fn yiso_base(site: &Site) -> (String, String) {
    let ext = site.ext_string();
    let mut parts = ext.split("$$$");
    let base = parts.next().unwrap_or_default().trim().trim_end_matches('/');
    let base = if base.is_empty() {
        "https://yiso.fun".to_string()
    } else {
        base.to_string()
    };
    let cookie = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| YISO_COOKIE.to_string());
    (base, cookie)
}

/* ============================================================
 * csp_Upyunso
 * ============================================================ */

fn search_upyunso(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, "https://upapi.juapp9.com");
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/search?keyword={kw}&page=1&s_type=1");
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    extra.insert("Accept".to_string(), "*/*".to_string());
    extra.insert("Origin".to_string(), UPYUNSO_ORIGIN.to_string());
    extra.insert("Referer".to_string(), format!("{UPYUNSO_ORIGIN}/"));
    let body = crate::http::get_text(&client, &url, &extra, cfg.http.retry)?;
    let decoded = decode_base64_lenient(&body).ok_or_else(|| {
        CatvodError::new(ErrorClass::Parse, site, "csp_Upyunso 响应不是合法 Base64").into_core()
    })?;
    let json: Value = serde_json::from_str(&decoded).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Upyunso 响应不是合法 JSON: {e}"),
        )
        .into_core()
    })?;
    Ok(parse_upyunso(&json, keyword))
}

/// 解析 Upyunso 的 `result.items[]`（只保留阿里云盘分享页）。
pub fn parse_upyunso(json: &Value, keyword: &str) -> Vec<VideoItem> {
    let items = json
        .get("result")
        .and_then(|v| v.get("items"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let url = item
            .get("page_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        if url.is_empty() || title.is_empty() {
            continue;
        }
        if !title.contains(keyword) || !alipan_url_re().is_match(url) {
            continue;
        }
        out.push(VideoItem {
            id: url.to_string(),
            name: title.to_string(),
            cover: Some(COVER_ALI.to_string()),
            remark: None,
        });
    }
    out
}

/* ============================================================
 * csp_Yisou（与 csp_YiSo 同端点，Cookie 初值不同）
 * ============================================================ */

fn search_yisou(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let (base, cookie) = {
        let (base, cookie) = yiso_base(site);
        // Yisou 的默认 Cookie 与 YiSo 不同；只有站点显式给了第二段才覆盖。
        let cookie = if site.ext_string().contains("$$$") {
            cookie
        } else {
            YISOU_COOKIE.to_string()
        };
        (base, cookie)
    };
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let url = format!("{base}/api/search?name={kw}&pageNo=1&from=ali");
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_ANDROID.to_string());
    extra.insert("Referer".to_string(), format!("{base}/"));
    extra.insert("Cookie".to_string(), cookie);
    let body = crate::http::get_text(&client, &url, &extra, cfg.http.retry)?;
    let json: Value = serde_json::from_str(&body).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Yisou 响应不是合法 JSON: {e}"),
        )
        .into_core()
    })?;
    Ok(parse_yiso(&json))
}

/* ============================================================
 * csp_AliPS
 * ============================================================ */

/// `(t 参数, 展示标签)`：原实现固定两轮（`7`=文件夹、`1`=视频）。
const ALIPS_TYPES: [(&str, &str); 2] = [("7", "文件夹"), ("1", "视频")];

fn search_alips(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, "https://www.alipansou.com");
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    let mut out = Vec::new();
    for (t, label) in ALIPS_TYPES {
        let url = format!("{base}/search?k={kw}&t={t}");
        let html = crate::http::get_text(&client, &url, &extra, cfg.http.retry)?;
        out.extend(parse_alips(&html, keyword, label));
    }
    Ok(out)
}

/// 解析 AliPS 的 `van-row a` 行：`template div` 文本 + `时间: …` 备注。
pub fn parse_alips(html: &str, keyword: &str, label: &str) -> Vec<VideoItem> {
    let patched = preprocess_templates(html);
    let doc = Html::parse_document(&patched);
    let row_sel = sel("van-row a");
    let tpl_sel = sel("[data-vbox-tpl] div");
    let time_re = time_remark_re();
    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let href = row
            .value()
            .attr("href")
            .unwrap_or_default()
            .trim()
            .to_string();
        if href.is_empty() {
            continue;
        }
        let text = row
            .select(&tpl_sel)
            .next()
            .map(|el| normalize_ws(&text_of(&el)))
            .unwrap_or_else(|| normalize_ws(&text_of(&row)));
        let Some(mat) = time_re.find(&text) else {
            continue;
        };
        if !text.contains(keyword) {
            continue;
        }
        out.push(VideoItem {
            id: href,
            name: format!("[{label}]{text}"),
            cover: Some(COVER_ALI.to_string()),
            remark: Some(mat.as_str().to_string()),
        });
    }
    out
}

fn alipan_url_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(ALIPAN_URL_RE).expect("静态正则"))
}

fn time_remark_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"时间: \S+").expect("静态正则"))
}

/* ============================================================
 * 通用工具
 * ============================================================ */

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("静态选择器应当合法")
}

fn text_of(el: &scraper::ElementRef<'_>) -> String {
    el.text().collect::<String>()
}

fn normalize_ws(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pick_str(item: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        item.get(*key)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

/// 相对链接补全（`http(s)` 原样、`/` 拼 host、其余拼 `host/`）。
fn join_url(host: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    let host = host.trim_end_matches('/');
    if href.starts_with('/') {
        format!("{host}{href}")
    } else {
        format!("{host}/{href}")
    }
}

/// `ext` 覆盖基址（去尾部 `/`；非 http(s) 视作未提供）。
fn base_override(site: &Site) -> Option<String> {
    let ext = site.ext_string();
    let base = ext.trim().trim_end_matches('/');
    if base.starts_with("http://") || base.starts_with("https://") {
        Some(base.to_string())
    } else {
        None
    }
}

fn base_or(site: &Site, default: &str) -> String {
    base_override(site).unwrap_or_else(|| default.trim_end_matches('/').to_string())
}

/// 宽松 Base64 解码：去空白、自动补 `=`。
fn decode_base64_lenient(text: &str) -> Option<String> {
    let compact: String = text.split_whitespace().collect();
    if compact.is_empty() {
        return None;
    }
    let mut padded = compact;
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(padded)
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// YiSo 的 AES-128-CBC/PKCS5 解密（key/iv 各 16 字节，输入 Base64）。
pub fn aes_cbc_decrypt_b64(input: &str) -> Option<String> {
    let compact: String = input.split_whitespace().collect();
    if compact.is_empty() {
        return None;
    }
    let mut padded = compact;
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let raw = base64::engine::general_purpose::STANDARD.decode(padded).ok()?;
    let plain = cbc::Decryptor::<Aes128>::new_from_slices(
        YISO_AES_KEY.as_bytes(),
        YISO_AES_IV.as_bytes(),
    )
    .ok()?
    .decrypt_padded_vec_mut::<Pkcs7>(&raw)
    .ok()?;
    String::from_utf8(plain).ok()
}


/// 把 `<template>` 改写成 `<div data-vbox-tpl>`（见 `parse_pansou` 注释）。
fn preprocess_templates(html: &str) -> String {
    html.replace("<template", "<div data-vbox-tpl")
        .replace("</template>", "</div>")
}

/* ============================================================
 * csp_Funletu / csp_QuarkPanso（search-only，POST JSON）
 *
 * 两者都 `extends Quark`，但只覆写 `searchContent`：请求体是 JSON，
 * 结果是「网盘分享链接 + 标题 + 时间」，没有分类/详情/播放契约。
 * ============================================================ */

fn search_funletu(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, "https://v.funletu.com");
    let url = format!("{base}/search");
    let body = serde_json::json!({
        "style": "get",
        "datasrc": "search",
        "query": {
            "id": "",
            "datetime": "",
            "commonid": 1,
            "parmid": "",
            "fileid": "",
            "reportid": "",
            "validid": "",
            "searchtext": keyword,
        },
        "page": { "pageSize": 20, "pageIndex": 1 },
        "order": { "prop": "id", "order": "desc" },
        "message": "请求资源列表数据",
    })
    .to_string();
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    let text = crate::http::post_json_text(&client, &url, &extra, &body)?;
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_Funletu 响应不是合法 JSON: {e}"),
        )
        .into_core()
    })?;
    Ok(parse_funletu(&json))
}

/// 解析 Funletu 的 `data[]`：`url`（去掉 `?` 后的查询串）/`title`/`updatetime`。
pub fn parse_funletu(json: &Value) -> Vec<VideoItem> {
    let items = json
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let raw_url = item
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let url = raw_url.split('?').next().unwrap_or_default().to_string();
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if url.is_empty() || title.is_empty() {
            continue;
        }
        let time = item
            .get("updatetime")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        out.push(VideoItem {
            id: url,
            name: title,
            cover: Some(COVER_BDSTATIC.to_string()),
            remark: if time.is_empty() {
                None
            } else {
                Some(time.to_string())
            },
        });
    }
    out
}

fn search_quarkpanso(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, "https://qkpanso.com");
    let url = format!("{base}/v1/search/disk");
    let kw = utf8_percent_encode(keyword, NON_ALPHANUMERIC).to_string();
    let body = serde_json::json!({
        "page": "1",
        "size": 10,
        "q": keyword,
        "type": "QUARK",
        "share_time": "",
    })
    .to_string();
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    extra.insert("Referer".to_string(), format!("{base}/film/{kw}"));
    extra.insert("Origin".to_string(), base);
    let text = crate::http::post_json_text(&client, &url, &extra, &body)?;
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_QuarkPanso 响应不是合法 JSON: {e}"),
        )
        .into_core()
    })?;
    Ok(parse_quarkpanso(&json))
}

/// 解析 QuarkPanso 的 `data.list[]`：`link`/`disk_name`（去掉 `<em>` 标签）/`shared_time`。
pub fn parse_quarkpanso(json: &Value) -> Vec<VideoItem> {
    let items = json
        .get("data")
        .and_then(|v| v.get("list"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let link = item
            .get("link")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let name = item
            .get("disk_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .replace("<em>", "")
            .replace("</em>", "");
        let name = name.trim().to_string();
        if link.is_empty() || name.is_empty() {
            continue;
        }
        let time = item
            .get("shared_time")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        out.push(VideoItem {
            id: link,
            name,
            cover: Some(COVER_BDSTATIC.to_string()),
            remark: if time.is_empty() {
                None
            } else {
                Some(time.to_string())
            },
        });
    }
    out
}

/* ============================================================
 * csp_HunHePan / csp_DaPanSo
 *
 * 两者都是「网盘搜索」型 spider：`extends Push` 只是为了复用父类的网盘链接
 * 识别能力，站点自身只覆写 `searchContent`。桌面端按计划 §3.3 不实现网盘
 * 播放引擎，因此这里只移植搜索（detail/player 统一走 catvod.host.unsupported）。
 * ============================================================ */

const HUNHEPAN_BASE: &str = "https://api.hunhepan.com";
const DAPANSO_BASE: &str = "https://dapanso.com";
const DAPANSO_PATH: &str = "/api/resources/m-search";
/// 原实现签名前缀：`sign = sha256(path + "KDNL" + timestamp_ms)`。
const DAPANSO_SIGN_SALT: &str = "KDNL";
const DAPANSO_PAGE_SIZE: usize = 15;

/// 原实现 `Push.f` / `Quark.b` / `Thunder.b` / `YD.b` / `PikPak.b` 的合并判定：
/// 命中具体服务时返回对应标签；只命中 `Push.f` 的宽泛网盘域名时按原实现默认值算「阿里」。
fn pan_link_label(link: &str) -> Option<&'static str> {
    if link.contains("pan.quark.cn/s/") {
        return Some("夸克");
    }
    if link.contains("pan.xunlei.com/s/") {
        return Some("迅雷");
    }
    if link.contains("caiyun.139.com/") {
        return Some("YD");
    }
    if link.contains("mypikpak.com/s/") {
        return Some("PikPak");
    }
    if link.contains("aliyundrive.com/")
        || link.contains("alipan.com/")
        || link.contains("xunlei.com/")
        || link.contains("quark.cn/")
        || link.contains("139.com/")
        || link.contains("mypikpak.com/")
    {
        return Some("阿里");
    }
    None
}

/// 去 HTML 标签（原实现 `replaceAll("<.*?>", "")`）并压缩空白。
fn strip_tags(raw: &str) -> String {
    static TAGS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = TAGS.get_or_init(|| regex::Regex::new(r"<[^>]*>").expect("静态正则"));
    normalize_ws(&re.replace_all(raw, ""))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// SHA-256 十六进制小写（原实现 `merge/V.A.i`）。
/// 项目未依赖 sha2 crate，这里内联实现，避免为一个签名再引入新依赖。
pub fn sha256_hex(input: &str) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut data = input.as_bytes().to_vec();
    let bit_len = (data.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in data.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
}

fn search_hunhepan(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, HUNHEPAN_BASE);
    let client = crate::http::client_for(&cfg.http)?;
    let body = serde_json::json!({
        "is_mine": false,
        "q": keyword,
        "page": 1,
        "size": DAPANSO_PAGE_SIZE,
        "type": "",
    })
    .to_string();
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    extra.insert("Referer".to_string(), base.clone());
    extra.insert("Origin".to_string(), base.clone());
    extra.insert(
        "Accept".to_string(),
        "application/json, text/plain, */*".to_string(),
    );
    extra.insert(
        "Accept-Language".to_string(),
        "zh-CN,zh;q=0.9,en;q=0.8,ja;q=0.7,zh-TW;q=0.6".to_string(),
    );
    let url = format!("{base}/v1/search");
    let text = crate::http::post_json_text(&client, &url, &extra, &body)?;
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_HunHePan 响应不是合法 JSON: {e}"),
        )
        .into_core()
    })?;
    Ok(parse_hunhepan(&json))
}

/// 解析 HunHePan 的 `data.list[]`（原实现按链接识别网盘类型并给固定封面）。
pub fn parse_hunhepan(json: &Value) -> Vec<VideoItem> {
    let items = json
        .get("data")
        .and_then(|v| v.get("list"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in &items {
        let link = item
            .get("link")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if link.is_empty() {
            continue;
        }
        let Some(label) = pan_link_label(&link) else {
            continue;
        };
        let name = strip_tags(item.get("disk_name").and_then(|v| v.as_str()).unwrap_or_default());
        if name.is_empty() {
            continue;
        }
        let update_time = item
            .get("update_time")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let share_user = item
            .get("share_user")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let remark = normalize_ws(&format!("{label} {update_time} {share_user}"));
        out.push(VideoItem {
            id: link,
            name,
            cover: Some(cover_for(label).to_string()),
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

fn search_dapanso(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, DAPANSO_BASE);
    let client = crate::http::client_for(&cfg.http)?;
    let ts = now_ms();
    let sign = sha256_hex(&format!("{DAPANSO_PATH}{DAPANSO_SIGN_SALT}{ts}"));
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    extra.insert("Referer".to_string(), String::new());
    extra.insert("Origin".to_string(), base.clone());
    extra.insert(
        "Accept".to_string(),
        "application/json, text/plain, */*".to_string(),
    );
    extra.insert(
        "Accept-Language".to_string(),
        "zh-CN,zh;q=0.9,en;q=0.8,ja;q=0.7,zh-TW;q=0.6".to_string(),
    );
    extra.insert("Pragma".to_string(), "no-cache".to_string());
    extra.insert("Cache-Control".to_string(), "no-cache".to_string());
    extra.insert(
        "Sec-Ch-Ua".to_string(),
        "\"Not_A Brand\";v=\"8\", \"Chromium\";v=\"117\", \"Google Chrome\";v=\"117\"".to_string(),
    );
    extra.insert("Sec-Ch-Ua-Mobile".to_string(), "?0".to_string());
    extra.insert("Sec-Ch-Ua-Platform".to_string(), "\"Windows\"".to_string());
    extra.insert("Sec-Fetch-Dest".to_string(), "empty".to_string());
    extra.insert("Sec-Fetch-Mode".to_string(), "cors".to_string());
    extra.insert("Sec-Fetch-Site".to_string(), "same-origin".to_string());
    extra.insert("Timestamp".to_string(), ts.to_string());
    extra.insert("Sign".to_string(), sign);
    extra.insert("Token".to_string(), String::new());
    extra.insert("X-Requested-With".to_string(), "XMLHttpRequest".to_string());
    let mut form = HashMap::new();
    form.insert("name".to_string(), keyword.to_string());
    form.insert("current".to_string(), "1".to_string());
    form.insert("type".to_string(), "1".to_string());
    form.insert("size".to_string(), DAPANSO_PAGE_SIZE.to_string());
    let url = format!("{base}{DAPANSO_PATH}");
    let text = crate::http::post_form_text(&client, &url, &extra, &form)?;
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("csp_DaPanSo 响应不是合法 JSON: {e}"),
        )
        .into_core()
    })?;
    Ok(parse_dapanso(&json))
}

/// 解析 DaPanSo 的 `ret.records[]`：id = `{url}###{标题}`（原实现把两者拼在 id 里）。
pub fn parse_dapanso(json: &Value) -> Vec<VideoItem> {
    let items = json
        .get("ret")
        .and_then(|v| v.get("records"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in &items {
        let url = item
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if url.is_empty() {
            continue;
        }
        let Some(label) = pan_link_label(&url) else {
            continue;
        };
        let name = strip_tags(item.get("name").and_then(|v| v.as_str()).unwrap_or_default());
        if name.is_empty() {
            continue;
        }
        let created = item
            .get("createdAt")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let remark = normalize_ws(&format!("{label} {created}"));
        out.push(VideoItem {
            id: format!("{url}###{name}"),
            name,
            cover: Some(cover_for(label).to_string()),
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}


/* ============================================================
 * csp_MiaoSou / csp_EightEight / csp_YunPanOne（第 25 轮批量）
 *
 * 与 HunHePan/DaPanSo 同型：`extends Push` 但只覆写 `searchContent`，
 * 桌面端只做搜索；detail/player 走 catvod.host.unsupported。
 * ============================================================ */

fn enc(raw: &str) -> String {
    utf8_percent_encode(raw, NON_ALPHANUMERIC).to_string()
}

const MIAOSOU_BASE: &str = "https://miaosou.fun";
const EIGHTEIGHT_BASE: &str = "https://662688.xyz";
const YUNPANONE_BASE: &str = "https://yunpan1.cc";

fn pan_headers(base: &str) -> HashMap<String, String> {
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    extra.insert("Accept".to_string(), "application/json, text/plain, */*".to_string());
    extra.insert(
        "Accept-Language".to_string(),
        "zh-CN,zh;q=0.9,en;q=0.8,ja;q=0.7,zh-TW;q=0.6".to_string(),
    );
    extra.insert("Referer".to_string(), format!("{base}/"));
    extra
}

fn search_miaosou(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, MIAOSOU_BASE);
    let client = crate::http::client_for(&cfg.http)?;
    let mut extra = pan_headers(&base);
    extra.insert(
        "Referer".to_string(),
        format!("{base}/info?searchKey={}", enc(keyword)),
    );
    let url = format!("{base}/api/secendsearch?name={}&pageNo=1", enc(keyword));
    let text = crate::http::get_text(&client, &url, &extra, 1)?;
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(ErrorClass::Parse, site, format!("csp_MiaoSou 响应不是合法 JSON: {e}"))
            .into_core()
    })?;
    Ok(parse_miaosou(&json, keyword))
}

/// 解析 MiaoSou 的 `data.list[]`：`url` / `name` / `gmtCreate` / `creatorName`。
pub fn parse_miaosou(json: &Value, keyword: &str) -> Vec<VideoItem> {
    let items = json
        .get("data")
        .and_then(|v| v.get("list"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in &items {
        let link = item.get("url").and_then(|v| v.as_str()).unwrap_or_default().trim();
        if !link.starts_with("http") {
            continue;
        }
        let Some(label) = pan_link_label(link) else { continue };
        let name = strip_tags(item.get("name").and_then(|v| v.as_str()).unwrap_or_default());
        if name.is_empty() {
            continue;
        }
        let name = if name.contains(keyword) || keyword.contains(&name) { keyword.to_string() } else { name };
        let gmt = item.get("gmtCreate").and_then(|v| v.as_str()).unwrap_or_default();
        let creator = item.get("creatorName").and_then(|v| v.as_str()).unwrap_or_default();
        let remark = normalize_ws(&format!("{label} {gmt} {creator}"));
        out.push(VideoItem {
            id: link.to_string(),
            name,
            cover: Some(cover_for(label).to_string()),
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

fn search_eighteight(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, EIGHTEIGHT_BASE);
    let client = crate::http::client_for(&cfg.http)?;
    let extra = pan_headers(&base);
    let url = format!("{base}/api/get_zy?keyword={}", enc(keyword));
    let text = crate::http::get_text(&client, &url, &extra, 1)?;
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(ErrorClass::Parse, site, format!("csp_EightEight 响应不是合法 JSON: {e}"))
            .into_core()
    })?;
    Ok(parse_eighteight(&json))
}

/// 解析 EightEight 的 `data[]`：`title` / `url`，只取第一页（原实现 `page > 1` 直接返回空）。
pub fn parse_eighteight(json: &Value) -> Vec<VideoItem> {
    let items = json.get("data").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for item in &items {
        let link = item.get("url").and_then(|v| v.as_str()).unwrap_or_default().trim();
        if !link.starts_with("http") {
            continue;
        }
        let Some(label) = pan_link_label(link) else { continue };
        let name = strip_tags(item.get("title").and_then(|v| v.as_str()).unwrap_or_default());
        if name.is_empty() {
            continue;
        }
        out.push(VideoItem {
            id: link.to_string(),
            name,
            cover: Some(cover_for(label).to_string()),
            remark: Some(label.to_string()),
        });
    }
    out
}

fn search_yunpanone(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, YUNPANONE_BASE);
    let client = crate::http::client_for(&cfg.http)?;
    let extra = pan_headers(&base);
    let url = format!(
        "{base}/api/discussions?filter%5Bq%5D={}&page%5Blimit%5D=20&include=mostRelevantPost&page%5Boffset%5D=0",
        enc(keyword)
    );
    let text = crate::http::get_text(&client, &url, &extra, 1)?;
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(ErrorClass::Parse, site, format!("csp_YunPanOne 响应不是合法 JSON: {e}"))
            .into_core()
    })?;
    Ok(parse_yunpanone(&json, &base))
}

/// 解析 YunPanOne 的 `data[]`：条目在 `attributeT` 下（`slug` / `title` / `createdAt`），
/// id 为 `{base}/d/{slug}`，备注为 `createdAt`（原实现同样不识别网盘类型）。
pub fn parse_yunpanone(json: &Value, base: &str) -> Vec<VideoItem> {
    let items = json.get("data").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for item in &items {
        let attr = item.get("attributeT").unwrap_or(item);
        let slug = attr.get("slug").and_then(|v| v.as_str()).unwrap_or_default().trim();
        if slug.is_empty() {
            continue;
        }
        let name = strip_tags(attr.get("title").and_then(|v| v.as_str()).unwrap_or_default());
        if name.is_empty() {
            continue;
        }
        let created = attr.get("createdAt").and_then(|v| v.as_str()).unwrap_or_default();
        out.push(VideoItem {
            id: format!("{base}/d/{slug}"),
            name,
            cover: None,
            remark: if created.is_empty() { None } else { Some(created.to_string()) },
        });
    }
    out
}

/* ============================================================
 * csp_PikaSo（第 27 轮）
 *
 * `extends Push` 但只覆写 `searchContent`（489 行，`0f047745-FxzHm`）：
 *   `GET {base}/search/?pan=all&type=all&q={urlencode(kw)}&page={page}`
 *   （UA Chrome122、Referer/Origin=base、Accept/Accept-Language）
 *   Jsoup 行 `.search-item`：首个 `a` 的 `href` 为分享链接（须命中网盘链接），
 *   `a h2` 文本为标题；`.search-note` 文本去掉 `文件大小: `、`数量: ` 换成 `/`，
 *   含 `KB`/`MB` 的行跳过；标签按链接域名分 阿里/夸克/迅雷/YD/PikPak，
 *   备注 = `{标签} {剩余文本}`。
 *
 * 有意偏差：① 相对链接补 base；② 不实现 `client()` 的代理分支；
 * ③ 上游 `www.pikaso.top` 已不可用（mock-only）；④ 忽略原实现的 quick 标题替换。
 * ============================================================ */

const PIKASO_BASE: &str = "https://www.pikaso.top";

fn search_pikaso(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, PIKASO_BASE);
    let client = crate::http::client_for(&cfg.http)?;
    let extra = pan_headers(&base);
    let url = format!("{base}/search/?pan=all&type=all&q={}&page=1", enc(keyword));
    let html = crate::http::get_text(&client, &url, &extra, 1)?;
    Ok(parse_pikaso(&html, &base))
}

/// 解析 PikaSo 的 HTML 搜索结果（纯函数，便于单测与镜像）。
pub fn parse_pikaso(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = sel(".search-item");
    let anchor_sel = sel("a");
    let title_sel = sel("h2");
    let note_sel = sel(".search-note");
    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let Some(anchor) = row.select(&anchor_sel).next() else {
            continue;
        };
        let href = anchor
            .value()
            .attr("href")
            .unwrap_or_default()
            .trim()
            .to_string();
        if href.is_empty() {
            continue;
        }
        let url = crate::html_rule::join_url(base, &href);
        let Some(label) = pan_link_label(&url) else {
            continue;
        };
        let title = anchor
            .select(&title_sel)
            .next()
            .map(|el| normalize_ws(&text_of(&el)))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let note = row
            .select(&note_sel)
            .next()
            .map(|el| normalize_ws(&text_of(&el)))
            .unwrap_or_default();
        let cleaned = normalize_ws(
            &note
                .replace("文件大小: ", "")
                .replace("文件大小：", "")
                .replace("数量: ", "/")
                .replace("数量：", "/"),
        );
        if cleaned.contains("KB") || cleaned.contains("MB") {
            continue;
        }
        let remark = normalize_ws(&format!("{label} {cleaned}"));
        out.push(VideoItem {
            id: url,
            name: title,
            cover: Some(cover_for(label).to_string()),
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

/* ============================================================
 * csp_Qianfan / csp_YunSo（第 28 轮，HTML 型网盘搜索补齐）
 *
 * 两者都是 `extends Push` 但只覆写 `searchContent`（detail/player 走网盘引擎，
 * 桌面端统一 `catvod.host.unsupported`），本站只做搜索：
 *
 * * Qianfan（`0f047745-FxzHm`，261 行）：
 *     `GET {base}/search/?pan=all&type=all&q={enc}&page={page}`
 *     头 UA Chrome122 + Origin=base + Accept/Accept-Language + Sec-* + Cookie
 *     行 `.search-item`：首个 `a` 的 `href` 为分享链接（`html_rule::join_url` 归一），
 *     该 `a` 的文本为标题，`.search-note` 文本去 `文件大小: `、`数量: `→`/`；无封面。
 *
 * * YunSo（`0f047745-FxzHm`，246 行）：
 *     `{base}/api/validate/search?wd={kw}&uk=&mode=90002&stype=0&scope_content=&page={page}&limit=20`
 *     以该 URL 作为 **POST body** 发送（原实现 `S.e.s(client,url,body,headers)`），
 *     响应 JSON 的 `data` 字段是 HTML；行 `.layui-card`：
 *     id = `{card 内 v a 的 id 属性}#{标题}`（原实现 `next.n0("v a").d("id")` + `###`），
 *     标题 = `.layui-card-header a` 文本；备注 = `网盘 {header 文本去掉标题} {img alt}`；
 *     封面 = `.layui-card-body img` 的 `src`。
 *
 * 有意偏差：① Qianfan 的 referer 为空、Cookie 默认空（ext 第 4 段配置未实现）；
 * ② YunSo 的 `v a` 选择器在原站点已不可考，实现里退化为 `.layui-card-header a[href]`
 *    （取 href 作 id 前半段）——以保证 mock 与未来镜像可用；③ 两者上游均不可达，mock 验收。
 * ============================================================ */

const QIANFAN_BASE: &str = "https://pan.qianfan.app";
const YUNSO_BASE: &str = "https://www.yunso.net";

fn qianfan_headers(base: &str) -> HashMap<String, String> {
    let mut extra = pan_headers(base);
    extra.insert("Referer".to_string(), String::new());
    extra.insert("Pragma".to_string(), "no-cache".to_string());
    extra.insert("Cache-Control".to_string(), "no-cache".to_string());
    extra.insert(
        "Sec-Ch-Ua".to_string(),
        "\"Not_A Brand\";v=\"8\", \"Chromium\";v=\"117\", \"Google Chrome\";v=\"117\"".to_string(),
    );
    extra.insert("Sec-Ch-Ua-Mobile".to_string(), "?0".to_string());
    extra.insert("Sec-Ch-Ua-Platform".to_string(), "\"Windows\"".to_string());
    extra.insert("Sec-Fetch-Dest".to_string(), "empty".to_string());
    extra.insert("Sec-Fetch-Mode".to_string(), "cors".to_string());
    extra.insert("Sec-Fetch-Site".to_string(), "same-origin".to_string());
    extra.insert("Cookie".to_string(), String::new());
    extra
}

fn search_qianfan(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, QIANFAN_BASE);
    let client = crate::http::client_for(&cfg.http)?;
    let extra = qianfan_headers(&base);
    let url = format!("{base}/search/?pan=all&type=all&q={}&page=1", enc(keyword));
    let html = crate::http::get_text(&client, &url, &extra, 1)?;
    Ok(parse_qianfan(&html, &base))
}

/// 解析 Qianfan 搜索页（纯函数）。
pub fn parse_qianfan(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = sel(".search-item");
    let anchor_sel = sel("a");
    let note_sel = sel(".search-note");
    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let Some(anchor) = row.select(&anchor_sel).next() else {
            continue;
        };
        let href = anchor
            .value()
            .attr("href")
            .unwrap_or_default()
            .trim()
            .to_string();
        if href.is_empty() {
            continue;
        }
        let id = crate::html_rule::join_url(base, &href);
        let name = normalize_ws(&text_of(&anchor));
        if name.is_empty() {
            continue;
        }
        let note = row
            .select(&note_sel)
            .next()
            .map(|el| normalize_ws(&text_of(&el)))
            .unwrap_or_default();
        let cleaned = normalize_ws(
            &note
                .replace("文件大小: ", "")
                .replace("文件大小：", "")
                .replace("数量: ", "/")
                .replace("数量：", "/"),
        );
        out.push(VideoItem {
            id,
            name,
            cover: None,
            remark: if cleaned.is_empty() { None } else { Some(cleaned) },
        });
    }
    out
}

fn yunso_headers(base: &str) -> HashMap<String, String> {
    let mut extra = HashMap::new();
    extra.insert("User-Agent".to_string(), UA_DESKTOP.to_string());
    extra.insert("Referer".to_string(), format!("{base}/"));
    extra.insert("Origin".to_string(), base.to_string());
    extra.insert("Accept".to_string(), "application/json, text/plain, */*".to_string());
    extra
}

fn search_yunso(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_or(site, YUNSO_BASE);
    let client = crate::http::client_for(&cfg.http)?;
    let extra = yunso_headers(&base);
    let path = format!(
        "/api/validate/search?wd={}&uk=&mode=90002&stype=0&scope_content=&page=1&limit=20",
        enc(keyword)
    );
    let url = format!("{base}{path}");
    let text = crate::http::post_json_text(&client, &url, &extra, &path)?;
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        CatvodError::new(ErrorClass::Parse, site, format!("csp_YunSo 响应不是合法 JSON: {e}"))
            .into_core()
    })?;
    let html = json.get("data").and_then(|v| v.as_str()).unwrap_or_default();
    Ok(parse_yunso(html, &base))
}

/// 解析 YunSo 的 `data` HTML（纯函数）。
pub fn parse_yunso(html: &str, base: &str) -> Vec<VideoItem> {
    let doc = Html::parse_document(html);
    let row_sel = sel(".layui-card");
    let header_sel = sel(".layui-card-header");
    let link_sel = sel(".layui-card-header a");
    let img_sel = sel(".layui-card-body img");
    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let Some(header) = row.select(&header_sel).next() else {
            continue;
        };
        let name = header
            .select(&link_sel)
            .next()
            .map(|el| normalize_ws(&text_of(&el)))
            .unwrap_or_else(|| normalize_ws(&text_of(&header)));
        if name.is_empty() {
            continue;
        }
        let link_key = header
            .select(&link_sel)
            .next()
            .and_then(|el| el.value().attr("id").or_else(|| el.value().attr("href")))
            .unwrap_or_default()
            .trim()
            .to_string();
        let id = format!("{link_key}###{name}");
        let img = row.select(&img_sel).next();
        let cover = img
            .as_ref()
            .and_then(|el| el.value().attr("src"))
            .map(|src| crate::html_rule::join_url(base, src.trim()))
            .filter(|s| !s.is_empty());
        let alt = img
            .as_ref()
            .and_then(|el| el.value().attr("alt"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let header_text = normalize_ws(&text_of(&header)).replace(&name, "");
        let remark = normalize_ws(&format!("网盘 {} {}", header_text, alt));
        out.push(VideoItem {
            id,
            name,
            cover,
            remark: if remark.is_empty() { None } else { Some(remark) },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_six_pan_apis() {
        for api in [
            PANSOU,
            UPYUN,
            DOVX,
            YISO,
            FUNLETU,
            QUARKPANSO,
            "csp_pansou",
            "CSP_YISO",
        ] {
            assert!(handles(api), "{api}");
        }
        assert!(!handles("csp_Bili"));
        assert!(!handles("csp_Push"));
    }

    #[test]
    fn parse_pansou_rows() {
        let html = r#"
        <html><body>
          <van-row><a href="/s/abc123">
            <template>流浪地球3</template>
            <template>时间: 2026-11-01 格式: mp4</template>
          </a></van-row>
          <van-row><a href="https://x.test/s/def">
            <template>另一个资源</template>
            <template>时间: 2026-10-01 格式: mkv</template>
          </a></van-row>
        </body></html>"#;
        let items = parse_pansou(html, "https://www.alipansou.com", "阿里");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "流浪地球3");
        assert_eq!(items[0].id, "https://www.alipansou.com/s/abc123");
        assert_eq!(items[0].remark.as_deref(), Some("阿里 2026-11-01 mp4"));
        assert_eq!(items[1].id, "https://x.test/s/def");
    }

    #[test]
    fn parse_upyun_items() {
        let json: Value = serde_json::json!({
            "result": {"items": [
                {"page_url": "https://pan.test/s/1", "title": "示例 合集", "insert_time": "2026-11-01"},
                {"page_url": "https://pan.test/s/2", "title": "无关", "insert_time": "2026-11-02"}
            ]}
        });
        let items = parse_upyun(&json, "示例");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "https://pan.test/s/1");
        assert_eq!(items[0].cover.as_deref(), Some(COVER_UPYUN));
        assert_eq!(items[0].remark.as_deref(), Some("2026-11-01"));
    }

    #[test]
    fn parse_dovx_items() {
        let json: Value = serde_json::json!({
            "list": [
                {"vod_id": "https://pan.test/s/x", "vod_name": "某资源", "vod_pic": "https://img.test/a.jpg", "vod_remarks": "阿里"},
                {"vod_name": "缺 id", "vod_pic": null}
            ]
        });
        let items = parse_dovx(&json);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "某资源");
        assert_eq!(items[0].id, "https://pan.test/s/x");
    }

    #[test]
    fn aes_roundtrip_matches_yiso_vectors() {
        use aes::cipher::BlockEncryptMut;
        let plain = b"https://www.aliyundrive.com/s/xyz";
        let cipher = cbc::Encryptor::<Aes128>::new_from_slices(
            YISO_AES_KEY.as_bytes(),
            YISO_AES_IV.as_bytes(),
        )
        .expect("key/iv")
        .encrypt_padded_vec_mut::<Pkcs7>(plain);
        let b64 = base64::engine::general_purpose::STANDARD.encode(cipher);
        assert_eq!(
            aes_cbc_decrypt_b64(&b64).as_deref(),
            Some("https://www.aliyundrive.com/s/xyz")
        );
    }

    #[test]
    fn parse_funletu_rows() {
        let json: Value = serde_json::json!({
            "data": [
                {"url": "https://pan.test/s/1?pwd=abcd", "title": " 示例 合集 ", "updatetime": "2026-11-01"},
                {"url": "https://pan.test/s/2", "title": "另一个", "updatetime": ""},
                {"title": "缺链接", "updatetime": "2026-11-02"}
            ]
        });
        let items = parse_funletu(&json);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "https://pan.test/s/1");
        assert_eq!(items[0].name, "示例 合集");
        assert_eq!(items[0].remark.as_deref(), Some("2026-11-01"));
        assert_eq!(items[0].cover.as_deref(), Some(COVER_BDSTATIC));
        assert_eq!(items[1].remark, None);
    }

    #[test]
    fn parse_quarkpanso_rows() {
        let json: Value = serde_json::json!({
            "data": {"list": [
                {"link": "https://pan.quark.cn/s/q1", "disk_name": "<em>夸克</em>资源甲", "shared_time": "2026-11-05"},
                {"link": "", "disk_name": "缺链接", "shared_time": "2026-11-06"}
            ]}
        });
        let items = parse_quarkpanso(&json);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "夸克资源甲");
        assert_eq!(items[0].id, "https://pan.quark.cn/s/q1");
        assert_eq!(items[0].remark.as_deref(), Some("2026-11-05"));
    }

    #[test]
    fn handles_new_pan_apis() {
        assert!(handles("csp_Upyunso"));
        assert!(handles("csp_Yisou"));
        assert!(handles("csp_AliPS"));
        assert!(!handles("csp_Unknown"));
    }

    #[test]
    fn parse_upyunso_filters_alipan_links() {
        let json: Value = serde_json::json!({
            "result": {"items": [
                {"title": " 示例 合集 ", "page_url": "https://www.alipan.com/s/abc123"},
                {"title": "示例别的", "page_url": "https://pan.quark.cn/s/q1"},
                {"title": "无关", "page_url": "https://www.aliyundrive.com/s/zzz"}
            ]}
        });
        let items = parse_upyunso(&json, "示例");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "https://www.alipan.com/s/abc123");
        assert_eq!(items[0].name, "示例 合集");
        assert_eq!(items[0].cover.as_deref(), Some(COVER_ALI));
        assert_eq!(items[0].remark, None);
    }

    #[test]
    fn parse_alips_rows_use_template_and_time_remark() {
        let html = r#"<html><body>
          <van-row><a href="/s/ali111"><template><div>流浪地球3 4K 时间: 2026-11-01</div></template></a></van-row>
          <van-row><a href="/s/ali222"><template><div>没有时间</div></template></a></van-row>
          <van-row><a href=""><template><div>流浪地球3 时间: 2026-11-02</div></template></a></van-row>
        </body></html>"#;
        let items = parse_alips(html, "流浪地球3", "文件夹");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "/s/ali111");
        assert_eq!(items[0].name, "[文件夹]流浪地球3 4K 时间: 2026-11-01");
        assert_eq!(items[0].remark.as_deref(), Some("时间: 2026-11-01"));
    }
    #[test]
    fn handles_hunhepan_and_dapanso() {
        assert!(handles("csp_HunHePan"));
        assert!(handles("csp_DaPanSo"));
        assert!(handles("csp_hunhepan"));
    }

    #[test]
    fn pan_link_label_classifies_services() {
        assert_eq!(
            pan_link_label("https://www.aliyundrive.com/s/x"),
            Some("阿里")
        );
        assert_eq!(pan_link_label("https://pan.quark.cn/s/abc"), Some("夸克"));
        assert_eq!(
            pan_link_label("https://pan.xunlei.com/s/abc"),
            Some("迅雷")
        );
        assert_eq!(pan_link_label("https://caiyun.139.com/w/i/abc"), Some("YD"));
        assert_eq!(
            pan_link_label("https://mypikpak.com/s/abc"),
            Some("PikPak")
        );
        assert_eq!(pan_link_label("https://example.com/x"), None);
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn parse_hunhepan_classifies_and_notes() {
        let json: Value = serde_json::json!({"data":{"list":[
            {"link":"https://www.aliyundrive.com/s/a1","disk_name":"<em>流浪地球3</em> 4K","update_time":"2026-11-01","share_user":"张三"},
            {"link":"https://pan.quark.cn/s/q1","disk_name":"流浪地球2","update_time":"2026-10-01","share_user":""},
            {"link":"https://example.com/s/zzz","disk_name":"无关","update_time":"","share_user":""}
        ]}});
        let items = parse_hunhepan(&json);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "https://www.aliyundrive.com/s/a1");
        assert_eq!(items[0].name, "流浪地球3 4K");
        assert_eq!(items[0].remark.as_deref(), Some("阿里 2026-11-01 张三"));
        assert_eq!(items[0].cover.as_deref(), Some(COVER_ALI));
        assert_eq!(items[1].remark.as_deref(), Some("夸克 2026-10-01"));
        assert_eq!(items[1].cover.as_deref(), Some(COVER_QUARK));
    }

    #[test]
    fn parse_dapanso_composes_id_and_remark() {
        let json: Value = serde_json::json!({"ret":{"records":[
            {"url":"https://pan.quark.cn/s/q9","name":"<em>流浪地球3</em>","createdAt":"2026-11-01 12:00"},
            {"url":"https://example.com/x","name":"无关","createdAt":""}
        ]}});
        let items = parse_dapanso(&json);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "https://pan.quark.cn/s/q9###流浪地球3");
        assert_eq!(items[0].name, "流浪地球3");
        assert_eq!(items[0].remark.as_deref(), Some("夸克 2026-11-01 12:00"));
        assert_eq!(items[0].cover.as_deref(), Some(COVER_QUARK));
    }
}

#[cfg(test)]
mod batch25_tests {
    use super::*;

    #[test]
    fn handles_batch25_apis() {
        assert!(handles("csp_MiaoSou"));
        assert!(handles(" CSP_EIGHTEIGHT "));
        assert!(handles("csp_yunpanone"));
    }

    #[test]
    fn parses_miaosou_items() {
        let json = serde_json::json!({
            "data": { "list": [
                { "url": "https://pan.quark.cn/s/abc", "name": "<b>流浪地球3</b>", "gmtCreate": "2026-11-01", "creatorName": "alice" },
                { "url": "https://pan.xunlei.com/s/xyz", "name": "沙丘2", "gmtCreate": "2026-10-01", "creatorName": "bob" },
                { "url": "https://example.com/no", "name": "无效" }
            ]}
        });
        let items = parse_miaosou(&json, "流浪地球");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "https://pan.quark.cn/s/abc");
        assert_eq!(items[0].name, "流浪地球");
        assert_eq!(items[0].remark.as_deref(), Some("夸克 2026-11-01 alice"));
        assert_eq!(items[0].cover.as_deref(), Some(COVER_QUARK));
        assert_eq!(items[1].name, "沙丘2");
    }

    #[test]
    fn parses_eighteight_items() {
        let json = serde_json::json!({
            "data": [
                { "title": "流浪地球3", "url": "https://pan.quark.cn/s/abc" },
                { "title": "沙丘2", "url": "https://www.aliyundrive.com/s/xyz" },
                { "title": "无效", "url": "https://example.com/no" }
            ]
        });
        let items = parse_eighteight(&json);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].remark.as_deref(), Some("夸克"));
        assert_eq!(items[1].remark.as_deref(), Some("阿里"));
        assert_eq!(items[1].cover.as_deref(), Some(COVER_ALI));
    }

    #[test]
    fn parses_yunpanone_items() {
        let json = serde_json::json!({
            "data": [
                { "attributeT": { "slug": "abc", "title": "流浪地球3", "createdAt": "2026-11-01T10:00:00Z" } },
                { "attributeT": { "slug": "", "title": "无 slug" } },
                { "attributeT": { "slug": "def", "title": "" } }
            ]
        });
        let items = parse_yunpanone(&json, "https://yunpan1.cc");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "https://yunpan1.cc/d/abc");
        assert_eq!(items[0].remark.as_deref(), Some("2026-11-01T10:00:00Z"));
    }
}

#[cfg(test)]
mod pikaso_tests {
    use super::*;

    #[test]
    fn handles_pikaso_api() {
        assert!(handles("csp_PikaSo"));
        assert!(handles("csp_pikaso"));
    }

    #[test]
    fn parses_pikaso_rows_and_filters() {
        let html = r#"<html><body>
          <div class="search-item">
            <a href="https://pan.quark.cn/s/abc"><h2>流浪地球3 4K</h2></a>
            <div class="search-note">文件大小: 数量: 12.5 GB</div>
          </div>
          <div class="search-item">
            <a href="https://example.com/x"><h2>不是网盘</h2></a>
            <div class="search-note">数量: 1</div>
          </div>
          <div class="search-item">
            <a href="https://pan.quark.cn/s/qwe"><h2>被过滤</h2></a>
            <div class="search-note">文件大小: 900 KB</div>
          </div>
          <div class="search-item">
            <a href="/s/ali777"><h2>相对链接</h2></a>
            <div class="search-note">数量: 2</div>
          </div>
        </body></html>"#;
        let items = parse_pikaso(html, "https://www.pikaso.top");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "https://pan.quark.cn/s/abc");
        assert_eq!(items[0].name, "流浪地球3 4K");
        assert_eq!(items[0].cover.as_deref(), Some(COVER_QUARK));
        assert_eq!(items[0].remark.as_deref(), Some("夸克 /12.5 GB"));
        // 相对链接被 join_url 补齐（/s/ali777 不含网盘域名 → 被过滤）
        assert!(items.iter().all(|i| i.id.contains("quark.cn")));
    }
}

#[cfg(test)]
mod qianyunsou_tests {
    use super::*;

    #[test]
    fn handles_qianfan_and_yunso() {
        assert!(handles("csp_Qianfan"));
        assert!(handles("csp_QianFan"));
        assert!(handles("csp_YunSo"));
        assert!(handles("csp_yunso"));
    }

    #[test]
    fn parses_qianfan_rows() {
        let html = r#"<html><body>
          <div class="search-item">
            <a href="https://pan.quark.cn/s/qf1">流浪地球3 4K</a>
            <div class="search-note">文件大小: 数量: 12.5 GB</div>
          </div>
          <div class="search-item">
            <a href="">空链接</a>
          </div>
        </body></html>"#;
        let items = parse_qianfan(html, "https://pan.qianfan.app");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "https://pan.quark.cn/s/qf1");
        assert_eq!(items[0].name, "流浪地球3 4K");
        assert_eq!(items[0].remark.as_deref(), Some("/12.5 GB"));
    }

    #[test]
    fn parses_yunso_cards() {
        let html = r#"<html><body>
          <div class="layui-card">
            <div class="layui-card-header"><a id="share-abc" href="/d/abc">流浪地球3</a></div>
            <div class="layui-card-body"><img src="/img/1.jpg" alt="阿里"></div>
          </div>
          <div class="layui-card"></div>
        </body></html>"#;
        let items = parse_yunso(html, "https://www.yunso.net");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "share-abc###流浪地球3");
        assert_eq!(items[0].name, "流浪地球3");
        assert_eq!(
            items[0].cover.as_deref(),
            Some("https://www.yunso.net/img/1.jpg")
        );
        assert_eq!(items[0].remark.as_deref(), Some("网盘 阿里".trim()));
    }
}
