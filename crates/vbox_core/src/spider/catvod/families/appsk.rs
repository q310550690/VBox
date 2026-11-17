//! `csp_AppSK` 原生实现（「小柚」App：单站 JSON API，扁平化已按可读段还原）。
//!
//! ## 来源（REA 反编译证据）
//!
//! * jar `69dadb9a-Yoursmile.jar` 的 `com.github.catvod.spider.AppSK`：jadx 只给出
//!   前身片段；第 126 轮改用 dex2jar 2.4 + Vineflower 1.10.1 单类 →
//!   `_rea/tmp/appmao/sel-AppSK/AppSK.java`（2446 行、0 失败）。
//! * 字符串为 d2j 短数组 XOR 编码，`_rea/tmp/appmao/dec2.mjs` 解出 88/88 条
//!   （901 shorts、0 miss）→ `AppSK.decoded.java`。
//! * ext：`Base64 → RC4("fantaiying666") → 每字节 -1 → JSON`，与
//!   `crate::spider::catvod::appto` 同构（复用 `decode_ext_value`）。
//!
//! ## 协议（从 decoded 源的非扁平化段逐段还原）
//!
//! * ext JSON：`host`（API 基址，必填）、`key`/`iv`（AES）、`token`、`sign`。
//! * 头：`User-Agent: Dart/2.10 (dart:io)`；`tF` 非空时追加
//!   `authorization: Bearer <token>`（原类字段名即小写 `authorization`）。
//! * 分类：`GET {host}/sk-api/type/list` → `{"data":[{type_id,type_name}]}`。
//! * 首页列表：`GET {host}/sk-api/vod/list?page=1&limit=100&type=aqy` → `data[]`。
//! * 分类列表：
//!   `GET {host}/sk-api/vod/list?typeId={id}&page={page}&limit=18&type=updateTime`
//!   `&area=&lang=&year=&mtype=&extendtype=` → `data[]`；输出
//!   `{page, pagecount: 999, limit: 18, total: INT_MAX, list}`（原类硬编码）。
//! * 详情：`GET {host}/sk-api/vod/one?vodId={id}` → `data` 映射 `vod_*`；
//!   `vod_play_from`/`vod_play_url` 由服务端直接给出（`$$$` 分线路、`#` 分集）。
//! * 搜索：`GET {host}/sk-api/search/pages?keyword={kw}&page=1&limit=10&typeId=-1`
//!   → `data[]`。
//! * 播放：id 以 `.mp4`/`.m3u8` 结尾 → 直链 `parse=0`；否则
//!   `GET {host}/sk-api/vod/skjson?url={id}&skjsonindex=0` → `data.url` 直链。
//!
//! ## 有意偏差（原类自带问题或桌面端边界）
//!
//! 1. 上游 `https://yz.yzys.top:9777` 在 2026-11 已不可达（fetch failed），
//!    本家族只能 mock 验收（无真站 `#[ignore]` 用例）。
//! 2. 原类 `init` 在 ext 无 token 时会先请求 `{domain}/get_config` 用
//!    `{"sign":…,"ck":…}` 换取 `Bearer` token；其签名函数（`bytesToHexStr` /
//!    `encryptToHex`）在扁平化体里无法 100% 还原，本实现只在 ext 自带 `token`
//!    时发送 `authorization`，否则匿名请求（绝大多数公开接口不需要）。
//! 3. 原类 `homeContent` 的 `filters` 为固定 `new JSONObject("{}")`（空对象），
//!    桌面端 `list_videos` 无筛选参数，故不建模。
//! 4. 原类列表备注在 `vod_remarks` 为空时回退 `vod_douban_score`，本实现保留该回退。
//! 5. 播放返回体用数字 `parse: 0`（原类为字符串 `"0"`，TVBox 两种都接受）。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::error::{CoreError, Result};
use crate::http::client_for;
use crate::model::{Category, Episode, EpisodeGroup, Site, TvConfig, VideoDetail, VideoItem};
use crate::spider::catvod::appto::decode_ext_value;
use crate::spider::catvod::contract::{CatvodError, ErrorClass};

