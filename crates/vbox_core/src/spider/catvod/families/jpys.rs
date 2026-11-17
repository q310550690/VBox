//! `csp_Jpys` 原生实现（签名 App API 垂直切片）。
//!
//! 语义来自静态反编译：`_rea/decompiled/5fc50e17-5199d8f8bcbe3c6d6938ded33cb90596/deobf/
//! com/github/catvod/spider/Jpys.java`（245 行，`detailContent` 被 jadx 跳过，已用
//! `--show-bad-code` 恢复 578 指令的伪代码后确认语义）。该类 `extends Spider`，
//! 协议完全自包含，可以 100% 原生重写：
//!
//! 1. `init(ctx, ext)`：ext 为逗号分隔的域名列表，原实现逐个 `HEAD` 校验后取第一个可用域；
//!    本移植按顺序取第一个 http(s) 域（不做 HEAD 探测，见「有意偏差」）。
//! 2. 请求签名：`sign = SHA1(MD5(paramString))`（`z.a` 即 MD5 hex，再取 SHA-1 hex），
//!    头 `sign` / `T`（毫秒时间戳）/ `Deviceid`。
//! 3. 分类固定 4 个：`1 电影`、`2 电视剧`、`4 动漫`、`3 综艺`（与原实现一致，顺序未调整）。
//! 4. 列表 `GET {base}/api/mw-movie/anonymous/video/list?type1=&pageNum=&area=&year=`，
//!    签名串 `area=&pageNum=&type1=&year=&key=&t=`（字段顺序与原实现逐字符一致）。
//! 5. 详情 `GET /api/mw-movie/anonymous/video/detail?id=`，`data` 给元数据与
//!    `episodeList[{name,nid}]`；播放线路单组 `在线播放`，分集 id 形如 `{vodId}@{nid}`
//!    （原实现用混淆字节 `'@'` 拼出，`playerContent` 再按 `@` 拆开）。
//! 6. 搜索 `GET /api/mw-movie/anonymous/video/searchByWord?keyword=&pageNum=1&pageSize=8`，
//!    结果在 `data.result.list`，跳过 `vodClass == "伦理"` 的条目。
//! 7. 播放 `GET /api/mw-movie/anonymous/v2/video/episode/url?id=&nid=`，取
//!    `data.list[0].url`，头带 UA/Origin/Referer（`parse: 0`）。
//!
//! 有意偏差（均在代码注释标注）：
//! - ext 域名列表不做 `HEAD` 探测，按顺序取第一个 http(s) 域（探测属于网络副作用）；
//! - `header` 输出为 JSON 对象而不是原实现的字符串化 JSON（VBox 模型约定对象）；
//! - `d` 静态缓存（按 `vodName` 去重）不实现——VBox 每页独立请求，去重会造成跨页漏项；
//! - `danmaku` 字段不输出（桌面端不实现弹幕协议，与计划 §3.3 一致）。

use std::collections::HashMap;

use serde_json::Value;

use crate::error::Result;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};

use super::super::contract::{CatvodError, ErrorClass};

pub const API: &str = "csp_Jpys";

/// 默认基址（原实现 `private String a = "https://www.hkybqufgh.com"`，该域已失效；
/// 站点当前可用域为 `www.ghw9zwp5.com`，与原实现 `playerContent` 里的 Origin 一致）。
const DEFAULT_BASE: &str = "https://www.ghw9zwp5.com";

/// 签名密钥（原实现 `this.c = "cb808529bae6b6be45ecfab29a4889bc"`）。
pub const KEY: &str = "cb808529bae6b6be45ecfab29a4889bc";

/// 播放请求头 UA（原实现 `playerContent` 内常量）。
const PLAY_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36";

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/117.0.0.0 Safari/537.36";

/// 固定分类（`homeContent` 的 `arrayList2`，顺序保持原实现）。
const CATEGORIES: [(&str, &str); 4] = [("1", "电影"), ("2", "电视剧"), ("4", "动漫"), ("3", "综艺")];

/// 线路组名（`detailContent` 里 `linkedHashMap.put("在线播放", …)`）。
const PLAY_FROM: &str = "在线播放";

pub fn handles(api: &str) -> bool {
    api.trim().eq_ignore_ascii_case(API)
}

/* ============================================================
 * 签名
 * ============================================================ */

pub fn md5_hex(input: &str) -> String {
    format!("{:x}", md5::compute(input.as_bytes()))
}