/// 家族标识（`profiles.json` 的 api 名）。
pub const API: &str = "csp_AppSK";
/// 原类 `getHeaders()` 的 UA。
pub const UA: &str = "Dart/2.10 (dart:io)";
/// 分类列表每页条数（原类硬编码 `limit=18`）。
pub const PAGE_LIMIT: i64 = 18;
/// 分类列表 `pagecount` 固定值（原类硬编码 999）。
pub const PAGECOUNT: i64 = 999;
/// 首页推荐列表 `limit`（原类 `limit=100&type=aqy`）。
pub const HOME_LIMIT: i64 = 100;
/// 分类列表端点。
pub const EP_LIST: &str = "/sk-api/vod/list";
/// 详情端点。
pub const EP_ONE: &str = "/sk-api/vod/one";
/// 分类端点。
pub const EP_TYPE_LIST: &str = "/sk-api/type/list";
/// 搜索端点。
pub const EP_SEARCH: &str = "/sk-api/search/pages";
/// 播放解析端点。
pub const EP_SKJSON: &str = "/sk-api/vod/skjson";
/// 单线路最多保留的剧集数。
const MAX_EPISODES: usize = 500;

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

/// ext：`host` 必填，`token` 可选。
struct Ext {
    host: String,
    token: Option<String>,
}

fn ext_of(site: &Site) -> Result<Ext> {
    let raw = site.ext_string();
    let value = decode_ext_value(&raw)
        .or_else(|_| serde_json::from_str::<Value>(raw.trim()).map_err(|e| e.to_string()))
        .map_err(|e| parse_err(site, format!("AppSK ext 解码失败: {e}")))?;
    let host = value
        .get("host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| parse_err(site, "AppSK ext 缺少 host"))?
        .trim_end_matches('/')
        .to_string();
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(Ext { host, token })
}

fn headers(ext: &Ext) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("User-Agent".to_string(), UA.to_string());
    if let Some(token) = &ext.token {
        let value = if token.starts_with("Bearer ") {
            token.to_string()
        } else {
            format!("Bearer {token}")
        };
        h.insert("authorization".to_string(), value);
    }
    h
}

fn get_json(cfg: &TvConfig, site: &Site, url: &str, extra: &HashMap<String, String>) -> Result<Value> {
    let client = client_for(&cfg.http)?;
    let text = crate::http::get_text(&client, url, extra, 2)?;
    serde_json::from_str(&text).map_err(|e| parse_err(site, format!("响应不是 JSON（{url}）: {e}")))
}

/// 原类 `yq(String)` 的等价实现：相对地址归一化到 host。
fn yq(base: &str, raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if raw.contains("http") {
        return raw.to_string();
    }
    if raw.contains("mac://") {
        return raw.replace("mac://", "http://");
    }
    if raw.starts_with('/') {
        return format!("{base}{raw}");
    }
    format!("{base}/{raw}")
}

fn s_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn id_field(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// 列表条目：`vod_id`/`vod_name` 必填；备注空时回退豆瓣评分。
fn item_from(v: &Value, base: &str) -> Option<VideoItem> {
    let id = id_field(v, "vod_id")?;
    let name = s_field(v, "vod_name")?.to_string();
    let cover = s_field(v, "vod_pic").map(|p| yq(base, p)).filter(|p| !p.is_empty());
    let remark = s_field(v, "vod_remarks")
        .map(str::to_string)
        .or_else(|| s_field(v, "vod_douban_score").map(str::to_string));
    Some(VideoItem { id, name, cover, remark })
}

fn items_from(value: &Value, base: &str) -> Vec<VideoItem> {
    value
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .map(|arr| arr.iter().filter_map(|v| item_from(v, base)).collect())
        .unwrap_or_default()
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

/// 分类：`GET {host}/sk-api/type/list`。
pub fn categories(cfg: &TvConfig, site: &Site) -> Result<Vec<Category>> {
    let ext = ext_of(site)?;
    let url = format!("{}{}", ext.host, EP_TYPE_LIST);
    let value = get_json(cfg, site, &url, &headers(&ext))?;
    let arr = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| parse_err(site, "分类响应缺少 data 数组"))?;
    let mut out = Vec::new();
    for v in arr {
        let type_id = id_field(v, "type_id").or_else(|| id_field(v, "id"));
        let type_name = s_field(v, "type_name").map(str::to_string);
        if let (Some(type_id), Some(type_name)) = (type_id, type_name) {
            out.push(Category { type_id, type_name });
        }
    }
    if out.is_empty() {
        return Err(parse_err(site, "分类响应为空"));
    }
    Ok(out)
}

/// 首页推荐：`GET {host}/sk-api/vod/list?page=1&limit=100&type=aqy`。
pub fn home_videos(cfg: &TvConfig, site: &Site) -> Result<Vec<VideoItem>> {
    let ext = ext_of(site)?;
    let url = format!(
        "{}{}?page=1&limit={HOME_LIMIT}&type=aqy",
        ext.host, EP_LIST
    );
    let value = get_json(cfg, site, &url, &headers(&ext))?;
    Ok(items_from(&value, &ext.host))
}

/// 分类列表：原类 18 条/页、`pagecount` 固定 999。
pub fn videos(cfg: &TvConfig, site: &Site, type_id: &str, page: u32) -> Result<Vec<VideoItem>> {
    let ext = ext_of(site)?;
    let url = format!(
        "{}{}?typeId={}&page={page}&limit={PAGE_LIMIT}&type=updateTime&area=&lang=&year=&mtype=&extendtype=",
        ext.host,
        EP_LIST,
        urlencode(type_id)
    );
    let value = get_json(cfg, site, &url, &headers(&ext))?;
    Ok(items_from(&value, &ext.host))
}

/// 搜索：固定 `page=1&limit=10&typeId=-1`（原类如此）。
pub fn search(cfg: &TvConfig, site: &Site, keyword: &str) -> Result<Vec<VideoItem>> {
    let ext = ext_of(site)?;
    let url = format!(
        "{}{}?keyword={}&page=1&limit=10&typeId=-1",
        ext.host,
        EP_SEARCH,
        urlencode(keyword)
    );
    let value = get_json(cfg, site, &url, &headers(&ext))?;
    Ok(items_from(&value, &ext.host))
}

/// 把 `data` 对象转成 `VideoDetail`（纯函数，便于单测）。
fn detail_from(site: &Site, id: &str, data: &Value, base: &str) -> Result<VideoDetail> {
    let name = s_field(data, "vod_name")
        .ok_or_else(|| parse_err(site, "详情缺少 vod_name"))?
        .to_string();
    let cover = s_field(data, "vod_pic").map(|p| yq(base, p)).filter(|p| !p.is_empty());
    let desc = s_field(data, "vod_content").map(strip_html).filter(|s| !s.is_empty());
    let from = s_field(data, "vod_play_from").map(str::to_string);
    let urls = s_field(data, "vod_play_url").map(str::to_string);
    let mut groups = Vec::new();
    if let (Some(from), Some(urls)) = (from, urls) {
        let names: Vec<&str> = from.split("$$$").collect();
        let lines: Vec<&str> = urls.split("$$$").collect();
        for (idx, line) in lines.iter().enumerate() {
            let group = names.get(idx).copied().unwrap_or("").trim().to_string();
            let mut items = Vec::new();
            for ep in line.split('#').take(MAX_EPISODES) {
                let ep = ep.trim();
                if ep.is_empty() {
                    continue;
                }
                let (title, url) = match ep.split_once('$') {
                    Some((t, u)) => (t.trim().to_string(), u.trim().to_string()),
                    None => (ep.to_string(), ep.to_string()),
                };
                if url.is_empty() {
                    continue;
                }
                items.push(Episode { title, url });
            }
            if !items.is_empty() {
                groups.push(EpisodeGroup { group, items });
            }
        }
    }
    if groups.is_empty() {
        return Err(parse_err(site, "详情缺少可播放线路（vod_play_from/vod_play_url 为空）"));
    }
    Ok(VideoDetail {
        id: id.to_string(),
        name,
        cover,
        desc,
        year: s_field(data, "vod_year").map(str::to_string),
        area: s_field(data, "vod_area").map(str::to_string),
        director: s_field(data, "vod_director").map(str::to_string),
        actor: s_field(data, "vod_actor").map(str::to_string),
        episodes: groups,
    })
}

/// 详情：`GET {host}/sk-api/vod/one?vodId={id}`。
pub fn detail(cfg: &TvConfig, site: &Site, id: &str) -> Result<VideoDetail> {
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "详情 id 为空"));
    }
    let ext = ext_of(site)?;
    let url = format!("{}{}?vodId={}", ext.host, EP_ONE, urlencode(id));
    let value = get_json(cfg, site, &url, &headers(&ext))?;
    let data = value
        .get("data")
        .ok_or_else(|| parse_err(site, "详情响应缺少 data"))?;
    detail_from(site, id, data, &ext.host)
}