/// 标准 SHA-1（项目未引入 `sha1` crate，与原实现 `sha1()` 等价）。
pub fn sha1_hex(input: &str) -> String {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut message = input.as_bytes().to_vec();
    let bit_len = (message.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            let x = w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16];
            w[i] = x.rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
}

/// 原实现 `sha1(z.a(paramString))`：`z.a` 是 MD5 hex。
pub fn sign(params: &str) -> String {
    sha1_hex(&md5_hex(params))
}

fn now_ms() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    millis.to_string()
}

/* ============================================================
 * 请求
 * ============================================================ */

fn base_of(site: &Site) -> Result<String> {
    let raw = site.ext_string();
    if raw.trim().is_empty() {
        return Ok(DEFAULT_BASE.to_string());
    }
    for part in raw.split(',') {
        let candidate = part.trim();
        if candidate.starts_with("http://") || candidate.starts_with("https://") {
            return Ok(candidate.trim_end_matches('/').to_string());
        }
    }
    Err(CatvodError::new(
        ErrorClass::Parse,
        site,
        format!("csp_Jpys 的 ext 不含 http(s) 域名：{raw}"),
    )
    .into_core())
}

fn headers(base: &str, timestamp: &str, sign_value: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Accept".to_string(), "application/json, text/plain, */*".to_string());
    headers.insert("sign".to_string(), sign_value.to_string());
    headers.insert("T".to_string(), timestamp.to_string());
    headers.insert("Deviceid".to_string(), "Deviceid".to_string());
    let _ = base;
    headers
}

/// 构造签名参数（`params` 里的 `{t}` 由当前毫秒时间戳替换），带签名 GET 并按 JSON 解析。
fn fetch_json(
    cfg: &TvConfig,
    site: &Site,
    base: &str,
    url: &str,
    params: impl Fn(&str) -> String,
) -> Result<Value> {
    let timestamp = now_ms();
    let params = params(&timestamp);
    let sign_value = sign(&params);
    let client = crate::http::client_for(&cfg.http)?;
    let body = crate::http::get_text(&client, url, &headers(base, &timestamp, &sign_value), 2)?;
    serde_json::from_str::<Value>(&body).map_err(|err| {
        CatvodError::new(
            ErrorClass::Parse,
            site,
            format!("响应不是 JSON（{err}）：{url}"),
        )
        .into_core()
    })
}

pub fn categories() -> Vec<Category> {
    CATEGORIES
        .iter()
        .map(|(id, name)| Category {
            type_id: (*id).to_string(),
            type_name: (*name).to_string(),
        })
        .collect()
}

pub fn videos(cfg: &TvConfig, site: &Site, cat: &str, page: u32) -> Result<Vec<VideoItem>> {
    let base = base_of(site)?;
    let url = format!(
        "{base}/api/mw-movie/anonymous/video/list?type1={cat}&pageNum={page}&area=&year="
    );
    let value = fetch_json(cfg, site, &base, &url, |t| {
        format!("area=&pageNum={page}&type1={cat}&year=&key={KEY}&t={t}")
    })?;
    Ok(parse_list(&value))
}

pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let base = base_of(site)?;
    let url = format!(
        "{base}/api/mw-movie/anonymous/video/searchByWord?keyword={}&pageNum=1&pageSize=8",
        utf8_encode(keyword)
    );
    let value = fetch_json(cfg, site, &base, &url, |t| {
        format!("keyword={keyword}&pageNum=1&pageSize=8&key={KEY}&t={t}")
    })?;
    Ok(parse_search(&value))
}

pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let base = base_of(site)?;
    let url = format!(
        "{base}/api/mw-movie/anonymous/video/detail?id={}",
        utf8_encode(id)
    );
    let value = fetch_json(cfg, site, &base, &url, |t| {
        format!("id={id}&key={KEY}&t={t}")
    })?;
    parse_detail(site, id, &value)
}

/// 分集 id 形如 `{vodId}@{nid}`（原实现用混淆字节拼出 `'@'`）。
pub fn player_content(cfg: &TvConfig, site: &Site, _flag: &str, id: &str) -> Result<Value> {
    let base = base_of(site)?;
    let Some((vod_id, nid)) = id.split_once('@') else {
        return Err(CatvodError::new(
            ErrorClass::HostApi,
            site,
            format!("csp_Jpys 的分集 id 缺少 `@` 分隔：{id}"),
        )
        .into_core());
    };
    let url = format!(
        "{base}/api/mw-movie/anonymous/v2/video/episode/url?id={}&nid={}",
        utf8_encode(vod_id),
        utf8_encode(nid)
    );
    let value = fetch_json(cfg, site, &base, &url, |t| {
        format!("id={vod_id}&nid={nid}&key={KEY}&t={t}")
    })?;
    let play_url = parse_play(&value);
    if play_url.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::HostApi,
            site,
            "播放接口没有返回 url（可能需要登录或该集已下线）",
        )
        .into_core());
    }
    let header = serde_json::json!({
        "User-Agent": PLAY_UA,
        "Origin": base,
        "Referer": base,
    });
    Ok(serde_json::json!({
        "url": play_url,
        "parse": 0,
        "jx": 0,
        "playUrl": "",
        "header": header,
    }))
}

/* ============================================================
 * 解析
 * ============================================================ */

pub fn parse_list(value: &Value) -> Vec<VideoItem> {
    let mut out = Vec::new();
    let Some(items) = value.pointer("/data/list").and_then(|v| v.as_array()) else {
        return out;
    };
    for item in items {
        let id = scalar(item, "vodId");
        if id.is_empty() {
            continue;
        }
        let name = scalar(item, "vodName");
        let cover = scalar(item, "vodPic");
        let mut remark = scalar(item, "vodVersion");
        if remark.is_empty() {
            remark = scalar(item, "vodRemarks");
        }
        out.push(VideoItem {
            id,
            name: clean(&name),
            cover: opt(cover),
            remark: opt(remark),
        });
    }
    out
}

pub fn parse_search(value: &Value) -> Vec<VideoItem> {
    let mut out = Vec::new();
    let Some(items) = value.pointer("/data/result/list").and_then(|v| v.as_array()) else {
        return out;
    };
    for item in items {
        if scalar(item, "vodClass") == "伦理" {
            continue;
        }
        let id = scalar(item, "vodId");
        if id.is_empty() {
            continue;
        }
        out.push(VideoItem {
            id,
            name: clean(&scalar(item, "vodName")),
            cover: opt(scalar(item, "vodPic")),
            remark: opt(scalar(item, "vodRemarks")),
        });
    }
    out
}

pub fn parse_detail(site: &Site, id: &str, value: &Value) -> Result<VideoDetail> {
    let Some(data) = value.pointer("/data") else {
        return Err(CatvodError::new(ErrorClass::Parse, site, "详情响应缺少 data").into_core());
    };
    let name = clean(&scalar(data, "vodName"));
    let mut items: Vec<Episode> = Vec::new();
    if let Some(episodes) = data.get("episodeList").and_then(|v| v.as_array()) {
        for ep in episodes {
            let nid = scalar(ep, "nid");
            if nid.is_empty() {
                continue;
            }
            let title = {
                let raw = clean(&scalar(ep, "name"));
                if raw.is_empty() {
                    format!("第{}集", items.len() + 1)
                } else {
                    raw
                }
            };
            items.push(Episode {
                title,
                url: format!("{id}@{nid}"),
            });
        }
    }
    if name.is_empty() && items.is_empty() {
        return Err(CatvodError::new(
            ErrorClass::Parse,
            site,
            "详情页缺少标题与分集（接口结构可能已变）",
        )
        .into_core());
    }
    let groups = if items.is_empty() {
        Vec::new()
    } else {
        vec![EpisodeGroup {
            group: PLAY_FROM.to_string(),
            items,
        }]
    };
    let desc = {
        let blurb = scalar(data, "vodBlurb");
        if blurb.is_empty() {
            scalar(data, "vodContent")
        } else {
            blurb
        }
    };
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover: opt(scalar(data, "vodPic")),
        desc: opt(strip_tags(&desc)),
        year: opt(scalar(data, "vodYear")),
        area: opt(scalar(data, "vodArea")),
        director: opt(clean(&scalar(data, "vodDirector"))),
        actor: opt(clean(&scalar(data, "vodActor"))),
        episodes: groups,
    })
}

pub fn parse_play(value: &Value) -> String {
    scalar(value, "/data/list/0/url")
}

/* ============================================================
 * 小工具
 * ============================================================ */

/// 按 key 取标量；key 可以是 `a/b/0/c` 形式的 JSON Pointer 片段（不带前导 `/` 亦可）。
fn scalar(value: &Value, key: &str) -> String {
    let pointer = if key.starts_with('/') {
        key.to_string()
    } else if key.contains('/') {
        format!("/{key}")
    } else {
        format!("/{key}")
    };
    let target = if value.pointer(&pointer).is_some() {
        value.pointer(&pointer)
    } else {
        value.get(key)
    };
    match target {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

fn opt(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn clean(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    clean(&out)
}

fn utf8_encode(raw: &str) -> String {
    percent_encoding::utf8_percent_encode(raw, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_Jpys"));
        assert!(handles(" CSP_JPYS "));
        assert!(!handles("csp_Xb6v"));
    }

    #[test]
    fn sha1_matches_known_vectors() {
        assert_eq!(sha1_hex(""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex("abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sign_matches_node_reference() {
        // node: sha1(md5("id=1&key=…&t=1700000000000"))
        let params = format!("id=1&key={KEY}&t=1700000000000");
        assert_eq!(sign(&params), "c60b256cd42fdab025fc49bde74037417b5f4a67");
        let list_params = format!("area=&pageNum=1&type1=1&year=&key={KEY}&t=1700000000000");
        assert_eq!(sign(&list_params), "4e16950fa4fe00c231b7039c79b6fddcba131061");
    }

    #[test]
    fn parses_list_rows() {
        let value = json!({"data": {"list": [
            {"vodId": 147973, "vodName": "杀或救", "vodPic": "https://img/1.webp", "vodVersion": "蓝光"},
            {"vodId": "114815", "vodName": "海贼王", "vodPic": "", "vodVersion": "", "vodRemarks": "(1180集)"}
        ]}});
        let items = parse_list(&value);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "147973");
        assert_eq!(items[0].name, "杀或救");
        assert_eq!(items[0].cover.as_deref(), Some("https://img/1.webp"));
        assert_eq!(items[0].remark.as_deref(), Some("蓝光"));
        assert_eq!(items[1].id, "114815");
        assert!(items[1].cover.is_none());
        assert_eq!(items[1].remark.as_deref(), Some("(1180集)"));
    }

    #[test]
    fn parses_search_and_skips_ethics() {
        let value = json!({"data": {"result": {"list": [
            {"vodId": 1, "vodName": "正常片", "vodClass": "剧情", "vodRemarks": ""},
            {"vodId": 2, "vodName": "伦理片", "vodClass": "伦理"},
            {"vodId": 3, "vodName": "海贼王", "vodClass": "动画", "vodRemarks": "(1180集)"}
        ]}}});
        let items = parse_search(&value);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "1");
        assert_eq!(items[1].id, "3");
        assert_eq!(items[1].remark.as_deref(), Some("(1180集)"));
    }

    #[test]
    fn parses_detail_with_episodes() {
        let value = json!({"data": {
            "vodName": "海贼王", "vodPic": "https://img/1.webp", "vodYear": "1999", "vodArea": "日本",
            "vodActor": "田中真弓,冈村明美", "vodDirector": "宇田钢之助",
            "vodContent": "<p>剧情简介</p>",
            "episodeList": [{"nid": 795979, "name": "1"}, {"nid": 1320715, "name": "1180"}]
        }});
        let detail = parse_detail(&Site::default(), "114815", &value).expect("detail");
        assert_eq!(detail.name, "海贼王");
        assert_eq!(detail.year.as_deref(), Some("1999"));
        assert_eq!(detail.area.as_deref(), Some("日本"));
        assert_eq!(detail.director.as_deref(), Some("宇田钢之助"));
        assert_eq!(detail.desc.as_deref(), Some("剧情简介"));
        assert_eq!(detail.episodes.len(), 1);
        assert_eq!(detail.episodes[0].group, "在线播放");
        assert_eq!(detail.episodes[0].items.len(), 2);
        assert_eq!(detail.episodes[0].items[0].url, "114815@795979");
        assert_eq!(detail.episodes[0].items[1].title, "1180");
    }

    #[test]
    fn parses_empty_detail_as_error() {
        let value = json!({"data": {}});
        let err = parse_detail(&Site::default(), "x", &value).expect_err("must fail");
        assert!(format!("{err:?}").contains("catvod.result.parse"), "{err:?}");
    }

    #[test]
    fn parses_play_url() {
        let value = json!({"data": {"list": [{"url": "https://cdn.test/a.m3u8", "resolution": 1080}]}});
        assert_eq!(parse_play(&value), "https://cdn.test/a.m3u8");
        assert_eq!(parse_play(&json!({"data": {"list": []}})), "");
    }
}