/// 直链判定（原类 `isVideoFormat`：`.mp4`/`.m3u8` 后缀）。
fn is_direct_video(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains(".mp4") || lower.contains(".m3u8")
}

/// 从 skjson 响应里取 `data.url`（纯函数）。
fn skjson_url(value: &Value) -> Option<String> {
    value
        .get("data")
        .and_then(|d| d.get("url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 播放：直链直出；否则经 `/sk-api/vod/skjson` 换直链。
pub fn player_content(cfg: &TvConfig, site: &Site, flag: &str, id: &str) -> Result<Value> {
    let _ = flag;
    let id = id.trim();
    if id.is_empty() {
        return Err(parse_err(site, "播放 id 为空"));
    }
    let direct_header = json!({ "User-Agent": UA });
    if is_direct_video(id) {
        return Ok(json!({ "parse": 0, "url": id, "playUrl": "", "header": direct_header }));
    }
    let ext = ext_of(site)?;
    let url = format!(
        "{}{}?url={}&skjsonindex=0",
        ext.host,
        EP_SKJSON,
        urlencode(id)
    );
    let value = get_json(cfg, site, &url, &headers(&ext))?;
    let real = skjson_url(&value)
        .ok_or_else(|| host_err(site, "skjson 响应缺少 data.url：播放地址无法解析"))?;
    Ok(json!({ "parse": 0, "url": real, "playUrl": "", "header": direct_header }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spider::catvod::appto::rc4;
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

    /// 小柚（`csp_AppSK`）在 `sites.json` 里的真实 ext。
    const REAL_EXT: &str = "FbjPcVZY48PN/RIZ5QqOrsp7JoZYCZdLEy9R9ri6ykNdUNSIsT3IgxCP9qyzByO7ZG6Z030vgWoyXGNlCJx9KFpYXtyZ3B/cLTtO0SAclaw3AOm0fS9oLx2taGF6hg==";

    fn site(ext: Value) -> Site {
        Site {
            key: "appsk".to_string(),
            name: "小柚".to_string(),
            api: API.to_string(),
            kind: 3,
            ext: Some(ext),
            ..Default::default()
        }
    }

    fn plain_site() -> Site {
        site(json!({ "host": "http://127.0.0.1:9/sk/", "token": "t-123" }))
    }

    fn encode_ext(raw: &str) -> String {
        let mut plain = Vec::with_capacity(raw.len());
        for b in raw.as_bytes() {
            plain.push(b.wrapping_add(1));
        }
        B64.encode(&rc4(b"fantaiying666", &plain))
    }

    #[test]
    fn handles_is_case_insensitive() {
        assert!(handles("csp_AppSK"));
        assert!(handles(" csp_appsk "));
        assert!(!handles("csp_AppLd"));
    }

    #[test]
    fn decodes_real_encoded_ext() {
        let ext = ext_of(&site(json!(REAL_EXT))).expect("ext");
        assert_eq!(ext.host, "https://yz.yzys.top:9777");
        assert!(ext.token.is_none());
    }

    #[test]
    fn accepts_raw_json_ext_and_trims_host() {
        let ext = ext_of(&plain_site()).expect("ext");
        assert_eq!(ext.host, "http://127.0.0.1:9/sk");
        assert_eq!(ext.token.as_deref(), Some("t-123"));
    }

    #[test]
    fn decodes_generated_encoded_ext() {
        let raw = r#"{"host":"http://a.b/sk/","key":"k","iv":"v","token":""}"#;
        let ext = ext_of(&site(json!(encode_ext(raw)))).expect("ext");
        assert_eq!(ext.host, "http://a.b/sk");
        assert!(ext.token.is_none());
    }

    #[test]
    fn headers_add_bearer_only_when_token_present() {
        let ext = Ext {
            host: "http://a".to_string(),
            token: Some("abc".to_string()),
        };
        let h = headers(&ext);
        assert_eq!(h.get("authorization").map(String::as_str), Some("Bearer abc"));
        assert_eq!(h.get("User-Agent").map(String::as_str), Some(UA));
        // 已带 Bearer 前缀时不重复添加。
        let ext = Ext {
            host: "http://a".to_string(),
            token: Some("Bearer zzz".to_string()),
        };
        assert_eq!(
            headers(&ext).get("authorization").map(String::as_str),
            Some("Bearer zzz")
        );
    }

    #[test]
    fn yq_normalizes_relative_forms() {
        assert_eq!(yq("http://h", ""), "");
        assert_eq!(yq("http://h", "http://cdn/a.jpg"), "http://cdn/a.jpg");
        assert_eq!(yq("http://h", "mac://cdn/a.jpg"), "http://cdn/a.jpg");
        assert_eq!(yq("http://h", "/img/a.jpg"), "http://h/img/a.jpg");
        assert_eq!(yq("http://h", "img/a.jpg"), "http://h/img/a.jpg");
    }

    #[test]
    fn item_remark_falls_back_to_douban_score() {
        let base = "http://h";
        let item = item_from(
            &json!({"vod_id": 7, "vod_name": "片名", "vod_pic": "/p.jpg", "vod_remarks": "", "vod_douban_score": "8.1"}),
            base,
        )
        .expect("item");
        assert_eq!(item.id, "7");
        assert_eq!(item.cover.as_deref(), Some("http://h/p.jpg"));
        assert_eq!(item.remark.as_deref(), Some("8.1"));
        let item = item_from(
            &json!({"vod_id": "8", "vod_name": "B", "vod_remarks": "全 12 集", "vod_douban_score": "9.0"}),
            base,
        )
        .expect("item");
        assert_eq!(item.remark.as_deref(), Some("全 12 集"));
    }

    #[test]
    fn detail_maps_play_from_and_play_url() {
        let data = json!({
            "vod_name": "示例剧",
            "vod_pic": "/cover.jpg",
            "vod_content": "<p>第一段</p><p>第二段</p>",
            "vod_year": "2025",
            "vod_area": "大陆",
            "vod_actor": "演员甲",
            "vod_director": "导演乙",
            "vod_play_from": "线路一$$$线路二",
            "vod_play_url": "第1集$http://a/1.m3u8#第2集$http://a/2.m3u8$$$正片$http://b/x.mp4"
        });
        let detail = detail_from(&plain_site(), "42", &data, "http://h").expect("detail");
        assert_eq!(detail.name, "示例剧");
        assert_eq!(detail.desc.as_deref(), Some("第一段第二段"));
        assert_eq!(detail.cover.as_deref(), Some("http://h/cover.jpg"));
        assert_eq!(detail.episodes.len(), 2);
        assert_eq!(detail.episodes[0].group, "线路一");
        assert_eq!(detail.episodes[0].items.len(), 2);
        assert_eq!(detail.episodes[0].items[0].title, "第1集");
        assert_eq!(detail.episodes[1].group, "线路二");
        assert_eq!(detail.episodes[1].items[0].url, "http://b/x.mp4");
    }

    #[test]
    fn detail_rejects_missing_play_lists() {
        let err = detail_from(&plain_site(), "1", &json!({"vod_name": "无线路"}), "http://h")
            .expect_err("应失败");
        assert!(err.to_string().contains("catvod.result.parse"), "{err}");
    }

    #[test]
    fn player_passes_through_direct_video_urls() {
        let value = player_content(&TvConfig::default(), &plain_site(), "", "https://cdn/x.m3u8")
            .expect("player");
        assert_eq!(value["parse"], 0);
        assert_eq!(value["url"], "https://cdn/x.m3u8");
        assert!(player_content(&TvConfig::default(), &plain_site(), "", "  ")
            .expect_err("空 id")
            .to_string()
            .contains("catvod.result.parse"));
    }

    #[test]
    fn skjson_and_direct_helpers() {
        assert!(is_direct_video("https://x/a.MP4"));
        assert!(!is_direct_video("https://x/play?u=1"));
        let value = json!({"data": {"url": "https://cdn/real.m3u8"}});
        assert_eq!(skjson_url(&value).as_deref(), Some("https://cdn/real.m3u8"));
        assert!(skjson_url(&json!({"data": {}})).is_none());
    }
}
